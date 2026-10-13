use oci_spec::runtime::MountBuilder;
use precreate_hook_diagnostics::{write_error, ExitStatus};
use serde_json::{Map, Value};
use std::{
    env,
    fmt,
    fs::create_dir,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{self, Command},
};

const HOOK_ANNOTATION_KEY: &str = "com.hooks.squashfs_mounts";
const SQUASHFUSE_ENVVAR: &str = "PARALLAX_MP_SQUASHFUSE_CMD";
const SQUASHFUSE_FLAG_ENVVAR: &str = "PARALLAX_MP_SQUASHFUSE_FLAG";
const SQUASHFUSE_UID_ENVVAR: &str = "PARALLAX_MP_UID";
const SQUASHFUSE_GID_ENVVAR: &str = "PARALLAX_MP_GID";
const SQUASHFUSE_DEFAULT_CMD: &str = "squashfuse_ll";
const GRAPHROOT_DIRNAME: &str = "graphroot";
const INTERMEDIATE_MOUNTPOINT_PREFIX: &str = "squashfs_mount_";

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        if let Err(log_error) = write_error("squashfs_mounts", error.exit_status(), &error.to_string()) {
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
        let (source, target) = input.split_once(':')
            .ok_or_else(|| {
                Error::new(
                    ExitStatus::DataErr,
                    &format!("Validation error: invalid squashfs mount format for \"{input}\""),
                )
            })?;

        Ok(
            Mount {
                source: PathBuf::from(source),
                target: PathBuf::from(target),
            }
        )
    }
}

fn run() -> Result<()> {
    let mut value = read_stdin_json()?;
    let obj = ensure_obj(value.as_object_mut(), "top-level JSON must be an object")?;
    apply_updates(obj)?;
    write_stdout_json(&value)?;
    Ok(())
}

// Pretty-print output JSON with trailing newline
fn write_stdout_json(value: &Value) -> Result<()> {
    let mut stdout = io::stdout().lock();

    serde_json::to_writer_pretty(&mut stdout, value).map_err(|e| {
        Error::new(
            ExitStatus::Software,
            format!("Failed to write JSON to stdout: {e}"),
        )
    })?;

    stdout.write_all(b"\n").map_err(|e| {
        Error::new(
            ExitStatus::IoErr,
            format!("Failed to write newline to stdout: {e}"),
        )
    })?;

    stdout
        .flush()
        .map_err(|e| Error::new(ExitStatus::IoErr, format!("Failed to flush stdout: {e}")))?;

    Ok(())
}

fn apply_updates(obj: &mut Map<String, Value>) -> Result<()> {
    let status = get_container_status(obj);
    match status.as_str() {
        "stopped" => apply_poststop_updates(obj),
        &_ => apply_precreate_updates(obj),
    }
}

fn apply_precreate_updates(obj: &mut Map<String, Value>) -> Result<()> {

    let squashfs_mounts_string = match get_annotation_option(HOOK_ANNOTATION_KEY, obj) {
        Some(s) => s,
        None => return Ok(()),
    };

    let squashfs_mounts = squashfs_mounts_string.split(";");

    let squashfuse_cmd = match get_squashfuse_from_env() {
        Some(s) => s,
        None => String::from(SQUASHFUSE_DEFAULT_CMD),
    };

    let squashfuse_options = get_squashfuse_options();

    let graphroot_basedir = get_graphroot_basedir(obj)?;

    for (index, mount_string) in squashfs_mounts.enumerate() {
        let mount = Mount::from_string(mount_string)?;
        let mountpoint_dirname = format!("{INTERMEDIATE_MOUNTPOINT_PREFIX}{index}");
        let mountpoint_dirpath = graphroot_basedir.join(PathBuf::from(&mountpoint_dirname));
        eprintln!("{} - {:?} -> {:?}", index, &mount.source, &mountpoint_dirpath);
        squashfuse_mount(&squashfuse_cmd, &squashfuse_options, &mount.source, &mountpoint_dirpath)?;
        add_bindmount(obj, &mountpoint_dirpath, &mount.target)?;
    };

    Ok(())
}

