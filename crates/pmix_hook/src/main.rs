use oci_spec::runtime::MountBuilder;
use precreate_hook_diagnostics::{write_error, ExitStatus};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    fmt,
    io::{self, Read, Write},
    path::PathBuf,
    process,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        if let Err(log_error) = write_error("pmix_hook", error.exit_status(), &error.to_string()) {
            eprintln!("pmix_hook: failed to write diagnostic log: {log_error}");
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
    let mut value = read_stdin_json()?;
    let obj = ensure_obj(value.as_object_mut(), "top-level JSON must be an object")?;
    apply_pmix_updates(obj)?;
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

fn apply_pmix_updates(obj: &mut Map<String, Value>) -> Result<()> {
    let container_env = get_process_env_hashmap(obj)?;

    let slurm_job_id = get_env_entry_str(&container_env, "SLURM_JOB_ID");
    let slurm_step_id = get_env_entry_str(&container_env, "SLURM_STEP_ID");

    // Skip it when outside of a JOB or if SLURM_* variables are not available
    if (slurm_job_id.is_empty()) || (slurm_step_id.is_empty()) {
        return Ok(());
    }

    let slurm_mpi_type = get_env_entry_str(&container_env, "SLURM_MPI_TYPE");
    let pmix_found = is_pattern_in_env_keys(&container_env, "PMIX_");

    if ((slurm_mpi_type.is_empty()) || slurm_mpi_type.starts_with("pmix")) && pmix_found {
        // UPDATE PMIx environment variables
        const PMIX_ENV_UPDATES: &[(&str, &str)] = &[
            ("PMIX_PTL_MODULE", "PMIX_MCA_ptl"),
            ("PMIX_SECURITY_MODE", "PMIX_MCA_psec"),
            ("PMIX_GDS_MODULE", "PMIX_MCA_gds"),
        ];

        for (src, dst) in PMIX_ENV_UPDATES {
            let src_val = get_env_entry_str(&container_env, src);
            let dst_val = get_env_entry_str(&container_env, dst);
            if !src_val.is_empty() && dst_val.is_empty() {
                insert_process_env(obj, dst, src_val)?;
            }
        }

        // UPDATE PMIx mounts
        let pmix_server_tmpdir = get_env_entry_str(&container_env, "PMIX_SERVER_TMPDIR");
        let pmix_system_tmpdir = get_env_entry_str(&container_env, "PMIX_SYSTEM_TMPDIR");

        if !pmix_server_tmpdir.is_empty() {
            let folder = pmix_server_tmpdir.trim_end_matches('/');
            if PathBuf::from(&folder).is_dir() {
                add_mount(obj, &folder)?;
            }
        }
        if !pmix_system_tmpdir.is_empty() {
            let folder = pmix_system_tmpdir.trim_end_matches('/');
            let folder_format1 =
                format!("{folder}/spmix_appdir_{slurm_job_id}_{slurm_job_id}.{slurm_step_id}");
            let folder_format2 = format!("{folder}/spmix_appdir_{slurm_job_id}.{slurm_step_id}");

            if PathBuf::from(&folder_format1).is_dir() {
                add_mount(obj, &folder_format1)?;
            } else if PathBuf::from(&folder_format2).is_dir() {
                add_mount(obj, &folder_format2)?;
            }
        }
    }

    Ok(())
}

fn get_env_entry_str<'a>(env: &'a HashMap<String, String>, key: &str) -> &'a str {
    env.get(key).map(|s| s.as_str()).unwrap_or("")
}

fn is_pattern_in_env_keys<'a>(env: &'a HashMap<String, String>, pattern: &str) -> bool {
    env.keys().any(|k| k.starts_with(pattern))
}

fn add_mount(obj: &mut Map<String, Value>, folder: &str) -> Result<()> {
    let opts: Vec<String> = vec!["private", "nosuid", "noexec", "nodev", "rw", "bind"]
        .into_iter()
        .map(String::from)
        .collect();

    let new_mount = MountBuilder::default()
        .typ("bind")
        .options(opts)
        .source(folder)
        .destination(folder)
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
