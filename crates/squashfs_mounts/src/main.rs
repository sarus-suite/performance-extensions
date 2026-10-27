use standard_hook_diagnostics::{write_error, ExitStatus};
use serde_json::{Map, Value};
use std::{
    env::var,
    ffi::OsString,
    fmt,
    fs::{create_dir, read_to_string, set_permissions, Permissions},
    io::{self, Read},
    os::unix::fs::{chown, PermissionsExt},
    path::{Path, PathBuf},
    process::{self, Command},
};

const HOOK_ANNOTATION_KEY: &str = "com.hooks.squashfs_mounts";
const SQUASHFUSE_DEFAULT_CMD: &str = "squashfuse_ll";
const SQUASHFUSE_ENVVAR: &str = "PARALLAX_MP_SQUASHFUSE_CMD";
const DIR_MODE: u32 = 0o755;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        if let Err(log_error) =
            write_error("squashfs_mounts", error.exit_status(), &error.to_string())
        {
            eprintln!("squashfs_mounts: failed to write diagnostic log: {log_error}");
        }
        process::exit(error.exit_status().code());
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
struct Error {
    status: ExitStatus,
    message: String,
}

#[derive(Debug)]
struct Mount {
    source: PathBuf,
    target: PathBuf,
}

impl Error {
    fn new(status: ExitStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn exit_status(&self) -> ExitStatus {
        self.status
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl Mount {
    pub fn from_string(input: &str) -> Result<Mount> {
        let (source, target) = input.split_once(':').ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                &format!("Validation error: invalid squashfs mount format for \"{input}\""),
            )
        })?;

        let source_pb = PathBuf::from(source);
        if !source_pb.is_file() {
            return Err(Error::new(
                ExitStatus::Unavailable,
                format!("Validation error: source \"{source_pb:?}\" is not a file"),
            ));
        }

        let target_pb = PathBuf::from(target);
        if !target_pb.is_absolute() {
            return Err(Error::new(
                ExitStatus::DataErr,
                format!("Validation error: mount target \"{target_pb:?}\" must be absolute"),
            ));
        }

        Ok(Mount {
            source: source_pb,
            target: target_pb,
        })
    }
}

fn run() -> Result<()> {
    let value = read_stdin_json()?;
    let obj = ensure_obj(value.as_object(), "top-level JSON must be an object")?;
    do_hook(obj)?;
    Ok(())
}

fn do_hook(obj: &Map<String, Value>) -> Result<()> {
    let status = get_container_status(obj);
    match status {
        "created" => do_create_container_hook(obj),
        &_ => Ok(()),
    }
}

fn do_create_container_hook(obj: &Map<String, Value>) -> Result<()> {
    let squashfs_mounts_string = match get_annotation_option(HOOK_ANNOTATION_KEY, obj) {
        Some(s) => s,
        None => return Ok(()),
    };

    let squashfs_mounts = squashfs_mounts_string.split(";");


    let perms = Permissions::from_mode(DIR_MODE);
    let config = get_container_config(obj)?;
    let rootfs_dirpath = get_container_rootfs_path(&config)?;
    let user_obj = get_config_user(&config)?;
    let uid = get_config_user_uid(&user_obj)?;
    let gid = get_config_user_gid(&user_obj)?;

    for mount_string in squashfs_mounts {
        let mount = Mount::from_string(mount_string)?;
        let mount_target_relpath = pb_trim_root(PathBuf::from(&mount.target));
        let mountpoint_dirpath = rootfs_dirpath.join(mount_target_relpath);
        create_directory_tree(&mountpoint_dirpath, uid, gid, &perms)?;
        squashfuse_mount(&mount.source, &mountpoint_dirpath)?;
    }

    Ok(())
}

fn pb_trim_root(pb: PathBuf) -> PathBuf {
    match pb.strip_prefix("/") {
        Ok(p) => p.to_path_buf(),
        Err(_) => pb,
    }
}

fn get_container_status(obj: &Map<String, Value>) -> &str {
    match obj.get("status") {
        Some(s) => s.as_str().unwrap_or(""),
        None => "",
    }
}

fn get_container_rootfs_path(config: &Map<String, Value>) -> Result<PathBuf> {
    let root_obj = get_json_obj_key_object(config, "root")?;
    let rootfs = get_json_obj_key_string(root_obj, "path")?;
    Ok(PathBuf::from(rootfs))
}

