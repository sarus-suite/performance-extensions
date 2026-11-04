use std::{
    env,
    ffi::{CStr, OsStr},
    fmt, fs,
    io::{self, Read, Seek, SeekFrom, Write},
    mem::MaybeUninit,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{self, Command},
    ptr,
    time::{SystemTime, UNIX_EPOCH},
};

use precreate_hook_diagnostics::{write_error, ExitStatus};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

const USER_RUNTIME_ROOT: &str = "/run/user";
const STATE_DIR_NAME: &str = "sarus-hook";
const AUTHORIZED_KEYS_NAME: &str = "authorized_keys";
const DESTINATION: &str = "/etc/ssh/hpc-dev-authorized_keys";
const SSHD_CONFIG_NAME: &str = "sshd_config.podman";
const SSHD_CONFIG_DESTINATION: &str = "/etc/ssh/sshd_config.podman";
const SSHD_LAUNCHER_NAME: &str = "hpc-dev-sshd";
const SSHD_LAUNCHER_DESTINATION: &str = "/usr/local/bin/hpc-dev-sshd";
const SSHD_BUNDLE_PREFIX: &str = "hpc-sshd";
const SSHD_BUNDLE_DESTINATION: &str = "/usr/local/libexec/hpc-sshd";
const SSHD_RUNTIME_NAME: &str = "hpc-dev-ssh";
const SSHD_RUNTIME_DESTINATION: &str = "/run/hpc-dev-ssh";
const CONTAINER_SSH_PROXY_NAME: &str = "container-ssh-proxy";
const PROXY_INSTALL_ANNOTATION: &str = "ssh.proxy.install";
const PROXY_MANAGED_HEADER: &[u8] =
    b"#!/usr/bin/env python3\n# managed-by: performance-extensions\n";
const DIRECTORY_MODE: u32 = 0o700;
const BUNDLE_DIRECTORY_MODE: u32 = 0o755;
const FILE_MODE: u32 = 0o600;
const CONFIG_MODE: u32 = 0o644;
const EXECUTABLE_MODE: u32 = 0o755;
const AUTHORIZED_KEY_ANNOTATION: &str = "ssh.authorized_key";
const SSHD_CONFIG: &[u8] = include_bytes!("../assets/sshd_config.podman");
const SSHD_LAUNCHER: &[u8] = include_bytes!("../assets/hpc-dev-sshd");
const CONTAINER_SSH_PROXY: &[u8] = include_bytes!("../assets/container-ssh-proxy");
const SSHD_BUNDLE: [(&str, &[u8]); 4] = [
    ("sshd", include_bytes!("../assets/sshd")),
    ("sshd-auth", include_bytes!("../assets/sshd-auth")),
    ("sshd-session", include_bytes!("../assets/sshd-session")),
    ("ssh-keygen", include_bytes!("../assets/ssh-keygen")),
];

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProxyInstallPolicy {
    Auto,
    Required,
    Never,
}

