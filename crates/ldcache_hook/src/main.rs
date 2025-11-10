use serde::Deserialize;
use std::{
    env,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    time::UNIX_EPOCH,
};

fn main() {
    let code = match run() {
        Ok(status) => status,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    };
    process::exit(code);
}

#[derive(Debug, Deserialize)]
struct OciState {
    bundle: String,
}

#[derive(Debug, Deserialize)]
struct OciConfig {
    root: OciRoot,
}

#[derive(Debug, Deserialize)]
struct OciRoot {
    path: String,
}

fn run() -> Result<i32, String> {
    // Get OCI state from stdin to get the bundle dir.
    let state: OciState = read_stdin_json()?;

    // Load bundle to resolve rootfs
    let bundle = PathBuf::from(&state.bundle);
    let cfg_path = bundle.join("config.json");
    let cfg: OciConfig = serde_json::from_str(
        &fs::read_to_string(&cfg_path)
            .map_err(|e| format!("failed to read {}: {e}", cfg_path.display()))?,
    )
    .map_err(|e| format!("invalid {}: {e}", cfg_path.display()))?;
    let rootfs = resolve_rootfs(&bundle, &cfg.root.path);

    // ldconfig binary
    let ldconfig = env::var_os("LDCONFIG_PATH")
        .map(|s| s.into_string().unwrap_or_else(|_| "ldconfig".to_string()))
        .unwrap_or_else(|| "ldconfig".to_string());

    // Run ldconfig -v -r on rootfs path
    let status = Command::new(&ldconfig)
        .arg("-v")
        .arg("-r")
        .arg(&rootfs)
        .stdin(Stdio::null())
        // Inherit stdout/stderr so operators can see ldconfig output if desired.
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                format!("ldcache_hook: `{}` not found in PATH.", ldconfig)
            } else {
                format!("ldcache_hook: failed to exec `{}`: {}", ldconfig, e)
            }
        });

    // Manage command output cases
    let code = match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(msg) if msg.contains("not found in PATH") => {
            eprintln!("{msg}");
            127
        }
        Err(msg) => {
            eprintln!("{msg}");
            1
        }
    };

    // summary of /etc/ld.so.cache from rootfs
    summarize_cache(&rootfs);

    Ok(code)
}

fn read_stdin_json<T: for<'de> serde::Deserialize<'de>>() -> Result<T, String> {
    let mut s = String::new();
    io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    serde_json::from_str::<T>(&s).map_err(|e| format!("invalid JSON on stdin: {e}"))
}

fn resolve_rootfs(bundle: &Path, root_path: &str) -> PathBuf {
    let p = Path::new(root_path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        bundle.join(p)
    }
}

fn summarize_cache(rootfs: &Path) {
    let p = rootfs.join("etc/ld.so.cache");
    match fs::metadata(&p) {
        Ok(md) => {
            let size = md.len();
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            eprintln!(
                "ldcache_hook: cache={} exists size={} mtime_unix={}",
                p.display(),
                size,
                mtime
            );
        }
        Err(_) => {
            eprintln!("ldcache_hook: cache={} missing", p.display());
        }
    }
}

