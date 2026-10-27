use oci_spec::runtime::{Spec, State, User};
use standard_hook_diagnostics::{write_error, ExitStatus};
use std::{
    env::var,
    ffi::OsString,
    fmt,
    fs::{create_dir, set_permissions, Permissions},
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
    let state: State = serde_json::from_reader(std::io::stdin()).map_err(|_| {
        Error::new(
            ExitStatus::IoErr,
            "IO error: failed to read state from stdin",
        )
    })?;

    let squashfs_mounts_string = match get_annotation_option(HOOK_ANNOTATION_KEY, &state) {
        Some(s) => s,
        None => return Ok(()),
    };

    let squashfs_mounts = squashfs_mounts_string.split(";").filter(|s| !s.is_empty());

    let perms = Permissions::from_mode(DIR_MODE);
    let config = get_container_config(&state)?;
    let rootfs_dirpath = get_container_rootfs_path(&config)?;
    let user = get_config_user(&config)?;
    let uid = user.uid();
    let gid = user.gid();

    for mount_string in squashfs_mounts {
        let mount = Mount::from_string(mount_string)?;
        let mount_target_relpath = pb_trim_root(&mount.target);
        let mountpoint_dirpath = rootfs_dirpath.join(mount_target_relpath);
        create_directory_tree(&mountpoint_dirpath, uid, gid, &perms)?;
        squashfuse_mount(&mount.source, &mountpoint_dirpath)?;
    }

    Ok(())
}

fn pb_trim_root(pb: &Path) -> &Path {
    pb.strip_prefix("/").unwrap_or(pb)
}

fn get_container_rootfs_path(config: &Spec) -> Result<PathBuf> {
    let root_obj = config.root().clone().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            "Validation error: missing root in config.json",
        )
    })?;
    let rootfs = root_obj.path().clone();
    Ok(rootfs)
}

fn get_container_config(state: &State) -> Result<Spec> {
    let bundle = state.bundle();
    let config_file = bundle.join(PathBuf::from("config.json"));
    let config = Spec::load(config_file).map_err(|_| {
        Error::new(
            ExitStatus::DataErr,
            format!("Validation error: failed to load config.json"),
        )
    })?;
    Ok(config)
}

fn get_config_user(config: &Spec) -> Result<User> {
    let process = config.process().clone().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            &format!("Validation error: missing process in config.json"),
        )
    })?;

    let user = process.user().clone();
    Ok(user)
}

fn get_annotation_option(key: &str, state: &State) -> Option<String> {
    let annotations = match state.annotations() {
        None => return None,
        Some(a) => a,
    };

    let string = match annotations.get(key) {
        None => return None,
        Some(s) => s.to_string(),
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
        .args(&options)
        .args(&additional_options)
        .arg(source)
        .arg(target)
        .output()
        .map_err(|e| {
            Error::new(
                ExitStatus::IoErr,
                format!("Error: cannot spawn squashfuse mount process, {e}"),
            )
        })?;

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
