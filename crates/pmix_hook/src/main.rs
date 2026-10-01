use oci_spec::runtime::MountBuilder;
use precreate_hook_diagnostics::{write_error, ExitStatus};
use serde_json::{json, map::Entry, Map, Value};
use std::{
    collections::HashMap,
    fmt,
    io::{self, Read, Write},
    path::PathBuf,
    process::{self},
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
    // Read and parse stdin JSON
    let mut value = read_stdin_json()?;
    let obj = ensure_obj(value.as_object_mut(), "top-level JSON must be an object")?;
    let container_env = get_process_env_hashmap(obj)?;

    let slurm_job_id = get_env_entry_str(container_env.clone(), String::from("SLURM_JOB_ID"));
    let slurm_step_id = get_env_entry_str(container_env.clone(), String::from("SLURM_STEP_ID"));

    if (slurm_job_id == "") || (slurm_step_id == "") {
        return Ok(());
    }

    let slurm_mpi_type = get_env_entry_str(container_env.clone(), String::from("SLURM_MPI_TYPE"));
    let pmix_found = is_pattern_in_env_keys(container_env.clone(), "PMIX_");

    if ((slurm_mpi_type == "") || slurm_mpi_type.starts_with("pmix")) && pmix_found {
        let pmix_ptl_module =
            get_env_entry_str(container_env.clone(), String::from("PMIX_PTL_MODULE"));
        let pmix_mca_ptl = get_env_entry_str(container_env.clone(), String::from("PMIX_MCA_ptl"));

        if pmix_ptl_module != "" && pmix_mca_ptl == "" {
            insert_process_env(obj, "PMIX_MCA_ptl", &pmix_ptl_module)?;
        }

        let pmix_security_mode =
            get_env_entry_str(container_env.clone(), String::from("PMIX_SECURITY_MODE"));
        let pmix_mca_psec = get_env_entry_str(container_env.clone(), String::from("PMIX_MCA_psec"));

        if pmix_security_mode != "" && pmix_mca_psec == "" {
            insert_process_env(obj, "PMIX_MCA_psec", &pmix_security_mode)?;
        }

        let pmix_gds_module =
            get_env_entry_str(container_env.clone(), String::from("PMIX_GDS_MODULE"));
        let pmix_mca_gds = get_env_entry_str(container_env.clone(), String::from("PMIX_MCA_gds"));

        if pmix_gds_module != "" && pmix_mca_gds == "" {
            insert_process_env(obj, "PMIX_MCA_gds", &pmix_gds_module)?;
        }

        let pmix_server_tmpdir =
            get_env_entry_str(container_env.clone(), String::from("PMIX_SERVER_TMPDIR"));
        let pmix_system_tmpdir =
            get_env_entry_str(container_env.clone(), String::from("PMIX_SYSTEM_TMPDIR"));

        if pmix_server_tmpdir != "" {
            let folder = pmix_server_tmpdir.trim_end_matches('/');
            add_mount(obj, &folder);
        }
        if pmix_system_tmpdir != "" {
            let folder = pmix_system_tmpdir.trim_end_matches('/');
            let folder_format1 =
                format!("{folder}/spmix_appdir_{slurm_job_id}_{slurm_job_id}.{slurm_step_id}");
            if PathBuf::from(&folder_format1).is_dir() {
                add_mount(obj, &folder_format1);
            } else {
                let folder_format2 =
                    format!("{folder}/spmix_appdir_{slurm_job_id}.{slurm_step_id}");
                add_mount(obj, &folder_format2);
            }
        }
    }

    // Pretty-print output JSON with trailing newline
    let mut stdout = io::stdout().lock();

    serde_json::to_writer_pretty(&mut stdout, &value).map_err(|e| {
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

fn get_env_entry_str(env: HashMap<String, String>, key: String) -> String {
    let ret = match env.get(&key) {
        Some(v) => v.clone(),
        None => String::from(""),
    };
    ret
}

fn is_pattern_in_env_keys(env: HashMap<String, String>, pattern: &str) -> bool {
    for (k, _v) in env.iter() {
        if k.starts_with(pattern) {
            return true;
        }
    }
    false
}

fn add_mount(obj: &mut Map<String, Value>, folder: &str) {
    let mut found = false;
    let opts = vec![
        String::from("private"),
        String::from("nosuid"),
        String::from("noexec"),
        String::from("nodev"),
        String::from("rw"),
        String::from("bind"),
    ];

    let new_mount = MountBuilder::default()
        .typ("bind")
        .options(opts)
        .source(folder)
        .destination(folder)
        .build()
        .unwrap();

    let new_mount_value = serde_json::to_value(new_mount).unwrap();

    for (k, v) in &mut *obj {
        if k == "mounts" {
            found = true;
            if !v.is_array() {
                return;
            };
            let mut new_v = v.as_array().unwrap().clone();
            new_v.push(new_mount_value.clone());
            obj.insert("mounts".to_string(), serde_json::Value::Array(new_v));
            break;
        }
    }
    if !found {
        let mounts = serde_json::to_value(vec![new_mount_value]).unwrap();
        obj.insert(String::from("mounts"), mounts);
    }
}

fn insert_process_env(obj: &mut Map<String, Value>, key: &str, value: &str) -> Result<()> {
    // Ensure "process" exists
    let process_val = obj.entry("process".to_string());
    match process_val {
        Entry::Vacant(_) => {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process' doesn't exist.",
            ))
        }
        Entry::Occupied(_) => {}
    }

    // Ensure "process" is an object
    let process_obj = process_val
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process' exists but is not an object.",
            )
        })?;

    // Ensure "env" exists
    let env_val = process_obj.entry("env".to_string());
    match env_val {
        Entry::Vacant(_) => {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' doesn't exist.",
            ))
        }
        Entry::Occupied(_) => {}
    }

    let env_array = env_val
        .or_insert_with(|| json!({}))
        .as_array_mut()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' exists but is not an array.",
            )
        })?;

    let mut new_env_array = vec![];

    let mut found = false;
    for entry in env_array.iter() {
        if !entry.is_string() {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' array contains an item that is not a string",
            ));
        }

        let (k, v) = match entry.as_str().unwrap().split_once("=") {
            Some(s) => s,
            None => {
                return Err(Error::new(
                    ExitStatus::DataErr,
                    "Validation error: 'process.env' array contains an item that doesn't contain '=' separator",
                ))
            },
        };

        if k == key {
            found = true;
            new_env_array.push(format!("{key}={value}").into());
        } else {
            new_env_array.push(format!("{k}={v}").into());
        }
    }
    if !found {
        new_env_array.push(format!("{key}={value}").into());
    }

    process_obj.insert(String::from("env"), serde_json::Value::Array(new_env_array));
    Ok(())
}