fn run() -> Result<()> {
    let mut config = read_stdin_json()?;
    let config_object = config.as_object_mut().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            "OCI configuration must be a JSON object",
        )
    })?;
    let proxy_install_policy = annotation_proxy_install_policy(config_object)?;
    let authorized_key = annotation_authorized_key(config_object)?;

    let state = state_directory()?;
    let _lock = Lock::acquire(&state)?;
    install_container_ssh_proxy(proxy_install_policy)?;
    let authorized_keys = write_annotated_authorized_key(&state, &authorized_key)?;
    let (sshd_config, sshd_launcher, sshd_bundle) = ensure_sshd_assets(&state)?;
    let sshd_runtime = ensure_sshd_runtime(&state)?;
    add_ssh_mounts(
        config_object,
        &authorized_keys,
        &sshd_config,
        &sshd_launcher,
        &sshd_bundle,
        &sshd_runtime,
    )?;
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
    let user_runtime = state
        .parent()
        .expect("state directory always has a per-user runtime parent");
    validate_directory(user_runtime, "per-user runtime directory")?;
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
    PathBuf::from(USER_RUNTIME_ROOT)
        .join(uid.to_string())
        .join(STATE_DIR_NAME)
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
            format!(
                "{label} {} is not an accessible directory: {error}",
                path.display()
            ),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != effective_uid()
    {
        return Err(Error::new(
            ExitStatus::Config,
            format!(
                "{label} {} must be a real directory owned by the effective user",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn install_container_ssh_proxy(policy: ProxyInstallPolicy) -> Result<()> {
    if policy == ProxyInstallPolicy::Never {
        return Ok(());
    }

    match ensure_container_ssh_proxy() {
        Ok(_) => Ok(()),
        Err(error) if policy == ProxyInstallPolicy::Auto => {
            eprintln!(
                "ssh_precreate_hook: warning: could not install {CONTAINER_SSH_PROXY_NAME}: {error}"
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn ensure_container_ssh_proxy() -> Result<PathBuf> {
    let host_uid = state_owner_uid()?;
    if host_uid == 0 {
        return Err(Error::new(
            ExitStatus::Config,
            format!(
                "refusing to install {CONTAINER_SSH_PROXY_NAME} for host UID 0; use rootless Podman or set {PROXY_INSTALL_ANNOTATION}=never"
            ),
        ));
    }

    let home = home_directory_for_uid(host_uid)?;
    install_container_ssh_proxy_in(&home)
}

fn home_directory_for_uid(uid: u32) -> Result<PathBuf> {
    match passwd_home_directory(uid)? {
        Some(home) => validate_home_directory(home),
        None => {
            let home = env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
                Error::new(
                    ExitStatus::Unavailable,
                    format!(
                        "cannot resolve a home directory for host UID {uid}; set {PROXY_INSTALL_ANNOTATION}=never or provide HOME"
                    ),
                )
            })?;
            validate_home_directory(home)
        }
    }
}

fn passwd_home_directory(uid: u32) -> Result<Option<PathBuf>> {
    let configured_size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut buffer_size = if configured_size > 0 {
        configured_size as usize
    } else {
        16 * 1024
    };
    buffer_size = buffer_size.max(1024);

    loop {
        let mut passwd = MaybeUninit::<libc::passwd>::uninit();
        let mut result = ptr::null_mut();
        let mut buffer = vec![0_u8; buffer_size];
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                passwd.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE {
            buffer_size = buffer_size.checked_mul(2).ok_or_else(|| {
                Error::new(
                    ExitStatus::Software,
                    "password-database buffer size overflow",
                )
            })?;
            if buffer_size > 1024 * 1024 {
                return Err(Error::new(
                    ExitStatus::Unavailable,
                    "password-database entry exceeds 1 MiB",
                ));
            }
            continue;
        }
        if status != 0 {
            return Err(Error::new(
                ExitStatus::IoErr,
                format!(
                    "failed to resolve host UID {uid}: {}",
                    io::Error::from_raw_os_error(status)
                ),
            ));
        }
        if result.is_null() {
            return Ok(None);
        }

        let passwd = unsafe { passwd.assume_init() };
        if passwd.pw_dir.is_null() {
            return Ok(None);
        }
        let bytes = unsafe { CStr::from_ptr(passwd.pw_dir) }.to_bytes();
        if bytes.is_empty() {
            return Ok(None);
        }
        return Ok(Some(PathBuf::from(OsStr::from_bytes(bytes))));
    }
}

fn validate_home_directory(home: PathBuf) -> Result<PathBuf> {
    if !home.is_absolute() {
        return Err(Error::new(
            ExitStatus::Config,
            format!("user home must be absolute: {}", home.display()),
        ));
    }
    validate_directory(&home, "user home")?;
    Ok(home)
}

fn install_container_ssh_proxy_in(home: &Path) -> Result<PathBuf> {
    validate_directory(home, "user home")?;
    let local = home.join(".local");
    ensure_user_directory(&local, ".local directory")?;
    let bin = local.join("bin");
    ensure_user_directory(&bin, ".local/bin directory")?;

    let destination = bin.join(CONTAINER_SSH_PROXY_NAME);
    let replace = match read_installed_proxy(&destination)? {
        None => true,
        Some(contents) if contents == CONTAINER_SSH_PROXY => {
            fs::set_permissions(&destination, fs::Permissions::from_mode(EXECUTABLE_MODE))
                .map_err(|error| {
                    Error::new(
                        ExitStatus::IoErr,
                        format!(
                            "failed to set permissions on {}: {error}",
                            destination.display()
                        ),
                    )
                })?;
            false
        }
        Some(contents) if contents.starts_with(PROXY_MANAGED_HEADER) => true,
        Some(_) => {
            return Err(Error::new(
                ExitStatus::Config,
                format!(
                    "{} exists but is not managed by performance-extensions; leaving it unchanged",
                    destination.display()
                ),
            ));
        }
    };

    if replace {
        install_proxy_atomically(&bin, &destination)?;
    }
    Ok(destination)
}

fn ensure_user_directory(path: &Path, label: &str) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {
            fs::set_permissions(path, fs::Permissions::from_mode(BUNDLE_DIRECTORY_MODE)).map_err(
                |error| {
                    Error::new(
                        ExitStatus::IoErr,
                        format!("failed to set permissions on {label}: {error}"),
                    )
                },
            )?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(Error::new(
                ExitStatus::IoErr,
                format!("failed to create {label} {}: {error}", path.display()),
            ));
        }
    }
    validate_directory(path, label)
}

fn read_installed_proxy(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(Error::new(
                ExitStatus::Config,
                format!("{} must not be a symlink", path.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Error::new(
                ExitStatus::IoErr,
                format!("failed to inspect {}: {error}", path.display()),
            ));
        }
    }

    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to open {}: {error}", path.display()),
            )
        })?;
    validate_open_file(&file, path, CONTAINER_SSH_PROXY_NAME)?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to read {}: {error}", path.display()),
        )
    })?;
    Ok(Some(contents))
}

