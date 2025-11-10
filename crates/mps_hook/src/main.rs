use std::io;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn main() {
    std::process::exit(run_unix());
}

fn run_unix() -> i32 {
    if let Err(e) = Command::new("nvidia-cuda-mps-control")
        .arg("-d")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit()) // show warnings like missing /var/log/nvidia-mps
        .status()
    {
        if e.kind() == io::ErrorKind::NotFound {
            eprintln!("mps_hook: `nvidia-cuda-mps-control` not found in PATH.");
            return 127;
        }
        eprintln!("mps_hook: failed to start control daemon: {e}");
        return 1;
    }

    let uid = unsafe { libc::geteuid() as u32 };
    match server_running_for_uid_ps_grep(uid) {
        Ok(true) => return 0,
        Ok(false) => { /* fallthrough to start */ }
        Err(e) => eprintln!("mps_hook: ps/grep check failed (will try start anyway): {e}"),
    }

    thread::sleep(Duration::from_secs(1));

    // lets retry just in case
    if let Err(e) = Command::new("nvidia-cuda-mps-control")
        .arg("-d")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit()) // show warnings like missing /var/log/nvidia-mps
        .status()
    {
        if e.kind() == io::ErrorKind::NotFound {
            eprintln!("mps_hook: `nvidia-cuda-mps-control` not found in PATH.");
            return 127;
        }
        eprintln!("mps_hook: failed to start control daemon: {e}");
        return 1;
    }

    match server_running_for_uid_ps_grep(uid) {
        Ok(true) => 0,
        Ok(false) => {
            eprintln!("mps_hook: server did not come up after retry.");
            1
        }
        Err(e) => {
            eprintln!("mps_hook: ps/grep re-check failed: {e}");
            1
        }
    }
}

/// ps/grep check: returns true if `nvidia-cuda-mps-server` is running for the given UID.
fn server_running_for_uid_ps_grep(uid: u32) -> Result<bool, String> {
    let pattern = format!(r#"ps -eo uid=,comm= | grep -E "^\s*{}\s+nvidia-cuda-mps-server(\s|$)" -q"#, uid);
    let status = Command::new("sh")
        .arg("-lc")
        .arg(&pattern)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("failed to run shell ps/grep: {e}"))?;
    Ok(status.success())
}
