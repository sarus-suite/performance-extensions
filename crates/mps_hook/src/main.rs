use std::io::{self, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

fn main() {
    std::process::exit(run_unix());
}

fn run_unix() -> i32 {
    let uid = unsafe { libc::geteuid() as u32 };

    for attempt in 0..2 {
        match start_control_daemon() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!(
                    "mps_hook: nvidia-cuda-mps-control -d exited with status {status}; will try control command anyway."
                );
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::NotFound {
                    eprintln!("mps_hook: `nvidia-cuda-mps-control` not found in PATH.");
                    return 127;
                }
                eprintln!("mps_hook: failed to start control daemon: {e}");
                return 1;
            }
        }

        thread::sleep(Duration::from_secs(1));

        match run_control_command(&format!("start_server -uid {uid}")) {
            Ok(status) if status.success() => return 0,
            Ok(status) if attempt == 0 => {
                eprintln!(
                    "mps_hook: failed to start MPS server for uid {uid} with status {status}; will retry."
                );
            }
            Ok(status) => {
                eprintln!("mps_hook: failed to start MPS server for uid {uid}: {status}");
                return 1;
            }
            Err(e) if attempt == 0 => {
                eprintln!("mps_hook: failed to contact MPS control daemon (will retry): {e}");
            }
            Err(e) => {
                eprintln!("mps_hook: failed to contact MPS control daemon: {e}");
                return 1;
            }
        }
    }

    1
}

fn start_control_daemon() -> io::Result<ExitStatus> {
    Command::new("nvidia-cuda-mps-control")
        .arg("-d")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit()) // show warnings like missing /var/log/nvidia-mps
        .status()
}

fn run_control_command(command: &str) -> io::Result<ExitStatus> {
    let mut child = Command::new("nvidia-cuda-mps-control")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(command.as_bytes())?;
        stdin.write_all(b"\n")?;
    }

    child.wait()
}