fn install_proxy_atomically(directory: &Path, destination: &Path) -> Result<()> {
    let temporary = directory.join(format!(
        ".{CONTAINER_SSH_PROXY_NAME}.{}-{}.tmp",
        process::id(),
        nonce()?
    ));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(|error| {
                Error::new(
                    ExitStatus::IoErr,
                    format!("failed to create temporary {CONTAINER_SSH_PROXY_NAME}: {error}"),
                )
            })?;
        file.write_all(CONTAINER_SSH_PROXY).map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to write {CONTAINER_SSH_PROXY_NAME}: {error}"),
            )
        })?;
        file.set_permissions(fs::Permissions::from_mode(EXECUTABLE_MODE))
            .map_err(|error| {
                Error::new(
                    ExitStatus::IoErr,
                    format!("failed to make {CONTAINER_SSH_PROXY_NAME} executable: {error}"),
                )
            })?;
        file.sync_all().map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to sync {CONTAINER_SSH_PROXY_NAME}: {error}"),
            )
        })?;
        drop(file);

        fs::rename(&temporary, destination).map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to install {}: {error}", destination.display()),
            )
        })?;
        fs::File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                Error::new(
                    ExitStatus::IoErr,
                    format!("failed to sync {}: {error}", directory.display()),
                )
            })?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn annotation_proxy_install_policy(config: &Map<String, Value>) -> Result<ProxyInstallPolicy> {
    let Some(annotations) = config.get("annotations") else {
        return Ok(ProxyInstallPolicy::Auto);
    };
    let annotations = annotations.as_object().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            "OCI configuration annotations must be an object",
        )
    })?;
    let Some(policy) = annotations.get(PROXY_INSTALL_ANNOTATION) else {
        return Ok(ProxyInstallPolicy::Auto);
    };
    let policy = policy.as_str().ok_or_else(|| {
        Error::new(
            ExitStatus::Config,
            format!("{PROXY_INSTALL_ANNOTATION} must be a string"),
        )
    })?;
    match policy {
        "auto" => Ok(ProxyInstallPolicy::Auto),
        "required" => Ok(ProxyInstallPolicy::Required),
        "never" => Ok(ProxyInstallPolicy::Never),
        _ => Err(Error::new(
            ExitStatus::Config,
            format!("{PROXY_INSTALL_ANNOTATION} must be one of auto, required, or never"),
        )),
    }
}