fn get_container_bundle(obj: &Map<String, Value>) -> Result<PathBuf> {
    let bundle = get_json_obj_key_string(obj, "bundle")?;
    Ok(PathBuf::from(bundle))
}

fn get_container_config(obj: &Map<String, Value>) -> Result<Map<String, Value>> {
    let bundle = get_container_bundle(obj)?;
    let config_file = bundle.join(PathBuf::from("config.json"));
    let config_str = read_to_string(config_file)
        .map_err(|e| Error::new(ExitStatus::IoErr, format!("Failed to read from file: {e}")))?;
    let config_value: Value = serde_json::from_str(&config_str)
        .map_err(|e| Error::new(ExitStatus::DataErr, format!("Invalid Config JSON: {e}")))?;
    let config_obj = ensure_obj(
        config_value.as_object(),
        "Unable to find root object in Config JSON",
    )?
    .clone();

    Ok(config_obj)
}

fn get_config_user(config_obj: &Map<String, Value>) -> Result<&Map<String, Value>> {
    let process_obj = get_json_obj_key_object(config_obj, "process")?;
    let user_obj = get_json_obj_key_object(process_obj, "user")?;
    Ok(user_obj)
}

fn get_config_user_uid(user_obj: &Map<String, Value>) -> Result<u32> {
    let uid = get_json_obj_key_u32(user_obj, "uid")?;
    Ok(uid)
}

fn get_config_user_gid(user_obj: &Map<String, Value>) -> Result<u32> {
    let gid = get_json_obj_key_u32(user_obj, "gid")?;
    Ok(gid)
}

fn get_annotation_option(key: &str, obj: &Map<String, Value>) -> Option<String> {
    let annotation_obj = match get_json_obj_key_object(obj, "annotations") {
        Ok(o) => o,
        Err(_) => return None,
    };

    let string = match get_json_obj_key_string(annotation_obj, key) {
        Ok(s) => s,
        Err(_) => return None,
    };

    Some(string)
}

fn get_squashfuse_from_env() -> Option<String> {
    var(SQUASHFUSE_ENVVAR).ok()
}

fn get_squashfuse_options() -> Vec<String> {
    let mut options: Vec<String> = vec![];

    options.push(String::from("-o"));
    options.push(String::from("allow_other"));
    options
}

fn squashfuse_mount(source: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        return Err(Error::new(
            ExitStatus::Unavailable,
            format!("Validation error: target folder {:?} doesn't exist", target),
        ));
    }

    if !target.is_dir() {
        return Err(Error::new(
            ExitStatus::Unavailable,
            format!(
                "Validation error: target folder {:?} is not a directory",
                target
            ),
        ));
    }

    let cmd = match get_squashfuse_from_env() {
        Some(s) => s,
        None => String::from(SQUASHFUSE_DEFAULT_CMD),
    };

    let options = get_squashfuse_options();

    let mut additional_options: Vec<String> = vec![];
    if !is_empty_directory(target) {
        additional_options.push(format!("-o"));
        additional_options.push(format!("nonempty"));
    }

    let output = Command::new(cmd)
        .args(options)
        .args(additional_options)
        .arg(source)
        .arg(target)
        .output()
        .map_err(|e| {
            return Err::<(), Error>(Error::new(
                ExitStatus::IoErr,
                format!("Error: cannot spawn squashfuse mount process, {e}"),
            ));
        })
        .unwrap();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(
            ExitStatus::IoErr,
            format!("Error: squashfuse mount failed, {stderr}"),
        ));
    }
    Ok(())
}