fn get_process_env_hashmap(obj: &mut Map<String, Value>) -> Result<HashMap<String, String>> {
    // Ensure "process" exists
    let process_val = obj.entry("process".to_string());
    match process_val {
        Entry::Vacant(_) => {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process' doesn't exist.",
            ))
        }
        Entry::Occupied(_) => {}
    }

    // Ensure "process" is an object
    let process_obj = process_val
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process' exists but is not an object.",
            )
        })?;

    // Ensure "env" exists
    let env_val = process_obj.entry("env".to_string());
    match env_val {
        Entry::Vacant(_) => {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' doesn't exist.",
            ))
        }
        Entry::Occupied(_) => {}
    }

    let env_array = env_val
        .or_insert_with(|| json!({}))
        .as_array_mut()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' exists but is not an array.",
            )
        })?;

    let mut ret = HashMap::new();

    for entry in env_array.iter() {
        if !entry.is_string() {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.env' array contains an item that is not a string",
            ));
        }

        let (k, v) = match entry.as_str().unwrap().split_once("=") {
            Some(s) => s,
            None => {
                return Err(Error::new(
                    ExitStatus::DataErr,
                    "Validation error: 'process.env' array contains an item that doesn't contain '=' separator",
                ))
            },
        };

        ret.insert(String::from(k), String::from(v));
    }
    Ok(ret)
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
    candidate.ok_or_else(|| Error::new(ExitStatus::DataErr, format!("Validation error: {err}.")))
}
