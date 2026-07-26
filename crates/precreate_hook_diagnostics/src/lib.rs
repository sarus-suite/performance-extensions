use chrono::{SecondsFormat, Utc};
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const LOG_ROOT: &str = "/tmp";
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

// TODO: this function is currently unused across this workspace, since the hooks call write_error(),
// which constructs the log path on its own. Does it make sense to keep this around?
pub fn error_log_path(hook_name: &str) -> PathBuf {
    error_log_path_in(Path::new(LOG_ROOT), effective_uid(), hook_name)
}

pub fn write_error(hook_name: &str, status: ExitStatus, message: &str) -> io::Result<PathBuf> {
    write_error_in(
        Path::new(LOG_ROOT),
        effective_uid(),
        std::process::id(),
        hook_name,
        status,
        message,
    )
}

fn error_log_path_in(root: &Path, uid: u32, hook_name: &str) -> PathBuf {
    root.join(format!("precreate-hooks-{uid}"))
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
            format!("invalid precreate hook name: {hook_name:?}"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "precreate-hook-diagnostics-test-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn appends_bounded_escaped_records() {
        let root = temp_root("append");
        fs::create_dir(&root).unwrap();
        let uid = effective_uid();

        let path = write_error_in(
            &root,
            uid,
            42,
            "pce_hook",
            ExitStatus::Config,
            "bad\n\"value\"",
        )
        .unwrap();
        write_error_in(
            &root,
            uid,
            43,
            "pce_hook",
            ExitStatus::DataErr,
            &"x".repeat(MAX_ESCAPED_MESSAGE_BYTES + 100),
        )
        .unwrap();

        let contents = fs::read_to_string(&path).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("status=78 category=EX_CONFIG"));
        assert!(lines[0].contains("message=\"bad\\n\\\"value\\\"\""));
        assert!(lines[1].contains("...[truncated]"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            FILE_MODE
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            DIRECTORY_MODE
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_symlinked_directory() {
        let root = temp_root("directory-symlink");
        let target = temp_root("directory-target");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&target).unwrap();
        symlink(
            &target,
            root.join(format!("precreate-hooks-{}", effective_uid())),
        )
        .unwrap();

        let error = write_error_in(
            &root,
            effective_uid(),
            1,
            "sethomevar",
            ExitStatus::DataErr,
            "bad input",
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a real directory"));

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn rejects_symlinked_log_file() {
        let root = temp_root("file-symlink");
        let uid = effective_uid();
        let directory = root.join(format!("precreate-hooks-{uid}"));
        fs::create_dir(&root).unwrap();
        DirBuilder::new()
            .mode(DIRECTORY_MODE)
            .create(&directory)
            .unwrap();
        symlink("/dev/null", directory.join("pc_injection_hook.log")).unwrap();

        let error = write_error_in(
            &root,
            uid,
            1,
            "pc_injection_hook",
            ExitStatus::Usage,
            "bad argument",
        )
        .unwrap_err();
        assert!(
            matches!(error.raw_os_error(), Some(libc::ELOOP) | Some(libc::EMLINK)),
            "{error}"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_unsafe_directory_permissions() {
        let root = temp_root("directory-mode");
        let uid = effective_uid();
        let directory = root.join(format!("precreate-hooks-{uid}"));
        fs::create_dir(&root).unwrap();
        DirBuilder::new().mode(0o755).create(&directory).unwrap();

        let error = write_error_in(&root, uid, 1, "pce_hook", ExitStatus::Config, "bad config")
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_non_directory_and_unsafe_file_permissions() {
        let uid = effective_uid();

        let non_directory_root = temp_root("not-directory");
        fs::create_dir(&non_directory_root).unwrap();
        fs::write(
            non_directory_root.join(format!("precreate-hooks-{uid}")),
            b"not a directory",
        )
        .unwrap();
        let error = write_error_in(
            &non_directory_root,
            uid,
            1,
            "pce_hook",
            ExitStatus::Config,
            "bad config",
        )
        .unwrap_err();
        assert!(error.to_string().contains("not a real directory"));
        fs::remove_dir_all(non_directory_root).unwrap();

        let unsafe_file_root = temp_root("unsafe-file");
        let directory = unsafe_file_root.join(format!("precreate-hooks-{uid}"));
        fs::create_dir(&unsafe_file_root).unwrap();
        DirBuilder::new()
            .mode(DIRECTORY_MODE)
            .create(&directory)
            .unwrap();
        let path = directory.join("pce_hook.log");
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o644)
            .open(&path)
            .unwrap();
        let error = write_error_in(
            &unsafe_file_root,
            uid,
            1,
            "pce_hook",
            ExitStatus::Config,
            "bad config",
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        fs::remove_dir_all(unsafe_file_root).unwrap();
    }

    #[test]
    fn rejects_wrong_owner_expectation() {
        let root = temp_root("wrong-owner");
        fs::create_dir(&root).unwrap();
        let metadata = fs::metadata(&root).unwrap();
        let unexpected_uid = metadata.uid().wrapping_add(1);
        let error =
            validate_owner_and_mode(&root, &metadata, unexpected_uid, metadata.mode() & 0o777)
                .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn concurrent_writers_preserve_all_records() {
        let root = temp_root("concurrent");
        fs::create_dir(&root).unwrap();
        let root = Arc::new(root);
        let barrier = Arc::new(Barrier::new(8));
        let mut writers = Vec::new();

        for index in 0..8 {
            let root = Arc::clone(&root);
            let barrier = Arc::clone(&barrier);
            writers.push(thread::spawn(move || {
                barrier.wait();
                write_error_in(
                    &root,
                    effective_uid(),
                    index,
                    "sethomevar",
                    ExitStatus::DataErr,
                    &format!("concurrent-error-{index}"),
                )
                .unwrap();
            }));
        }
        for writer in writers {
            writer.join().unwrap();
        }

        let path = error_log_path_in(&root, effective_uid(), "sethomevar");
        let contents = fs::read_to_string(path).unwrap();
        assert_eq!(contents.lines().count(), 8);
        for index in 0..8 {
            assert!(contents.contains(&format!("concurrent-error-{index}")));
        }

        fs::remove_dir_all(root.as_ref()).unwrap();
    }

    #[test]
    fn exposes_stable_status_codes() {
        assert_eq!(ExitStatus::Usage.code(), 64);
        assert_eq!(ExitStatus::DataErr.code(), 65);
        assert_eq!(ExitStatus::NoInput.code(), 66);
        assert_eq!(ExitStatus::Unavailable.code(), 69);
        assert_eq!(ExitStatus::Software.code(), 70);
        assert_eq!(ExitStatus::IoErr.code(), 74);
        assert_eq!(ExitStatus::Config.code(), 78);
    }
}
