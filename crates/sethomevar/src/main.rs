use std::{
    fmt,
    io::{self, Read, Write},
    process::{self, Command},
};

use precreate_hook_diagnostics::{write_error, ExitStatus};
use serde_json::{json, map::Entry, Map, Value};

const REPLACE_DEFAULT_VALUE: bool = false;
const REPLACE_ANNOTATION_NAME: &str = "com.hooks.sethomevar.override";

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        if let Err(log_error) = write_error("sethomevar", error.exit_status(), &error.to_string()) {
            eprintln!("sethomevar: failed to write diagnostic log: {log_error}");
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

    let env_entries_raw = vec![get_home_env_entry(obj)?];

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

fn get_replace_mode(obj: &Map<String, Value>) -> Result<bool> {
    // Ensure "annotations" exists
    let annotations = obj
        .get("annotations")
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'annotations' doesn't exist.",
            )
        })?
        // Ensure "annotations" is an object
        .as_object()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'annotations' exists but is not an object.",
            )
        })?;

    // Check REPLACE_ANNOTATION_NAME entry exists
    let replace = annotations
        .get(REPLACE_ANNOTATION_NAME)
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                format!(
                    "Validation error: '{}' doesn't exist.",
                    REPLACE_ANNOTATION_NAME
                ),
            )
        })?
        .as_str()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                format!(
                    "Validation error: 'annotations.{}' exists but is not a string.",
                    REPLACE_ANNOTATION_NAME
                ),
            )
        })?;

    Ok(match replace {
        "true" => true,
        "false" => false,
        _ => REPLACE_DEFAULT_VALUE,
    })
}

// Return the HOME environment entry from the system account database.
// 1. get process.user.uid entry from json obj
// 2. get user entry from uid through getent passwd
// 3. get homedir from user entry
// 4. build HOME entry string and return it
fn get_home_env_entry(obj: &mut Map<String, Value>) -> Result<String> {
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

    // Ensure "user" exists
    let user_val = process_obj.entry("user".to_string());
    match user_val {
        Entry::Vacant(_) => {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.user' doesn't exist.",
            ))
        }
        Entry::Occupied(_) => {}
    }

    let user_obj = user_val
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.user' exists but is not an object.",
            )
        })?;

    // Ensure "uid" exists
    let uid_val = user_obj.entry("uid".to_string());
    match uid_val {
        Entry::Vacant(_) => {
            return Err(Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.user.uid' doesn't exist.",
            ))
        }
        Entry::Occupied(_) => {}
    }

    // Ensure "uid" is a number
    let uid: u32 = uid_val
        .or_insert_with(|| json!(0))
        .as_number()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.user.uid' exists but is not a number.",
            )
        })?
        .as_u64()
        .ok_or_else(|| {
            Error::new(
                ExitStatus::DataErr,
                "Validation error: 'process.user.uid' is a number but doesn't fit u64.",
            )
        })?
        .try_into()
        .map_err(|e| {
            Error::new(
                ExitStatus::DataErr,
                format!(
                    "Validation error: 'process.user.uid' is a number but doesn't fit u32: {e}"
                ),
            )
        })?;

    let getent_out = match Command::new("getent")
        .args(["passwd", uid.to_string().as_str()])
        .output()
    {
        Ok(process) => process,
        Err(err) => {
            return Err(Error::new(
                ExitStatus::Unavailable,
                format!("Running command error: getent passwd {uid}: {err}"),
            ))
        }
    };

    if !getent_out.status.success() {
        return Err(Error::new(
            ExitStatus::Unavailable,
            format!(
                "Exit command error: getent passwd {uid} returned: {}",
                getent_out.status
            ),
        ));
    }

    let getent_stdout = match std::string::String::from_utf8(getent_out.stdout) {
        Ok(out) => out,
        Err(err) => {
            return Err(Error::new(
                ExitStatus::Software,
                format!("Translating output command error: getent passwd {uid}: {err}"),
            ))
        }
    };

    let getent_stdout_vec: Vec<&str> = getent_stdout.split(':').collect();
    let homedir = match getent_stdout_vec.get(5) {
        Some(s) => s,
        None => {
            return Err(Error::new(
                ExitStatus::Software,
                format!("Output command error: getent passwd {uid}, cannot find homedir field."),
            ))
        }
    };

    let home_env_entry = format!("HOME={homedir}");

    Ok(home_env_entry)
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
                Value::Array(arr) => Ok(arr),
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
                ExitStatus::Software,
                "Empty environment variable name before '='",
            ));
        }
        Ok(())
    } else {
        Err(Error::new(
            ExitStatus::Software,
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
// 3.2 we replace if we find it duplicated and replace annotation is true
fn merge_process_env_strings(obj: &mut Map<String, Value>, env_entries: Vec<String>) -> Result<()> {
    let replace = get_replace_mode(obj).unwrap_or(REPLACE_DEFAULT_VALUE);

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
            if replace {
                env_arr[idx] = Value::String(new);
            }
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
    fn missing_uid_is_data_error() {
        let mut config = json!({"process": {"user": {}}});
        let error = get_home_env_entry(config.as_object_mut().unwrap()).unwrap_err();
        assert_eq!(error.exit_status(), ExitStatus::DataErr);
    }

    #[test]
    fn sethomevar_uses_only_shared_statuses() {
        assert_eq!(ExitStatus::DataErr.code(), 65);
        assert_eq!(ExitStatus::Unavailable.code(), 69);
        assert_eq!(ExitStatus::Software.code(), 70);
        assert_eq!(ExitStatus::IoErr.code(), 74);
    }
}