fn annotation_authorized_key(config: &Map<String, Value>) -> Result<String> {
    let Some(annotations) = config.get("annotations") else {
        return Err(Error::new(
            ExitStatus::Config,
            format!("missing required {AUTHORIZED_KEY_ANNOTATION} annotation"),
        ));
    };
    let annotations = annotations.as_object().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            "OCI configuration annotations must be an object",
        )
    })?;
    let Some(key) = annotations.get(AUTHORIZED_KEY_ANNOTATION) else {
        return Err(Error::new(
            ExitStatus::Config,
            format!("missing required {AUTHORIZED_KEY_ANNOTATION} annotation"),
        ));
    };
    let key = key.as_str().ok_or_else(|| {
        Error::new(
            ExitStatus::Config,
            format!("{AUTHORIZED_KEY_ANNOTATION} must be a string"),
        )
    })?;
    if key.contains(['\n', '\r', '\0']) {
        return Err(Error::new(
            ExitStatus::Config,
            format!("{AUTHORIZED_KEY_ANNOTATION} must contain exactly one public key"),
        ));
    }
    let key = key.trim();
    if key.is_empty() {
        return Err(Error::new(
            ExitStatus::Config,
            format!("{AUTHORIZED_KEY_ANNOTATION} must not be empty"),
        ));
    }
    Ok(key.to_owned())
}

fn write_annotated_authorized_key(state: &Path, key: &str) -> Result<PathBuf> {
    validate_annotated_public_key(state, key)?;
    let digest = Sha256::digest(key.as_bytes());
    let name = format!("{AUTHORIZED_KEYS_NAME}.{:x}", digest);
    let path = state.join(name);
    write_authorized_keys_file(&path, key.as_bytes())?;
    Ok(path)
}

fn validate_annotated_public_key(state: &Path, key: &str) -> Result<()> {
    let path = state.join(format!(".validate-key-{}-{}", process::id(), nonce()?));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
        .and_then(|mut file| file.write_all(key.as_bytes()))
        .map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to stage {AUTHORIZED_KEY_ANNOTATION}: {error}"),
            )
        })?;

    let result = Command::new("ssh-keygen")
        .args(["-l", "-f"])
        .arg(&path)
        .output();
    let _ = fs::remove_file(&path);
    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(_) => Err(Error::new(
            ExitStatus::Config,
            format!("{AUTHORIZED_KEY_ANNOTATION} is not a valid OpenSSH public key"),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(Error::new(
            ExitStatus::Unavailable,
            "ssh-keygen is required on the host",
        )),
        Err(error) => Err(Error::new(
            ExitStatus::IoErr,
            format!("failed to execute ssh-keygen: {error}"),
        )),
    }
}

fn ensure_sshd_assets(state: &Path) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let config = state.join(SSHD_CONFIG_NAME);
    write_state_file(
        &config,
        SSHD_CONFIG,
        CONFIG_MODE,
        "sshd configuration",
        false,
    )?;

    let launcher = state.join(SSHD_LAUNCHER_NAME);
    write_state_file(
        &launcher,
        SSHD_LAUNCHER,
        EXECUTABLE_MODE,
        "sshd launcher",
        false,
    )?;
    let bundle = ensure_sshd_bundle(state)?;
    Ok((config, launcher, bundle))
}