fn create_directory_tree(path: &Path, uid: u32, gid: u32, perms: &Permissions) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    let mut tree_stack: Vec<OsString> = vec![];
    let mut folder = path.to_path_buf();

    while !folder.exists() {
        let dirname = match folder.file_name() {
            Some(name) => name.to_os_string(),
            None => {
                return Err(Error::new(
                    ExitStatus::DataErr,
                    format!(
                        "Validation error: unexpected missing directory name for {:?}",
                        folder
                    ),
                ));
            }
        };
        tree_stack.push(dirname);

        folder = match folder.parent() {
            Some(parent) => parent.to_path_buf(),
            None => {
                return Err(Error::new(
                    ExitStatus::DataErr,
                    format!(
                        "Validation error: unexpected missing parent for {:?}",
                        folder
                    ),
                ));
            }
        };
    }

    let mut d = tree_stack.pop();

    while d.is_some() {
        let dirname = d.unwrap();
        folder = folder.join(PathBuf::from(dirname));
        let _ = create_dir(&folder).map_err(|e| {
            return Err::<(), Error>(Error::new(
                ExitStatus::IoErr,
                format!("Error: cannot create directory at \"{folder:?}\", {e}"),
            ));
        });

        match set_permissions(&folder, perms.clone()) {
            Ok(_) => {}
            Err(e) => {
                return Err::<(), Error>(Error::new(
                    ExitStatus::IoErr,
                    format!("Error: cannot set permissions for directory at \"{folder:?}\", {e}"),
                ));
            }
        };

        match chown(&folder, Some(uid), Some(gid)) {
            Ok(_) => {}
            Err(e) => {
                return Err::<(), Error>(Error::new(
                    ExitStatus::IoErr,
                    format!("Error: cannot set ownership for directory at \"{folder:?}\", {e}"),
                ));
            }
        };

        d = tree_stack.pop();
    }
    Ok(())
}

fn is_empty_directory(path: &Path) -> bool {
    match path.read_dir() {
        Ok(mut content) => content.next().is_none(),
        Err(_) => false,
    }
}

fn get_json_obj_key_string(obj: &Map<String, Value>, key: &str) -> Result<String> {
    let value = get_json_obj_key_value(obj, key)?;
    let string = get_json_value_as_string(value, key)?;
    Ok(String::from(string))
}

fn get_json_obj_key_object<'a>(
    obj: &'a Map<String, Value>,
    key: &'a str,
) -> Result<&'a Map<String, Value>> {
    let value = get_json_obj_key_value(obj, key)?;
    let object = get_json_value_as_object(value, key)?;
    Ok(object)
}

fn get_json_obj_key_u32(obj: &Map<String, Value>, key: &str) -> Result<u32> {
    let value = get_json_obj_key_value(obj, key)?;
    let number = get_json_value_as_u32(value, key)?;
    Ok(number)
}

fn get_json_obj_key_value<'a>(obj: &'a Map<String, Value>, key: &'a str) -> Result<&'a Value> {
    let value = obj.get(key).ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            format!("Validation error: '{key}' doesn't exist"),
        )
    })?;

    Ok(value)
}

fn get_json_value_as_string(value: &Value, key: &str) -> Result<String> {
    let string = value.as_str().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            format!("Validation error: '{key}' is not a string"),
        )
    })?;

    Ok(String::from(string))
}

fn get_json_value_as_object<'a>(value: &'a Value, key: &'a str) -> Result<&'a Map<String, Value>> {
    let obj = value.as_object().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            format!("Validation error: '{key}' is not an object"),
        )
    })?;

    Ok(obj)
}

fn get_json_value_as_u32(value: &Value, key: &str) -> Result<u32> {
    let num_u64 = value.as_u64().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            format!("Validation error: '{key}' is not an u64 number"),
        )
    })?;
    let num_u32 = match u32::try_from(num_u64) {
        Ok(n) => n,
        Err(_) => {
            return Err(Error::new(
                ExitStatus::DataErr,
                format!("Validation error: '{key}' cannot be converted to u32"),
            ));
        }
    };

    Ok(num_u32)
}

// createContainer hook takes as stdin the state json
// We return error if we cannot read or
// if we cannot parse a valid input json
fn read_stdin_json() -> Result<Value> {
    let mut input = String::new();

    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| Error::new(ExitStatus::IoErr, format!("Failed to read from stdin: {e}")))?;

    serde_json::from_str(&input)
        .map_err(|e| Error::new(ExitStatus::DataErr, format!("Invalid JSON: {e}")))
}

/// Ensure a `Value` is an object and return it as a map.
fn ensure_obj<'a>(
    candidate: Option<&'a Map<String, Value>>,
    err: &str,
) -> Result<&'a Map<String, Value>> {
    candidate.ok_or_else(|| Error::new(ExitStatus::DataErr, format!("Validation error: {err}")))
}
