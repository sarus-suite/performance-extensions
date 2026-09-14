use std::{
    fmt, fs,
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use precreate_hook_diagnostics::{write_error, ExitStatus};
use serde_json::{json, Map, Value};

const STATE_DIR_PREFIX: &str = "sarus-hook-";
const AUTHORIZED_KEYS_NAME: &str = "authorized_keys";
const IDENTITY_NAME: &str = "identity";
const DESTINATION: &str = "/etc/ssh/hpc-dev-authorized_keys";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

fn main() {
    if let Err(error) = run() {
        eprintln!("ssh_precreate_hook: {error}");
        if let Err(log_error) = write_error(
            "ssh_precreate_hook",
            error.exit_status(),
            &error.to_string(),
        ) {
            eprintln!("ssh_precreate_hook: failed to write diagnostic log: {log_error}");
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

fn run() -> Result<()> {
    let mut config = read_stdin_json()?;
    let config_object = config.as_object_mut().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            "OCI configuration must be a JSON object",
        )
    })?;

    let state = state_directory()?;
    let _lock = Lock::acquire(&state)?;
    let identity = ensure_identity(&state)?;
    let authorized_keys = state.join(AUTHORIZED_KEYS_NAME);
    write_authorized_keys(&identity, &authorized_keys)?;
    add_authorized_keys_mount(config_object, &authorized_keys)?;
    write_stdout_json(&config)
}

fn read_stdin_json() -> Result<Value> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| Error::new(ExitStatus::IoErr, format!("failed to read stdin: {error}")))?;
    serde_json::from_str(&input).map_err(|error| {
        Error::new(
            ExitStatus::DataErr,
            format!("invalid JSON on stdin: {error}"),
        )
    })
}

fn write_stdout_json(value: &Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value).map_err(|error| {
        Error::new(
            ExitStatus::Software,
            format!("failed to write JSON to stdout: {error}"),
        )
    })?;
    stdout.write_all(b"\n").map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to write newline: {error}"),
        )
    })?;
    stdout.flush().map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to flush stdout: {error}"),
        )
    })
}

fn state_directory() -> Result<PathBuf> {
    let state = state_directory_path()?;
    match fs::create_dir(&state) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(Error::new(
                ExitStatus::IoErr,
                format!(
                    "failed to create SSH state directory {}: {error}",
                    state.display()
                ),
            ));
        }
    }
    validate_directory(&state, "SSH state directory")?;
    fs::set_permissions(&state, fs::Permissions::from_mode(DIRECTORY_MODE)).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to set SSH state directory permissions: {error}"),
        )
    })?;
    Ok(state)
}

fn state_directory_path() -> Result<PathBuf> {
    Ok(state_directory_path_for_uid(state_owner_uid()?))
}

fn state_directory_path_for_uid(uid: u32) -> PathBuf {
    PathBuf::from("/tmp").join(format!("{STATE_DIR_PREFIX}{uid}"))
}

fn state_owner_uid() -> Result<u32> {
    let uid = effective_uid();
    if uid == 0 {
        host_uid_from_map(
            uid,
            &fs::read_to_string("/proc/self/uid_map").map_err(|error| {
                Error::new(
                    ExitStatus::Config,
                    format!("failed to read /proc/self/uid_map: {error}"),
                )
            })?,
        )
    } else {
        Ok(uid)
    }
}

fn host_uid_from_map(namespace_uid: u32, map: &str) -> Result<u32> {
    let namespace_uid = u64::from(namespace_uid);

    for line in map.lines().filter(|line| !line.trim().is_empty()) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(Error::new(
                ExitStatus::Config,
                format!("invalid /proc/self/uid_map line: {line:?}"),
            ));
        }
        let inside = fields[0].parse::<u64>().map_err(|error| {
            Error::new(
                ExitStatus::Config,
                format!("invalid namespace UID in /proc/self/uid_map: {error}"),
            )
        })?;
        let outside = fields[1].parse::<u64>().map_err(|error| {
            Error::new(
                ExitStatus::Config,
                format!("invalid host UID in /proc/self/uid_map: {error}"),
            )
        })?;
        let length = fields[2].parse::<u64>().map_err(|error| {
            Error::new(
                ExitStatus::Config,
                format!("invalid UID range in /proc/self/uid_map: {error}"),
            )
        })?;
        let end = inside.checked_add(length).ok_or_else(|| {
            Error::new(
                ExitStatus::Config,
                "UID range overflows in /proc/self/uid_map",
            )
        })?;

        if namespace_uid >= inside && namespace_uid < end {
            let host_uid = outside.checked_add(namespace_uid - inside).ok_or_else(|| {
                Error::new(
                    ExitStatus::Config,
                    "host UID overflows in /proc/self/uid_map",
                )
            })?;
            return u32::try_from(host_uid).map_err(|_| {
                Error::new(ExitStatus::Config, "mapped host UID does not fit in u32")
            });
        }
    }

    Err(Error::new(
        ExitStatus::Config,
        format!("UID {namespace_uid} is not mapped in /proc/self/uid_map"),
    ))
}

