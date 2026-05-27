use cp_r::CopyOptions;
use libc::uid_t;
use procfs::process::Process;
use serde::Deserialize;
use std::{
    env::{current_dir, set_current_dir},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{self},
};

#[derive(Debug, Deserialize)]
struct OciState {
    bundle: String,
    id: String,
    status: String,
    root: String,
}

#[derive(Debug, Deserialize)]
struct OciConfig {
    process: OciProcess,
}

#[derive(Debug, Deserialize)]
struct OciProcess {
    user: OciUser,
    env: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OciUser {
    uid: uid_t,
}

struct ConfigData {
    uid: uid_t,
    home: Option<PathBuf>,
}

// Deserialize stdin json
fn read_stdin_json<T: for<'de> serde::Deserialize<'de>>() -> Result<T, String> {
    let mut s = String::new();
    io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| format!("failed to read stdin: {e}"))?;
    serde_json::from_str::<T>(&s).map_err(|e| format!("invalid JSON on stdin: {e}"))
}

// Collect ConfigData from OCI State bundle
fn get_config_from_bundle(bundle: &Path) -> Result<ConfigData, String> {
    let bundle_path = PathBuf::from(bundle);
    let cfg_path = bundle_path.join("config.json");

    let cfg: OciConfig = serde_json::from_str(
        &fs::read_to_string(&cfg_path)
            .map_err(|e| format!("failed to read {}: {e}", cfg_path.display()))?,
    )
    .map_err(|e| format!("invalid {}: {e}", cfg_path.display()))?;

    let uid = cfg.process.user.uid;
    let mut home = None;

    for item in cfg.process.env.iter() {
        if item.starts_with("HOME=") {
            let substrings: Vec<&str> = item.splitn(2, '=').collect();
            match substrings.get(1) {
                Some(s) => {
                    home = Some(Path::new(s).to_path_buf());
                }
                None => (),
            }
            break;
        }
    }

    let data = ConfigData {
        uid: uid,
        home: home,
    };

    return Ok(data);
}

// Create home directory if missing
fn create_homedir(root: &Path, home: &Path) -> Result<(), String> {
    let prev_cwd = current_dir().map_err(|e| format!("failed to get current dir: {e}"))?;

    set_current_dir(root).map_err(|e| format!("failed to set current dir: {e}"))?;

    let rel_home_string = format!(".{}", home.display());
    let rel_home = Path::new(&rel_home_string);

    if !rel_home.exists() {
        let prev_umask = Process::myself().unwrap().status().unwrap().umask.unwrap();

        let new_umask = 0o077;
        file_mode::set_umask(new_umask);

        fs::create_dir_all(rel_home).map_err(|e| format!("failed to create dir: {e}"))?;

        file_mode::set_umask(prev_umask);

        let skel = Path::new("etc/skel");
        if skel.exists() {
            CopyOptions::new()
                .copy_tree(skel, rel_home)
                .map_err(|e| format!("failed to copy dir: {e}"))?;
        }
    }
    set_current_dir(&prev_cwd).map_err(|e| format!("failed to set current dir: {e}"))?;
    Ok(())
}

// Find and replace the home directory of user in /etc/passwd file of a container filesystem
fn update_etc_passwd(root: &Path, uid: uid_t, home: &Path) -> Result<(), String> {
    let rel_path = Path::new("etc/passwd");
    let etc_passwd = root.join(rel_path);

    let content: String =
        fs::read_to_string(&etc_passwd).map_err(|e| format!("Cannot read etc_passwd: {e}"))?;

    let mut new_content = String::from("");

    for mut line in content.lines() {
        let split: Vec<&str> = line.split(':').collect();

        let cur_uid = split
            .get(2)
            .ok_or("Cannot read UID from line in /etc/passwd")?
            .to_string();

        let cur_home = split
            .get(5)
            .ok_or("Cannot read HOME from line in /etc/passwd")?
            .to_string();

        let mut new_line = String::from("");

        if cur_uid == uid.to_string() {
            let new_home = home
                .as_os_str()
                .to_os_string()
                .into_string()
                .map_err(|e| format!("failed to convert osstring: {:#?}", e))?;

            if cur_home != new_home {
                for (pos, value) in split.iter().enumerate() {
                    if pos == 0 {
                        new_line.push_str(format!("{value}").as_str())
                    } else if pos == 5 {
                        new_line.push_str(format!(":{new_home}").as_str())
                    } else {
                        new_line.push_str(format!(":{value}").as_str())
                    }
                }
                line = new_line.as_str();
            }
        }
        new_content.push_str(format!("{line}\n").as_str());
    }

    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&etc_passwd)
        .map_err(|e| format!("failed to open file {}: {e}", etc_passwd.display()))?;

    file.write(new_content.as_bytes())
        .map_err(|e| format!("failed to write on file {}: {e}", etc_passwd.display()))?;

    return Ok(());
}

// Get home path value from Container /etc/passwd
fn get_home_from_etc_passwd(root: &Path, uid: uid_t) -> Result<PathBuf, String> {
    let rel_path = Path::new("etc/passwd");
    let etc_passwd = root.join(rel_path);

    let content: String =
        fs::read_to_string(&etc_passwd).map_err(|e| format!("Cannot read etc_passwd: {e}"))?;

    for line in content.lines() {
        let split: Vec<&str> = line.split(':').collect();

        let cur_uid = split
            .get(2)
            .ok_or("Cannot read UID from line in /etc/passwd")?
            .to_string();

        let cur_home = split
            .get(5)
            .ok_or("Cannot read HOME from line in /etc/passwd")?
            .to_string();

        if cur_uid == uid.to_string() {
            return Ok(Path::new(&cur_home).to_path_buf());
        }
    }
    Err(format!(
        "Cannot find home for {uid} in container /etc/passwd"
    ))
}

fn get_graphroot_from_root(root: &PathBuf) -> Result<PathBuf, String> {

    let mut graphroot = root.clone();
    let mut uplevels = 3;
    while uplevels > 0 {
        if graphroot.pop() {
            uplevels -= 1;
        } else {
            return Err(format!("failed get graphroot from root: {:#?}", root));
        }
    }
    Ok(graphroot)
}

fn get_bundle_from_graphroot_and_id(graphroot: &PathBuf, id: &String) -> String {
    let rel_path_str = format!("overlay-containers/{id}/userdata");
    let rel_path = Path::new(&rel_path_str);
    let bundle = graphroot.join(rel_path);
    let bundle_string = bundle.to_string_lossy().to_string();
    bundle_string
}

fn run() -> Result<i32, String> {
    let oci_state: OciState =
        read_stdin_json().map_err(|e| format!("failed to parse OCI State: {e}"))?;

    if oci_state.status != "created" {
        return Ok(0);
    }

    let bundle;
    if oci_state.bundle != "/" {
        bundle = oci_state.bundle;
    } else {
        let root_path = PathBuf::from(&oci_state.root);
        let graphroot = get_graphroot_from_root(&root_path)?;
        bundle = get_bundle_from_graphroot_and_id(&graphroot, &oci_state.id);
    }

    let config = get_config_from_bundle(Path::new(&bundle))?;

    let home = match config.home {
        Some(h) => {
            update_etc_passwd(Path::new(&oci_state.root), config.uid, &h)?;
            h
        }
        None => get_home_from_etc_passwd(Path::new(&oci_state.root), config.uid)?,
    };
    create_homedir(Path::new(&oci_state.root), &home)?;

    Ok(0)
}

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