fn get_container_status(obj: &mut Map<String, Value>) -> String {
    let status = match obj.get("status") {
        Some(s) => s.as_str().unwrap_or(""),
        None => "",
    };

    String::from(status)
}

fn get_annotation_option(key: &str, obj: &mut Map<String, Value>) -> Option<String> {

    let annotations_obj = match obj.get("annotations") {
        Some(av) => match av.as_object() {
            Some(ao) => ao,
            None => return None,
        },
        None => return None,
    };

    let string = match annotations_obj.get(key) {
        Some(v) => match v.as_str() {
            Some(s) => s.to_string(),
            None => return None,
        },
        None => return None, 
    };

    Some(string)
}

fn get_squashfuse_from_env() -> Option<String> {
    env::var(SQUASHFUSE_ENVVAR).ok()
}

fn get_squashfuse_options() -> Vec<String> {
    let mut options: Vec<String> = vec![];

    for opt in env::var(SQUASHFUSE_FLAG_ENVVAR).unwrap_or(String::from("")).split_whitespace() {
        options.push(opt.to_string());
    }

    let uid = env::var(SQUASHFUSE_UID_ENVVAR).ok();
    let gid = env::var(SQUASHFUSE_GID_ENVVAR).ok();

    if uid.is_some() && gid.is_some() {
        let u = uid.unwrap();
        let g = gid.unwrap();
        options.push(format!("-o uid={u},gid={g}"));
    }

    options
}

fn get_graphroot_basedir(obj: &mut Map<String, Value>) -> Result<PathBuf> {
    let rootpath_string = obj.get("root")
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'root' doesn't exist",
            )
        })?
        .as_object()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'root' is not an object",
            )
        })?
        .get("path")
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'path' doesn't exist",
            )
        })?
        .as_str()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'path' is not a string",
            )
        })?;

    // /dev/shm/skybox_7210.0/graphroot/overlay/ca2fdb40f38090441ba6b6fac508812276cba66cdc1465fc96d33d44d3d1e065/merged"
    let rootpath = PathBuf::from(rootpath_string);
    
    let mut graphroot_basedir = PathBuf::from("/");

    for a in rootpath.ancestors() {
        let filename = match a.file_name() {
            Some(os) => os,
            None => {
                return Err(Error::new(
                    ExitStatus::DataErr,
                    "Validation error: filename not found",
                ));
            },
        };
        if filename == GRAPHROOT_DIRNAME {
            graphroot_basedir = match a.parent() {
                Some(p) => p.to_path_buf(),
                None => {
                    return Err(Error::new(
                        ExitStatus::DataErr,
                        "Validation error: 'graphroot' should have a parent directory",
                    ));
                },
            };
            break;
        }
    }

    if graphroot_basedir == PathBuf::from("/") {
        return Err(Error::new(
            ExitStatus::DataErr,
            "Validation error: cannot find graphroot base directory",
        ));
    }

    Ok(graphroot_basedir)
}

fn squashfuse_mount(cmd: &str, options: &Vec<String>, source: &Path, target: &Path) -> Result<()> {

    if ! source.is_file() {
        return Err(Error::new(
            ExitStatus::DataErr,
            format!("Validation error: source {:?} is not a file", source),
        ));
    }

    if target.exists() {
        return Err(Error::new(
            ExitStatus::DataErr,
            format!("Validation error: intermediate mount folder {:?} already exists", target),
        ));
    } else {

        let basedir = match target.parent() {
            Some(p) => p,
            None => {
                return Err(Error::new(
                    ExitStatus::DataErr,
                    format!("Validation error: intermediate mount folder {:?} doesn't have a parent", target),
                ));
            },
        };

        if !basedir.exists() {
            return Err(Error::new(
                ExitStatus::DataErr,
                format!("Validation error: cannot create intermediate folder because base directory {:?} doesn't exists", basedir),
            ));
        }

        let _ = create_dir(target).map_err(|_| {
            return Err::<(), Error>(Error::new(
                ExitStatus::DataErr,
                format!("Validation error: cannot create directory: {:?}", target),
            ));
        });
           
    }

    let output = Command::new(cmd)
        .args(options)
        .arg(source)
        .arg(target)
        .output()
        .map_err(|_| {
            return Err::<(), Error>(Error::new(
                ExitStatus::DataErr,
                "Validation error: cannot spawn squashfuse mount process",
            ));
        })
        .unwrap();

    if !output.status.success() {
        return Err(Error::new(
            ExitStatus::DataErr,
            "Validation error: squashfuse mount failed",
        ));
    }
    Ok(())

}
/*
fn get_env_entry_str<'a>(env: &'a HashMap<String, String>, key: &str) -> &'a str {
    env.get(key).map(|s| s.as_str()).unwrap_or("")
}

fn is_pattern_in_env_keys<'a>(env: &'a HashMap<String, String>, pattern: &str) -> bool {
    env.keys().any(|k| k.starts_with(pattern))
}
*/