fn validate_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        Error::new(
            ExitStatus::Config,
            format!("{label} is not an accessible directory: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != effective_uid()
    {
        return Err(Error::new(
            ExitStatus::Config,
            format!("{label} must be a real directory owned by the effective user"),
        ));
    }
    Ok(())
}

fn ensure_identity(state: &Path) -> Result<PathBuf> {
    let identity = state.join(IDENTITY_NAME);
    match fs::symlink_metadata(&identity) {
        Ok(_) => validate_private_file(&identity, "private key")?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            generate_identity(state, &identity)?
        }
        Err(error) => {
            return Err(Error::new(
                ExitStatus::IoErr,
                format!(
                    "failed to inspect private key {}: {error}",
                    identity.display()
                ),
            ));
        }
    }
    Ok(identity)
}

fn generate_identity(state: &Path, identity: &Path) -> Result<()> {
    let work = state.join(format!(".prepare-{}-{}", process::id(), nonce()?));
    fs::create_dir(&work).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to create SSH preparation directory: {error}"),
        )
    })?;
    let generated = work.join(IDENTITY_NAME);
    let result = Command::new("ssh-keygen")
        .args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "sarus-ssh-session",
            "-f",
        ])
        .arg(&generated)
        .status();
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => {
            let _ = fs::remove_dir_all(&work);
            return Err(Error::new(
                ExitStatus::Software,
                format!("ssh-keygen failed with {status}"),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let _ = fs::remove_dir_all(&work);
            return Err(Error::new(
                ExitStatus::Unavailable,
                "ssh-keygen is required on the host",
            ));
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&work);
            return Err(Error::new(
                ExitStatus::IoErr,
                format!("failed to execute ssh-keygen: {error}"),
            ));
        }
    }
    fs::set_permissions(&generated, fs::Permissions::from_mode(FILE_MODE)).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to protect generated private key: {error}"),
        )
    })?;
    validate_private_file(&generated, "generated private key")?;
    fs::rename(&generated, identity).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!(
                "failed to install private key {}: {error}",
                identity.display()
            ),
        )
    })?;
    fs::remove_dir_all(&work).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to remove SSH preparation directory: {error}"),
        )
    })?;
    validate_private_file(identity, "private key")
}

fn write_authorized_keys(identity: &Path, authorized_keys: &Path) -> Result<()> {
    validate_not_symlink(authorized_keys, "authorized_keys")?;
    let output = Command::new("ssh-keygen")
        .args(["-y", "-P", "", "-f"])
        .arg(identity)
        .output()
        .map_err(|error| {
            let status = if error.kind() == io::ErrorKind::NotFound {
                ExitStatus::Unavailable
            } else {
                ExitStatus::IoErr
            };
            Error::new(status, format!("failed to execute ssh-keygen: {error}"))
        })?;
    if !output.status.success() {
        return Err(Error::new(
            ExitStatus::Software,
            format!(
                "ssh-keygen failed while deriving authorized_keys: {}",
                output
                    .status
                    .code()
                    .unwrap_or_else(|| 128 + output.status.signal().unwrap_or(0))
            ),
        ));
    }

    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .mode(FILE_MODE)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(authorized_keys)
        .map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!(
                    "failed to open authorized_keys {}: {error}",
                    authorized_keys.display()
                ),
            )
        })?;
    validate_open_file(&file, authorized_keys, "authorized_keys")?;
    file.set_len(0).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to truncate authorized_keys: {error}"),
        )
    })?;
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to seek authorized_keys: {error}"),
        )
    })?;
    file.write_all(&output.stdout).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to write authorized_keys: {error}"),
        )
    })?;
    if !output.stdout.ends_with(b"\n") {
        file.write_all(b"\n").map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to finish authorized_keys: {error}"),
            )
        })?;
    }
    file.set_permissions(fs::Permissions::from_mode(FILE_MODE))
        .map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to protect authorized_keys: {error}"),
            )
        })
}

fn validate_not_symlink(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(Error::new(
            ExitStatus::Config,
            format!("{label} must not be a symlink"),
        )),
        Ok(_) | Err(_) => Ok(()),
    }
}