fn ensure_sshd_runtime(state: &Path) -> Result<PathBuf> {
    let runtime = state.join(SSHD_RUNTIME_NAME);
    match fs::create_dir(&runtime) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(Error::new(
                ExitStatus::IoErr,
                format!(
                    "failed to create sshd runtime directory {}: {error}",
                    runtime.display()
                ),
            ));
        }
    }
    validate_directory(&runtime, "sshd runtime directory")?;
    fs::set_permissions(&runtime, fs::Permissions::from_mode(DIRECTORY_MODE)).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to set sshd runtime directory permissions: {error}"),
        )
    })?;
    Ok(runtime)
}

fn ensure_sshd_bundle(state: &Path) -> Result<PathBuf> {
    let bundle = state.join(sshd_bundle_name());
    match fs::symlink_metadata(&bundle) {
        Ok(_) => {
            validate_directory(&bundle, "sshd bundle")?;
            validate_sshd_bundle(&bundle)?;
            return Ok(bundle);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(Error::new(
                ExitStatus::IoErr,
                format!(
                    "failed to inspect sshd bundle {}: {error}",
                    bundle.display()
                ),
            ));
        }
    }

    let work = state.join(format!(".prepare-sshd-{}-{}", process::id(), nonce()?));
    fs::create_dir(&work).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to create sshd bundle directory: {error}"),
        )
    })?;
    fs::set_permissions(&work, fs::Permissions::from_mode(BUNDLE_DIRECTORY_MODE)).map_err(
        |error| {
            let _ = fs::remove_dir_all(&work);
            Error::new(
                ExitStatus::IoErr,
                format!("failed to set sshd bundle directory permissions: {error}"),
            )
        },
    )?;

    for (name, contents) in SSHD_BUNDLE {
        if let Err(error) = write_state_file(
            &work.join(name),
            contents,
            EXECUTABLE_MODE,
            "sshd bundle executable",
            false,
        ) {
            let _ = fs::remove_dir_all(&work);
            return Err(error);
        }
    }

    if let Err(error) = fs::rename(&work, &bundle) {
        let _ = fs::remove_dir_all(&work);
        return Err(Error::new(
            ExitStatus::IoErr,
            format!(
                "failed to install sshd bundle {}: {error}",
                bundle.display()
            ),
        ));
    }
    validate_sshd_bundle(&bundle)?;
    Ok(bundle)
}

fn sshd_bundle_name() -> String {
    let mut digest = Sha256::new();
    for (name, contents) in SSHD_BUNDLE {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((contents.len() as u64).to_le_bytes());
        digest.update(contents);
    }
    format!("{SSHD_BUNDLE_PREFIX}-{:x}", digest.finalize())
}