fn add_bindmount(obj: &mut Map<String, Value>, source: &Path, target: &Path) -> Result<()> {
    let opts: Vec<String> = vec!["private", "nosuid", "nodev", "ro", "bind"]
        .into_iter()
        .map(String::from)
        .collect();

    let source_str = source.to_str()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                format!("Validation error: source cannot be converted to string"),
            )
        }).unwrap();

    let target_str = target.to_str()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                format!("Validation error: target cannot be converted to string"),
            )
        }).unwrap();

    let new_mount = MountBuilder::default()
        .typ("bind")
        .options(opts)
        .source(source_str)
        .destination(target_str)
        .build()
        .map_err(|e| Error::new(ExitStatus::DataErr, format!("invalid new mount: {e}")))?;

    let new_mount_value = serde_json::to_value(new_mount).map_err(|e| {
        Error::new(
            ExitStatus::DataErr,
            format!("unable to serialize new mount: {e}"),
        )
    })?;

    match obj.get_mut("mounts") {
        Some(Value::Array(arr)) => arr.push(new_mount_value),
        Some(_) => {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'mounts' is not an array",
            ))
        }
        None => {
            obj.insert("mounts".to_string(), Value::Array(vec![new_mount_value]));
        }
    }
    Ok(())
}
/*
fn insert_process_env(obj: &mut Map<String, Value>, key: &str, value: &str) -> Result<()> {
    let env_array = get_process_env_array(obj)?;

    let mut found = false;
    for entry in env_array.iter_mut() {
        let s = entry.as_str().ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                format!(
                    "Validation error: 'process.env' array contains an item that is not a string"
                ),
            )
        })?;

        let (k, _) = parse_env_entry(s)?;
        if k == key {
            *entry = format!("{key}={value}").into();
            found = true;
            break;
        }
    }

    if !found {
        env_array.push(format!("{key}={value}").into());
    }
    Ok(())
}

fn get_process_env_hashmap(obj: &mut Map<String, Value>) -> Result<HashMap<String, String>> {
    let env_array = get_process_env_array(obj)?;

    let mut ret = HashMap::new();

    for entry in env_array.iter() {
        if !entry.is_string() {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' array contains an item that is not a string",
            ));
        }

        let (k, v) = parse_env_entry(entry.as_str().unwrap())?;
        ret.insert(String::from(k), String::from(v));
    }
    Ok(ret)
}

fn parse_env_entry(entry: &str) -> Result<(&str, &str)> {
    entry.split_once('=').ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            "Validation error: 'process.env' array contains an item without '=' separator",
        )
    })
}

fn get_process_env_array(obj: &mut Map<String, Value>) -> Result<&mut Vec<Value>> {
    obj.get_mut("process")
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process' doesn't exist",
            )
        })?
        .as_object_mut()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process' is not an object",
            )
        })?
        .get_mut("env")
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' doesn't exist",
            )
        })?
        .as_array_mut()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' is not an array",
            )
        })
}
*/

fn apply_poststop_updates(obj: &mut Map<String, Value>) -> Result<()> {
    Ok(())
}

// Precreate takes as stdin the container config json
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

/// Ensure a `Value` is an object and return it as a mutable map.
fn ensure_obj<'a>(
    candidate: Option<&'a mut Map<String, Value>>,
    err: &str,
) -> Result<&'a mut Map<String, Value>> {
    candidate.ok_or_else(|| Error::new(ExitStatus::DataErr, format!("Validation error: {err}")))
}
