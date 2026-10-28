use chrono::{SecondsFormat, Utc};
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const FALLBACK_LOG_ROOT: &str = "/tmp";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const MAX_ESCAPED_MESSAGE_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum ExitStatus {
    Usage = 64,
    DataErr = 65,
    NoInput = 66,
    Unavailable = 69,
    Software = 70,
    IoErr = 74,
    Config = 78,
}

impl ExitStatus {
    pub const fn code(self) -> i32 {
        self as i32
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Usage => "EX_USAGE",
            Self::DataErr => "EX_DATAERR",
            Self::NoInput => "EX_NOINPUT",
            Self::Unavailable => "EX_UNAVAILABLE",
            Self::Software => "EX_SOFTWARE",
            Self::IoErr => "EX_IOERR",
            Self::Config => "EX_CONFIG",
        }
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.label(), self.code())
    }
}

pub fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

pub fn write_error(hook_name: &str, status: ExitStatus, message: &str) -> io::Result<PathBuf> {
    let uid = effective_uid();
    write_error_in(
        &log_root(uid),
        uid,
        std::process::id(),
        hook_name,
        status,
        message,
    )
}

fn log_root(uid: u32) -> PathBuf {
    log_root_from(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from), uid)
}

fn log_root_from(runtime_dir: Option<PathBuf>, uid: u32) -> PathBuf {
    let runtime_path = runtime_dir.unwrap_or_default();
    let is_absolute_path = runtime_path.is_absolute();

    let is_valid_runtime_directory = fs::symlink_metadata(&runtime_path).is_ok_and(|metadata| {
        metadata.is_dir() && !metadata.file_type().is_symlink() && metadata.uid() == uid
    });

    if is_absolute_path && is_valid_runtime_directory {
        runtime_path
    } else {
        PathBuf::from(FALLBACK_LOG_ROOT)
    }
}

fn error_log_path_in(root: &Path, uid: u32, hook_name: &str) -> PathBuf {
    root.join(format!("standard-hooks-{uid}"))
        .join(format!("{hook_name}.log"))
}

fn write_error_in(
    root: &Path,
    uid: u32,
    pid: u32,
    hook_name: &str,
    status: ExitStatus,
    message: &str,
) -> io::Result<PathBuf> {
    validate_hook_name(hook_name)?;

    let path = error_log_path_in(root, uid, hook_name);

    let directory = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("error log path has no parent: {}", path.display()),
        )
    })?;
    create_or_validate_directory(directory, uid)?;

    let mut file = open_and_validate_log(&path, uid)?;
    let escaped = escape_message(message);
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let record = format!(
        "timestamp={timestamp} hook={hook_name} uid={uid} pid={pid} status={} category={} message=\"{escaped}\"\n",
        status.code(),
        status.label()
    );
    file.write_all(record.as_bytes())?;
    file.flush()?;
    Ok(path)
}

fn validate_hook_name(hook_name: &str) -> io::Result<()> {
    if hook_name.is_empty()
        || !hook_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid standard hook name: {hook_name:?}"),
        ));
    }
    Ok(())
}

fn create_or_validate_directory(path: &Path, uid: u32) -> io::Result<()> {
    match DirBuilder::new().mode(DIRECTORY_MODE).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other(format!(
            "diagnostic path is not a real directory: {}",
            path.display()
        )));
    }
    validate_owner_and_mode(path, &metadata, uid, DIRECTORY_MODE)
}

fn open_and_validate_log(path: &Path, uid: u32) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(FILE_MODE)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "diagnostic log is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.nlink() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "diagnostic log has {} hard links, expected 1: {}",
                metadata.nlink(),
                path.display()
            ),
        ));
    }
    validate_owner_and_mode(path, &metadata, uid, FILE_MODE)?;
    Ok(file)
}

fn validate_owner_and_mode(
    path: &Path,
    metadata: &fs::Metadata,
    uid: u32,
    expected_mode: u32,
) -> io::Result<()> {
    if metadata.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is owned by uid {}, expected uid {uid}",
                path.display(),
                metadata.uid()
            ),
        ));
    }

    let actual_mode = metadata.permissions().mode() & 0o777;
    if actual_mode != expected_mode {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} has mode {actual_mode:#o}, expected {expected_mode:#o}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn escape_message(message: &str) -> String {
    let mut output = String::new();
    let mut truncated = false;

    for character in message.chars() {
        let escaped = match character {
            '\\' => "\\\\".to_string(),
            '"' => "\\\"".to_string(),
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            control if control.is_control() => format!("\\u{{{:x}}}", control as u32),
            other => other.to_string(),
        };

        if output.len() + escaped.len() > MAX_ESCAPED_MESSAGE_BYTES {
            truncated = true;
            break;
        }
        output.push_str(&escaped);
    }

    if truncated {
        output.push_str("...[truncated]");
    }
    output
}