fn validate_sshd_bundle(bundle: &Path) -> Result<()> {
    for (name, expected) in SSHD_BUNDLE {
        let path = bundle.join(name);
        validate_not_symlink(&path, "sshd bundle executable")?;
        let mut file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|error| {
                Error::new(
                    ExitStatus::IoErr,
                    format!(
                        "failed to open sshd bundle executable {}: {error}",
                        path.display()
                    ),
                )
            })?;
        validate_open_file(&file, &path, "sshd bundle executable")?;
        let mode = file
            .metadata()
            .map_err(|error| {
                Error::new(
                    ExitStatus::IoErr,
                    format!("failed to inspect sshd bundle executable: {error}"),
                )
            })?
            .permissions()
            .mode()
            & 0o777;
        if mode != EXECUTABLE_MODE {
            return Err(Error::new(
                ExitStatus::Config,
                format!(
                    "sshd bundle executable has unsafe permissions: {}",
                    path.display()
                ),
            ));
        }
        let mut actual = Vec::with_capacity(expected.len());
        file.read_to_end(&mut actual).map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!(
                    "failed to read sshd bundle executable {}: {error}",
                    path.display()
                ),
            )
        })?;
        if actual != expected {
            return Err(Error::new(
                ExitStatus::Config,
                format!(
                    "sshd bundle executable does not match embedded asset: {}",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn write_authorized_keys_file(authorized_keys: &Path, contents: &[u8]) -> Result<()> {
    write_state_file(
        authorized_keys,
        contents,
        FILE_MODE,
        "authorized_keys",
        true,
    )
}

fn write_state_file(
    path: &Path,
    contents: &[u8],
    mode: u32,
    label: &str,
    append_newline: bool,
) -> Result<()> {
    validate_not_symlink(path, label)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .mode(mode)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to open {label} {}: {error}", path.display()),
            )
        })?;
    validate_open_file(&file, path, label)?;
    file.set_len(0).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to truncate {label}: {error}"),
        )
    })?;
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to seek {label}: {error}"),
        )
    })?;
    file.write_all(contents).map_err(|error| {
        Error::new(
            ExitStatus::IoErr,
            format!("failed to write {label}: {error}"),
        )
    })?;
    if append_newline && !contents.ends_with(b"\n") {
        file.write_all(b"\n").map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to finish {label}: {error}"),
            )
        })?;
    }
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|error| {
            Error::new(
                ExitStatus::IoErr,
                format!("failed to protect {label}: {error}"),
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

struct MountSpec<'a> {
    source: &'a Path,
    destination: &'static str,
    options: &'static [&'static str],
}

fn add_ssh_mounts(
    config: &mut Map<String, Value>,
    authorized_keys: &Path,
    sshd_config: &Path,
    sshd_launcher: &Path,
    sshd_bundle: &Path,
    sshd_runtime: &Path,
) -> Result<()> {
    let specs = [
        MountSpec {
            source: authorized_keys,
            destination: DESTINATION,
            options: &["bind", "ro", "nosuid", "nodev", "noexec"],
        },
        MountSpec {
            source: sshd_config,
            destination: SSHD_CONFIG_DESTINATION,
            options: &["bind", "ro", "nosuid", "nodev", "noexec"],
        },
        MountSpec {
            source: sshd_launcher,
            destination: SSHD_LAUNCHER_DESTINATION,
            options: &["bind", "ro", "nosuid", "nodev"],
        },
        MountSpec {
            source: sshd_bundle,
            destination: SSHD_BUNDLE_DESTINATION,
            options: &["bind", "ro", "nosuid", "nodev"],
        },
        MountSpec {
            source: sshd_runtime,
            destination: SSHD_RUNTIME_DESTINATION,
            options: &["bind", "rw", "nosuid", "nodev", "noexec"],
        },
    ];
    add_mounts(config, &specs)
}

