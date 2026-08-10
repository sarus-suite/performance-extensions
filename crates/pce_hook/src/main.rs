use std::{
    env, fmt, fs,
    io::{self, Read, Write},
    process,
};

use precreate_hook_diagnostics::{write_error, ExitStatus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

// Definition of Precreate structure uses the container_edits structure from CDI
// In this version of precreate we only parse for env and mounts
// as can be seen in the containerEdit struct

/// Note: struct uses snake_case but JSON should be camelCase.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Precreate {
    #[serde(default)]
    precreate: Option<String>,

    #[serde(default)]
    container_edits: Vec<ContainerEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContainerEdit {
    #[serde(default)]
    env: Vec<String>,

    #[serde(default)]
    mounts: Vec<Mount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mount {
    container_path: String,
    host_path: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    r#type: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    options: Option<Vec<String>>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        if let Err(log_error) = write_error("pce_hook", error.exit_status(), &error.to_string()) {
            eprintln!("pce_hook: failed to write diagnostic log: {log_error}");
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

    let (mounts_to_add, env_entries_raw) = read_pce_input()?;

    if !mounts_to_add.is_empty() {
        append_mounts(obj, mounts_to_add)?;
    }

    // Validate env entries and merge as strings
    let env_entries = validate_env_strings(env_entries_raw)?;
    if !env_entries.is_empty() {
        merge_process_env_strings(obj, env_entries)?;
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

// Reading for precreate container edits input
// we try to read the input file
// we try to parse it into json
// then we boostrapt an empty mount and env vectors
// then we extend them with the new mounts and envs
fn read_pce_input() -> Result<(Vec<Mount>, Vec<String>)> {
    let Some(path_os) = env::var_os("PCE_INPUT") else {
        return Ok((Vec::new(), Vec::new()));
    };

    let path = path_os.to_string_lossy();

    // Read file
    let s = fs::read_to_string(&*path).map_err(|e| {
        Error::new(
            ExitStatus::NoInput,
            format!("PCE_INPUT: fail to read {}: {}", path, e),
        )
    })?;

    // parse into json
    let pre: Precreate = serde_json::from_str(&s)
        .map_err(|e| Error::new(ExitStatus::Config, format!("PCE_INPUT: Invalid JSON: {e}")))?;

    // extract mounts and envs
    let mut mounts = Vec::new();
    let mut envs = Vec::new();
    for cedit in pre.container_edits {
        mounts.extend(cedit.mounts);
        envs.extend(cedit.env);
    }

    Ok((mounts, envs))
}

/// Ensure a `Value` is an object and return it as a mutable map.
fn ensure_obj<'a>(
    candidate: Option<&'a mut Map<String, Value>>,
    err: &str,
) -> Result<&'a mut Map<String, Value>> {
    candidate.ok_or_else(|| Error::new(ExitStatus::DataErr, format!("Validation error: {err}.")))
}

// Manual write of mount block as cdi and container config formats dont match
fn append_mounts(obj: &mut Map<String, Value>, mounts_to_add: Vec<Mount>) -> Result<()> {
    let mounts = ensure_array_field(obj, "mounts")?;

    for m in mounts_to_add {
        // Map input hostPath/containerPath → OCI source/destination
        let mut out = Map::new();
        out.insert("destination".to_string(), Value::String(m.container_path));

        if let Some(t) = m.r#type {
            out.insert("type".to_string(), Value::String(t));
        }

        out.insert("source".to_string(), Value::String(m.host_path));

        if let Some(opts) = m.options {
            out.insert(
                "options".to_string(),
                Value::Array(opts.into_iter().map(Value::String).collect()),
            );
        }

        mounts.push(Value::Object(out));
    }

    Ok(())
}

//fn append_mounts(obj: &mut Map<String, Value>, mounts_to_add: Vec<Mount>) -> Result<(), String> {
//    let mounts = ensure_array_field(obj, "mounts")?;
//
//    // mount entries just get added, no special logic
//    for m in mounts_to_add {
//        mounts.push(serde_json::to_value(m).map_err(|e| format!("Failed to serialize mount: {e}"))?);
//    }
//
//    Ok(())
//}

fn ensure_array_field<'a>(
    obj: &'a mut Map<String, Value>,
    field: &str,
) -> Result<&'a mut Vec<Value>> {
    use serde_json::map::Entry;

    // before we return the field, we check if the entry is empty/vacant, if so we create the
    // field, otherwise, we check it needs to be an array or we return error.
    // TODO: can we have non-array env and mounts?
    match obj.entry(field.to_string()) {
        Entry::Vacant(v) => {
            // Insert an empty array and return a mutable ref to it.
            let val = v.insert(Value::Array(Vec::new()));
            Ok(val.as_array_mut().expect("we just inserted an Array"))
        }
        Entry::Occupied(e) => {
            // Tie the borrow to `obj` by consuming the entry.
            let v = e.into_mut(); // &'a mut Value
            match v {
                Value::Array(ref mut arr) => Ok(arr),
                _ => Err(Error::new(
                    ExitStatus::DataErr,
                    format!("Validation error: '{field}' exists but is not an array."),
                )),
            }
        }
    }
}

/// Validate a list of "KEY=value" strings.
fn validate_env_strings(entries: Vec<String>) -> Result<Vec<String>> {
    for s in &entries {
        validate_kv_format(s)?;
    }

    Ok(entries)
}

fn validate_kv_format(s: &str) -> Result<()> {
    if let Some((k, _v)) = s.split_once('=') {
        if k.is_empty() {
            return Err(Error::new(
                ExitStatus::Config,
                "Empty environment variable name before '='",
            ));
        }
        Ok(())
    } else {
        Err(Error::new(
            ExitStatus::Config,
            format!("Invalid env entry (expected KEY=VALUE): {s}"),
        ))
    }
}

// merging envs into the container config json is as follows
// 1. we need to add envs into the process object
// 1.5 we create process if it is not there
// 2. we validate out envs
// 3 new env entries are added using two rules
// 3.1 we append if the env var is new
// 3.2 we replace if we find it duplicated
fn merge_process_env_strings(obj: &mut Map<String, Value>, env_entries: Vec<String>) -> Result<()> {
    // Ensure "process" is an object
    let process_val = obj
        .entry("process".to_string())
        .or_insert_with(|| json!({}));
    let process_obj = process_val.as_object_mut().ok_or_else(|| {
        Error::new(
            ExitStatus::DataErr,
            "Validation error: 'process' exists but is not an object.",
        )
    })?;

    let env_arr = ensure_array_field(process_obj, "env")?;

    // logic to add new envs
    for new in env_entries {
        // Safe: already validated as KEY=value in main
        let (new_key, _) = new.split_once('=').unwrap();

        // We scan to find if we have a duplicate, if so we overwrite with new
        if let Some(idx) = env_arr.iter().rposition(|v| {
            v.as_str()
                .and_then(|s| s.split_once('=').map(|(k, _)| k))
                .is_some_and(|k| k == new_key)
        }) {
            env_arr[idx] = Value::String(new);
        } else {
            env_arr.push(Value::String(new));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_pce_env_is_config_error() {
        let error = validate_kv_format("NOT_AN_ASSIGNMENT").unwrap_err();
        assert_eq!(error.exit_status(), ExitStatus::Config);
    }

    #[test]
    fn malformed_oci_shape_is_data_error() {
        let error = ensure_obj(None, "top-level JSON must be an object").unwrap_err();
        assert_eq!(error.exit_status(), ExitStatus::DataErr);
    }
}