fn validate_private_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to inspect {label}: {error}"),
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != effective_uid()
        || metadata.nlink() != 1
    {
        return Err(Error::new(
            ExitStatus::Config,
            format!("{label} must be an unlinked regular file owned by the effective user"),
        ));
    }
    Ok(())
}

fn validate_open_file(file: &fs::File, path: &Path, label: &str) -> Result<()> {
    let metadata = file.metadata().map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to inspect {label}: {error}"),
        )
    })?;
    if !metadata.is_file() || metadata.uid() != effective_uid() || metadata.nlink() != 1 {
        return Err(Error::new(
            ExitStatus::Config,
            format!(
                "{label} must be an unlinked regular file owned by the effective user: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn add_authorized_keys_mount(config: &mut Map<String, Value>, source: &Path) -> Result<()> {
    if !config.get("process").is_some_and(Value::is_object) {
        return Err(Error::new(
            ExitStatus::DataErr,
            "OCI configuration must contain a process object",
        ));
    }
    let source = source
        .to_str()
        .ok_or_else(|| Error::new(ExitStatus::Config, "SSH state path must be valid UTF-8"))?;
    let mounts_value = match config.entry("mounts".to_owned()) {
        serde_json::map::Entry::Vacant(entry) => entry.insert(Value::Array(Vec::new())),
        serde_json::map::Entry::Occupied(entry) => {
            let value = entry.into_mut();
            if value.is_null() {
                *value = Value::Array(Vec::new());
            }
            value
        }
    };
    let mounts = mounts_value.as_array_mut().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            "OCI configuration mounts must be an array",
        )
    })?;

    for mount in mounts.iter().filter_map(Value::as_object) {
        if mount.get("destination").and_then(Value::as_str) == Some(DESTINATION)
            && !(mount.get("source").and_then(Value::as_str) == Some(source)
                && mount.get("type").and_then(Value::as_str) == Some("bind"))
        {
            return Err(Error::new(
                ExitStatus::Config,
                format!("conflicting mount already owns {DESTINATION}"),
            ));
        }
    }
    mounts.retain(|mount| {
        mount
            .as_object()
            .and_then(|mount| mount.get("destination"))
            .and_then(Value::as_str)
            != Some(DESTINATION)
    });
    mounts.push(json!({
        "destination": DESTINATION,
        "type": "bind",
        "source": source,
        "options": ["bind", "ro", "nosuid", "nodev", "noexec"],
    }));
    Ok(())
}

struct Lock(PathBuf);

impl Lock {
    fn acquire(state: &Path) -> Result<Self> {
        let path = state.join(".lock");
        fs::create_dir(&path).map_err(|error| {
            Error::new(
                ExitStatus::Unavailable,
                format!(
                    "SSH preparation is already running (or {} remains): {error}",
                    path.display()
                ),
            )
        })?;
        Ok(Self(path))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

fn nonce() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(|error| {
            Error::new(
                ExitStatus::Software,
                format!("clock before Unix epoch: {error}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_an_equivalent_mount() {
        let source = Path::new("/run/user/1000/sarus-ssh/authorized_keys");
        let mut config = json!({
            "process": {},
            "mounts": [{"destination": DESTINATION, "type": "bind", "source": source}],
        });
        add_authorized_keys_mount(config.as_object_mut().unwrap(), source).unwrap();
        let mounts = config["mounts"].as_array().unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(
            mounts[0]["options"],
            json!(["bind", "ro", "nosuid", "nodev", "noexec"])
        );
    }

    #[test]
    fn rejects_a_conflicting_mount() {
        let source = Path::new("/run/user/1000/sarus-ssh/authorized_keys");
        let mut config = json!({
            "process": {},
            "mounts": [{"destination": DESTINATION, "type": "bind", "source": "/other"}],
        });
        assert!(add_authorized_keys_mount(config.as_object_mut().unwrap(), source).is_err());
    }

    #[test]
    fn uses_a_per_uid_tmp_state_directory() {
        assert_eq!(
            state_directory_path_for_uid(23_961),
            PathBuf::from("/tmp/sarus-hook-23961")
        );
    }

    #[test]
    fn maps_namespace_root_to_its_host_uid() {
        assert_eq!(
            host_uid_from_map(0, "0 23961 1\n1 100000 65536\n").unwrap(),
            23_961
        );
    }

    #[test]
    fn maps_a_uid_inside_a_larger_range() {
        assert_eq!(host_uid_from_map(42, "0 100000 65536\n").unwrap(), 100_042);
    }
}