fn add_mounts(config: &mut Map<String, Value>, specs: &[MountSpec<'_>]) -> Result<()> {
    if !config.get("process").is_some_and(Value::is_object) {
        return Err(Error::new(
            ExitStatus::DataErr,
            "OCI configuration must contain a process object",
        ));
    }
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

    for spec in specs {
        let source = spec
            .source
            .to_str()
            .ok_or_else(|| Error::new(ExitStatus::Config, "SSH state path must be valid UTF-8"))?;
        for mount in mounts.iter().filter_map(Value::as_object) {
            if mount.get("destination").and_then(Value::as_str) == Some(spec.destination)
                && !(mount.get("source").and_then(Value::as_str) == Some(source)
                    && mount.get("type").and_then(Value::as_str) == Some("bind"))
            {
                return Err(Error::new(
                    ExitStatus::Config,
                    format!("conflicting mount already owns {}", spec.destination),
                ));
            }
        }
    }
    mounts.retain(|mount| {
        mount
            .as_object()
            .and_then(|mount| mount.get("destination"))
            .and_then(Value::as_str)
            .map_or(true, |destination| {
                !specs.iter().any(|spec| spec.destination == destination)
            })
    });
    for spec in specs {
        let source = spec.source.to_str().expect("validated above");
        mounts.push(json!({
            "destination": spec.destination,
            "type": "bind",
            "source": source,
            "options": spec.options,
        }));
    }
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
    use std::os::unix::fs::symlink;

    fn temp_home(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ssh-precreate-hook-{label}-{}-{}",
            process::id(),
            nonce().unwrap()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn defaults_to_auto_proxy_installation() {
        let config = json!({});
        assert_eq!(
            annotation_proxy_install_policy(config.as_object().unwrap()).unwrap(),
            ProxyInstallPolicy::Auto
        );
    }

    #[test]
    fn reads_proxy_installation_policy() {
        for (value, expected) in [
            ("auto", ProxyInstallPolicy::Auto),
            ("required", ProxyInstallPolicy::Required),
            ("never", ProxyInstallPolicy::Never),
        ] {
            let config = json!({
                "annotations": { PROXY_INSTALL_ANNOTATION: value }
            });
            assert_eq!(
                annotation_proxy_install_policy(config.as_object().unwrap()).unwrap(),
                expected
            );
        }

        let config = json!({
            "annotations": { PROXY_INSTALL_ANNOTATION: "sometimes" }
        });
        assert!(annotation_proxy_install_policy(config.as_object().unwrap()).is_err());
    }

    #[test]
    fn installs_and_reuses_the_embedded_proxy() {
        let home = temp_home("proxy-install");
        let first = install_container_ssh_proxy_in(&home).unwrap();
        let second = install_container_ssh_proxy_in(&home).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(&first).unwrap(), CONTAINER_SSH_PROXY);
        assert_eq!(
            fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            EXECUTABLE_MODE
        );

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn upgrades_a_managed_proxy() {
        let home = temp_home("proxy-upgrade");
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let proxy = bin.join(CONTAINER_SSH_PROXY_NAME);
        fs::write(
            &proxy,
            b"#!/usr/bin/env python3\n# managed-by: performance-extensions\n# version: old\n",
        )
        .unwrap();

        install_container_ssh_proxy_in(&home).unwrap();
        assert_eq!(fs::read(&proxy).unwrap(), CONTAINER_SSH_PROXY);

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn preserves_an_unmanaged_proxy() {
        let home = temp_home("proxy-unmanaged");
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let proxy = bin.join(CONTAINER_SSH_PROXY_NAME);
        let original = b"#!/bin/sh\necho user-owned\n";
        fs::write(&proxy, original).unwrap();

        let error = install_container_ssh_proxy_in(&home).unwrap_err();
        assert_eq!(error.exit_status(), ExitStatus::Config);
        assert_eq!(fs::read(&proxy).unwrap(), original);

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn rejects_a_symlinked_proxy_destination() {
        let home = temp_home("proxy-symlink");
        let bin = home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let target = home.join("target");
        fs::write(&target, b"do not replace\n").unwrap();
        symlink(&target, bin.join(CONTAINER_SSH_PROXY_NAME)).unwrap();

        assert!(install_container_ssh_proxy_in(&home).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"do not replace\n");

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn replaces_an_equivalent_mount() {
        let source = Path::new("/run/user/1000/sarus-ssh/authorized_keys");
        let mut config = json!({
            "process": {},
            "mounts": [{"destination": DESTINATION, "type": "bind", "source": source}],
        });
        add_mounts(
            config.as_object_mut().unwrap(),
            &[MountSpec {
                source,
                destination: DESTINATION,
                options: &["bind", "ro", "nosuid", "nodev", "noexec"],
            }],
        )
        .unwrap();
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
        assert!(add_mounts(
            config.as_object_mut().unwrap(),
            &[MountSpec {
                source,
                destination: DESTINATION,
                options: &["bind", "ro", "nosuid", "nodev", "noexec"],
            }],
        )
        .is_err());
    }

    #[test]
    fn uses_the_per_uid_runtime_directory() {
        assert_eq!(
            state_directory_path_for_uid(23_961),
            PathBuf::from("/run/user/23961/sarus-hook")
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

    #[test]
    fn reads_a_single_authorized_key_annotation() {
        let config = json!({
            "annotations": {
                AUTHORIZED_KEY_ANNOTATION: " ssh-ed25519 AAAA example "
            }
        });
        assert_eq!(
            annotation_authorized_key(config.as_object().unwrap()).unwrap(),
            "ssh-ed25519 AAAA example"
        );
    }

    #[test]
    fn requires_an_authorized_key_annotation() {
        let config = json!({ "annotations": {} });
        let error = annotation_authorized_key(config.as_object().unwrap()).unwrap_err();

        assert_eq!(error.exit_status(), ExitStatus::Config);
        assert!(error.to_string().contains(AUTHORIZED_KEY_ANNOTATION));
    }

    #[test]
    fn rejects_multiline_authorized_key_annotation() {
        let config = json!({
            "annotations": { AUTHORIZED_KEY_ANNOTATION: "key-one\nkey-two" }
        });
        assert!(annotation_authorized_key(config.as_object().unwrap()).is_err());
    }

    #[test]
    fn adds_executable_sshd_launcher_mount() {
        let authorized_keys = Path::new("/run/user/23961/sarus-hook/authorized_keys");
        let sshd_config = Path::new("/run/user/23961/sarus-hook/sshd_config.podman");
        let sshd_launcher = Path::new("/run/user/23961/sarus-hook/hpc-dev-sshd");
        let sshd_bundle = Path::new("/run/user/23961/sarus-hook/hpc-sshd-digest");
        let sshd_runtime = Path::new("/run/user/23961/sarus-hook/hpc-dev-ssh");
        let mut config = json!({ "process": {}, "mounts": [] });

        add_ssh_mounts(
            config.as_object_mut().unwrap(),
            authorized_keys,
            sshd_config,
            sshd_launcher,
            sshd_bundle,
            sshd_runtime,
        )
        .unwrap();

        let launcher = config["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|mount| mount["destination"] == SSHD_LAUNCHER_DESTINATION)
            .unwrap();
        assert_eq!(
            launcher["options"],
            json!(["bind", "ro", "nosuid", "nodev"])
        );

        let bundle = config["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|mount| mount["destination"] == SSHD_BUNDLE_DESTINATION)
            .unwrap();
        assert_eq!(bundle["source"], sshd_bundle.to_str().unwrap());
        assert_eq!(bundle["options"], json!(["bind", "ro", "nosuid", "nodev"]));

        let runtime = config["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|mount| mount["destination"] == SSHD_RUNTIME_DESTINATION)
            .unwrap();
        assert_eq!(runtime["source"], sshd_runtime.to_str().unwrap());
        assert_eq!(
            runtime["options"],
            json!(["bind", "rw", "nosuid", "nodev", "noexec"])
        );
    }

    #[test]
    fn versions_the_sshd_bundle_by_its_contents() {
        let name = sshd_bundle_name();
        assert!(name.starts_with("hpc-sshd-"));
        assert_eq!(name.len(), "hpc-sshd-".len() + 64);
    }

    #[test]
    fn stages_and_reuses_the_embedded_sshd_bundle() {
        let state = std::env::temp_dir().join(format!(
            "ssh-precreate-hook-test-{}-{}",
            process::id(),
            nonce().unwrap()
        ));
        fs::create_dir(&state).unwrap();

        let first = ensure_sshd_bundle(&state).unwrap();
        let second = ensure_sshd_bundle(&state).unwrap();
        assert_eq!(first, second);
        for (name, _) in SSHD_BUNDLE {
            assert!(first.join(name).is_file());
        }

        fs::remove_dir_all(state).unwrap();
    }
}
