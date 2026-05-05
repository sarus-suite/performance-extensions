use std::{
    io::{self, Read, Write},
    process,
};

use serde_json::{Map, map::Entry, Value, json};
use users::get_user_by_uid;
use users::os::unix::UserExt;

fn main() -> io::Result<()> {
    // we go for run
    if let Err(e) = run() {
        // we output errors to stderr
        eprintln!("{e}");

        // we return failure status
        process::exit(1);
    }

    Ok(())
}

fn run() -> Result<(), String> {
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

    serde_json::to_writer_pretty(&mut stdout, &value)
        .map_err(|e| format!("Failed to write JSON to stdout: {e}"))?;

    stdout
        .write_all(b"\n")
        .map_err(|e| format!("Failed to write newline to stdout: {e}"))?;
    stdout
        .flush()
        .map_err(|e| format!("Failed to flush stdout: {e}"))?;

    Ok(())
}

// Returning ENV HOME entry from the system /etc/passwd
// 1. get process.user.uid entry from json obj
// 2. get user entry from uid through getpwuid_r
// 3. get homedir from user entry
// 4. build HOME entry string and return it
fn get_home_env_entry(obj: &mut Map<String, Value>) -> Result<String, String> {

    // Ensure "process" exists
    let process_val = obj.entry("process".to_string());
    match process_val {
        Entry::Vacant(_) => return Err(format!("Validation error: 'process' doesn't exist.")),
        Entry::Occupied(_) => {},
    }

    // Ensure "process" is an object
    let process_obj = process_val
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "Validation error: 'process' exists but is not an object.".to_string())?;

    // Ensure "user" exists
    let user_val = process_obj.entry("user".to_string());
    match user_val {
        Entry::Vacant(_) => return Err(format!("Validation error: 'process.user' doesn't exist.")),
        Entry::Occupied(_) => {},
    }

    let user_obj = user_val
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "Validation error: 'process.user' exists but is not an object.".to_string())?;

    // Ensure "uid" exists
    let uid_val = user_obj.entry("uid".to_string());
    match uid_val {
        Entry::Vacant(_) => return Err(format!("Validation error: 'process.user.uid' doesn't exist.")),
        Entry::Occupied(_) => {},
    }

    // Ensure "uid" is a number
    let uid: u32 = uid_val
        .or_insert_with(|| json!(0))
        .as_number()
        .ok_or_else(|| "Validation error: 'process.user.uid' exists but is not a number.".to_string())?
        .as_u64()
        .ok_or_else(|| "Validation error: 'process.user.uid' is a number but doesn't fit u64.".to_string())?
        .try_into()
        .map_err(|e| format!("Validation error: 'process.user.uid' is a number but doesn't fit u32: {e}"))?;

    let user = match get_user_by_uid(uid) {
        Some(u) => u,
        None => return Err(format!("Unknown UID: cannot find User by UID {uid}")),
    };
    let homedir = user.home_dir().display();

    let home_env_entry = format!("HOME={homedir}");

    Ok(home_env_entry)
}

// Precreate takes as stdin the container config json
// We return error if we cannot read or
// if we cannot parse a valid input json
fn read_stdin_json() -> Result<Value, String> {
    let mut input = String::new();

    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("Failed to read from stdin: {e}"))?;

    serde_json::from_str(&input).map_err(|e| format!("Invalid JSON: {e}"))
}

/// Ensure a `Value` is an object and return it as a mutable map.
fn ensure_obj<'a>(
    candidate: Option<&'a mut Map<String, Value>>,
    err: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    candidate.ok_or_else(|| format!("Validation error: {err}."))
}

fn ensure_array_field<'a>(
    obj: &'a mut Map<String, Value>,
    field: &str,
) -> Result<&'a mut Vec<Value>, String> {
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
                _ => Err(format!(
                    "Validation error: '{field}' exists but is not an array."
                )),
            }
        }
    }
}

/// Validate a list of "KEY=value" strings.
fn validate_env_strings(entries: Vec<String>) -> Result<Vec<String>, String> {
    for s in &entries {
        validate_kv_format(s)?;
    }

    Ok(entries)
}

fn validate_kv_format(s: &str) -> Result<(), String> {
    if let Some((k, _v)) = s.split_once('=') {
        if k.is_empty() {
            return Err("Empty environment variable name before '='".into());
        }
        Ok(())
    } else {
        Err(format!("Invalid env entry (expected KEY=VALUE): {s}"))
    }
}

// merging envs into the container config json is as follows
// 1. we need to add envs into the process object
// 1.5 we create process if it is not there
// 2. we validate out envs
// 3 new env entries are added using two rules
// 3.1 we append if the env var is new
// 3.2 we replace if we find it duplicated
fn merge_process_env_strings(
    obj: &mut Map<String, Value>,
    env_entries: Vec<String>,
) -> Result<(), String> {
    // Ensure "process" is an object
    let process_val = obj
        .entry("process".to_string())
        .or_insert_with(|| json!({}));
    let process_obj = process_val
        .as_object_mut()
        .ok_or_else(|| "Validation error: 'process' exists but is not an object.".to_string())?;

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
