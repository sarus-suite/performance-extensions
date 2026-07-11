use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::error::Error as StdError;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::symlink;
use std::path::{Component, Path, PathBuf};
use std::process::{self, Command};
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    match run() {
        Ok(()) => process::exit(0),
        Err(error) => {
            eprintln!("pc_injection_hook: {error}");
            process::exit(1);
        }
    }
}

fn run() -> Result<()> {
    let mut config = read_stdin_json_value()?;
    let inputs = load_inputs(&config)?;
    let discovery = discover_container_libraries(&inputs)?;
    let edits = plan_config_edits(&inputs, &discovery.libraries)?;
    apply_config_edits(&mut config, &edits)?;
    write_stdout_json(&config)?;

    for warning in discovery.warnings.into_iter().chain(edits.warnings) {
        eprintln!("pc_injection_hook: warning: {warning}");
    }

    Ok(())
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
enum Error {
    Message(String),
    Io { context: String, source: io::Error },
    Json(serde_json::Error),
}

impl Error {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => write!(f, "{message}"),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::Json(source) => write!(f, "invalid JSON: {source}"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Io { source, .. } => Some(source),
            Self::Json(source) => Some(source),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(source: serde_json::Error) -> Self {
        Self::Json(source)
    }
}

#[derive(Debug, Clone)]
struct HookInputs {
    rootfs: PathBuf,
    ldconfig: PathBuf,
    primary_libs: Vec<Library>,
    dependency_libs: Vec<Library>,
    allow_unversioned_primary_overwrite: bool,
    extra_files: Vec<PathBuf>,
    extra_mounts: Vec<ExtraMountEdit>,
    extra_env: Vec<String>,
}

#[derive(Debug, Default)]
struct CliOverrides {
    ldconfig: Option<PathBuf>,
    primary_libs: Vec<Library>,
    dependency_libs: Vec<Library>,
    allow_unversioned_primary_overwrite: bool,
    extra_files: Vec<PathBuf>,
    extra_mounts: Vec<ExtraMountEdit>,
    extra_env: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct AbiVersion {
    major: Option<u32>,
    minor: Option<u32>,
    patch: Option<u32>,
}

impl AbiVersion {
    fn has_major(&self) -> bool {
        self.major.is_some()
    }

    fn components(&self) -> [Option<u32>; 3] {
        [self.major, self.minor, self.patch]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Library {
    path: PathBuf,
    file_name: String,
    linker_name: String,
    real_name: String,
    abi: AbiVersion,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DiscoveryOutcome {
    libraries: Vec<Library>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConfigEdits {
    mounts: Vec<MountEdit>,
    ld_library_path_dirs: Vec<PathBuf>,
    extra_mounts: Vec<ExtraMountEdit>,
    extra_env: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MountEdit {
    source: PathBuf,
    destination: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExtraMountEdit {
    source: PathBuf,
    destination: PathBuf,
    mount_type: String,
    options: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MountDecision {
    mounts: Vec<MountEdit>,
    ld_library_path_dir: Option<PathBuf>,
    warnings: Vec<String>,
}

fn read_stdin_json_value() -> Result<Value> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| Error::io("failed to read OCI config from stdin", e))?;
    serde_json::from_str(&input).map_err(Error::from)
}

fn write_stdout_json(value: &Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    stdout
        .write_all(b"\n")
        .map_err(|e| Error::io("failed to write newline to stdout", e))?;
    stdout
        .flush()
        .map_err(|e| Error::io("failed to flush stdout", e))?;
    Ok(())
}

fn load_inputs(config: &Value) -> Result<HookInputs> {
    load_inputs_from_sources(config, parse_cli_overrides()?)
}

fn load_inputs_from_sources(config: &Value, cli: CliOverrides) -> Result<HookInputs> {
    let root_path = config
        .get("root")
        .and_then(Value::as_object)
        .and_then(|root| root.get("path"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::message("OCI config is missing root.path"))?;

    let env_ldconfig =
        PathBuf::from(env::var_os("LDCONFIG_PATH").unwrap_or_else(|| "ldconfig".into()));
    let env_primary_libs = parse_optional_library_list("INJECTION_PRIMARY_LIBS")?;
    let env_dependency_libs = parse_optional_library_list("INJECTION_DEPENDENCY_LIBS")?;
    let env_extra_files = parse_optional_path_list("INJECTION_EXTRA_FILES");
    let env_extra_mounts = parse_optional_mount_specs("INJECTION_EXTRA_MOUNTS")?;
    let env_extra_env = parse_optional_env_specs("INJECTION_EXTRA_ENV")?;

    let inputs = HookInputs {
        rootfs: resolve_rootfs(root_path)?,
        ldconfig: cli.ldconfig.unwrap_or(env_ldconfig),
        primary_libs: prefer_cli_vec(cli.primary_libs, env_primary_libs),
        dependency_libs: prefer_cli_vec(cli.dependency_libs, env_dependency_libs),
        allow_unversioned_primary_overwrite: cli.allow_unversioned_primary_overwrite,
        extra_files: prefer_cli_vec(cli.extra_files, env_extra_files),
        extra_mounts: prefer_cli_vec(cli.extra_mounts, env_extra_mounts),
        extra_env: prefer_cli_vec(cli.extra_env, env_extra_env),
    };

    validate_inputs(&inputs)?;

    Ok(inputs)
}

fn parse_cli_overrides() -> Result<CliOverrides> {
    parse_cli_overrides_from_args(env::args_os().skip(1))
}

fn parse_cli_overrides_from_args<I>(args: I) -> Result<CliOverrides>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let mut overrides = CliOverrides::default();

    for arg in args {
        let arg = arg
            .into_string()
            .map_err(|_| Error::message("hook args must contain valid UTF-8"))?;

        if let Some(value) = arg.strip_prefix("--ldconfig=") {
            overrides.ldconfig = Some(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--lib=") {
            overrides.primary_libs.push(Library::parse_host(value)?);
        } else if let Some(value) = arg.strip_prefix("--dependency-lib=") {
            overrides
                .dependency_libs
                .push(Library::parse_host(value)?);
        } else if arg == "--allow-unversioned-primary-overwrite" {
            overrides.allow_unversioned_primary_overwrite = true;
        } else if let Some(value) = arg.strip_prefix("--file=") {
            overrides.extra_files.push(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--env=") {
            let value = value.trim().to_string();
            validate_kv_format(&value)?;
            overrides.extra_env.push(value);
        } else if let Some(value) = arg.strip_prefix("--mount=") {
            overrides.extra_mounts.push(parse_cli_mount_spec(value)?);
        } else {
            return Err(Error::message(format!("unsupported argument: {arg}")));
        }
    }

    Ok(overrides)
}

fn resolve_rootfs(root_path: &str) -> Result<PathBuf> {
    let root = Path::new(root_path);
    if root.is_absolute() {
        return Ok(root.to_path_buf());
    }

    Err(Error::message(format!(
        "pc_injection_hook requires an absolute OCI root.path in precreate mode: {root_path}"
    )))
}

fn parse_optional_library_list(var: &'static str) -> Result<Vec<Library>> {
    match env::var_os(var) {
        Some(value) if !value.is_empty() => {
            env::split_paths(&value).map(Library::parse_host).collect()
        }
        _ => Ok(Vec::new()),
    }
}

fn validate_inputs(inputs: &HookInputs) -> Result<()> {
    if inputs.primary_libs.is_empty() && inputs.dependency_libs.is_empty() {
        return Err(Error::message(
            "at least one primary library (--lib or INJECTION_PRIMARY_LIBS) or dependency library (--dependency-lib or INJECTION_DEPENDENCY_LIBS) must be provided",
        ));
    }

    Ok(())
}

fn prefer_cli_vec<T>(cli: Vec<T>, env: Vec<T>) -> Vec<T> {
    if cli.is_empty() { env } else { cli }
}

fn parse_optional_path_list(var: &'static str) -> Vec<PathBuf> {
    match env::var_os(var) {
        Some(value) if !value.is_empty() => env::split_paths(&value).collect(),
        _ => Vec::new(),
    }
}

fn parse_optional_env_specs(var: &'static str) -> Result<Vec<String>> {
    let Some(raw) = env::var_os(var) else {
        return Ok(Vec::new());
    };

    if raw.is_empty() {
        return Ok(Vec::new());
    }

    let raw = raw
        .to_str()
        .ok_or_else(|| Error::message(format!("{var} must contain valid UTF-8")))?;

    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let entries = raw
        .split(';')
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| entry.trim().to_string())
        .collect::<Vec<_>>();

    validate_env_strings(&entries)?;
    Ok(entries)
}

fn parse_optional_mount_specs(var: &'static str) -> Result<Vec<ExtraMountEdit>> {
    let Some(raw) = env::var_os(var) else {
        return Ok(Vec::new());
    };

    if raw.is_empty() {
        return Ok(Vec::new());
    }

    let raw = raw
        .to_str()
        .ok_or_else(|| Error::message(format!("{var} must contain valid UTF-8")))?;

    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    raw.split(';')
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| parse_mount_spec_entry(var, entry.trim()))
        .collect()
}

fn parse_cli_mount_spec(entry: &str) -> Result<ExtraMountEdit> {
    let parts = entry.splitn(3, ':').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err(Error::message(format!(
            "--mount entries must use source:destination:options format: {entry}"
        )));
    }

    let source = canonical_mount_source_path(
        &PathBuf::from(parts[0].trim()),
        "extra mount source",
    )?;
    let destination = PathBuf::from(parts[1].trim());
    let options = if parts[2].trim().is_empty() {
        Vec::new()
    } else {
        parts[2]
            .split(',')
            .map(str::trim)
            .filter(|option| !option.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    };
    validate_mount_destination(&destination)?;
    validate_mount_options(&options)?;

    Ok(ExtraMountEdit {
        source,
        destination,
        mount_type: "bind".to_string(),
        options: strip_non_oci_mount_options(options),
    })
}

fn parse_mount_spec_entry(var: &'static str, entry: &str) -> Result<ExtraMountEdit> {
    let parts = entry.splitn(4, ':').collect::<Vec<_>>();
    if parts.len() != 4 {
        return Err(Error::message(format!(
            "{var} mount entries must use source:destination:type:options format: {entry}"
        )));
    }

    let source = canonical_mount_source_path(
        &PathBuf::from(parts[0].trim()),
        "extra mount source",
    )?;
    let destination = PathBuf::from(parts[1].trim());
    let mount_type = parts[2].trim();
    let options = if parts[3].trim().is_empty() {
        Vec::new()
    } else {
        parts[3]
            .split(',')
            .map(str::trim)
            .filter(|option| !option.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    };
    validate_mount_destination(&destination)?;
    validate_mount_options(&options)?;

    let mount_type = match mount_type {
        "" | "none" | "bind" => "bind".to_string(),
        other => {
            return Err(Error::message(format!(
                "unsupported extra mount type '{other}', only bind-style mounts are supported"
            )))
        }
    };

    Ok(ExtraMountEdit {
        source,
        destination,
        mount_type,
        options: strip_non_oci_mount_options(options),
    })
}

fn strip_non_oci_mount_options(options: Vec<String>) -> Vec<String> {
    options
        .into_iter()
        .filter(|option| option != "x-create=dir")
        .collect()
}

fn validate_mount_options(options: &[String]) -> Result<()> {
    for option in options {
        match option.as_str() {
            "bind" | "rbind" | "ro" | "rw" | "nosuid" | "suid" | "nodev" | "dev" | "noexec"
            | "exec" | "private" | "rprivate" | "slave" | "rslave" | "shared" | "rshared"
            | "x-create=dir" => {}
            other => {
                return Err(Error::message(format!(
                    "unsupported extra mount option '{other}'"
                )))
            }
        }
    }

    Ok(())
}

impl Library {
    // constructor from host libs
    fn parse_host(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        Self::from_name_source(path, None)
    }

    // constructor for container libs (rootfs is used to resolve relative symlinks)
    fn parse_container(path: impl Into<PathBuf>, rootfs: &Path) -> Result<Self> {
        let path = path.into();
        Self::from_name_source(path, Some(rootfs))
    }

    // shared base constructor
    fn from_name_source(path: PathBuf, rootfs: Option<&Path>) -> Result<Self> {
        let name = name_for_parsing(&path, rootfs)?;
        let (linker_name, abi, real_name) = parse_library_name(&name)?;
        Ok(Self {
            path,
            file_name: name,
            linker_name,
            real_name,
            abi,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn file_name(&self) -> &str {
        &self.file_name
    }

    fn linker_name(&self) -> &str {
        &self.linker_name
    }

    fn real_name(&self) -> &str {
        &self.real_name
    }

    fn has_major_version(&self) -> bool {
        self.abi.has_major()
    }

    // Compatibility checks
    fn is_major_compatible_with(&self, other: &Self) -> bool {
        self.linker_name == other.linker_name && self.abi.major == other.abi.major
    }

    fn link_chain_names(file_name: &str) -> Result<Vec<String>> {
        let (linker_name, abi, _) = parse_library_name(file_name)?;
        let mut names = vec![linker_name];
        for component in abi.components().into_iter().flatten() {
            let next = format!(
                "{}.{}",
                names.last().expect("link chain has at least one element"),
                component
            );
            names.push(next);
        }
        Ok(names)
    }
}

// we try to resolve the "real" lib name, fallback is filename
fn name_for_parsing(path: &Path, rootfs: Option<&Path>) -> Result<String> {
    if let Some(rootfs) = rootfs {
        let joined = resolve_in_rootfs(rootfs, path);
        if let Ok(real) = fs::canonicalize(&joined) {
            if real.starts_with(rootfs) {
                return file_name_to_string(&real);
            }
        }
    }
    file_name_to_string(path)
}

fn file_name_to_string(path: &Path) -> Result<String> {
    let name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
        Error::message(format!(
            "shared library path has no valid file name: {}",
            path.display()
        ))
    })?;
    Ok(name.to_string())
}

// this is how we extract linker_name, abi, and real_name
fn parse_library_name(name: &str) -> Result<(String, AbiVersion, String)> {
    let Some(so_idx) = name.find(".so") else {
        return Err(Error::message(format!(
            "shared library name does not contain '.so': {name}"
        )));
    };

    let linker_name = name[..so_idx + 3].to_string();
    let suffix = &name[so_idx + 3..];

    // simplest case, only .so
    if suffix.is_empty() {
        return Ok((
            linker_name.clone(),
            AbiVersion {
                major: None,
                minor: None,
                patch: None,
            },
            linker_name,
        ));
    }

    // basic check that suffix is .so.
    if !suffix.starts_with('.') {
        return Err(Error::message(format!(
            "unsupported shared library suffix in {name}"
        )));
    }

    // we parse each component
    let mut components = suffix[1..].split('.');
    let major = components
        .next()
        .map(parse_component)
        .transpose()?
        .flatten();
    let minor = components
        .next()
        .map(parse_component)
        .transpose()?
        .flatten();
    let patch = components
        .next()
        .map(parse_component)
        .transpose()?
        .flatten();

    if components.next().is_some() {
        return Err(Error::message(format!(
            "unsupported ABI version with more than 3 components in {name}"
        )));
    }

    Ok((
        linker_name.clone(),
        AbiVersion {
            major,
            minor,
            patch,
        },
        name.to_string(),
    ))
}

fn parse_component(component: &str) -> Result<Option<u32>> {
    if component.is_empty() {
        return Ok(None);
    }
    component
        .parse::<u32>()
        .map(Some)
        .map_err(|_| Error::message(format!("invalid ABI version component: {component}")))
}

fn discover_container_libraries(inputs: &HookInputs) -> Result<DiscoveryOutcome> {
    let mut libraries = Vec::new();
    let mut warnings = Vec::new();

    for path in list_dynamic_linker_libraries(&inputs.ldconfig, &inputs.rootfs)? {
        match Library::parse_container(path.clone(), &inputs.rootfs) {
            Ok(lib) => libraries.push(lib),
            Err(error) => {
                push_warning(
                    &mut warnings,
                    format!(
                        "skipping unparseable container library {}: {}",
                        path.display(),
                        error
                    ),
                );
            }
        }
    }

    Ok(DiscoveryOutcome {
        libraries,
        warnings,
    })
}

fn plan_config_edits(inputs: &HookInputs, container_libs: &[Library]) -> Result<ConfigEdits> {
    let container_index = index_container_libraries(container_libs);
    let fallback_dir = PathBuf::from("/run/pc-injection");
    let mut mounts = Vec::new();
    let mut warnings = Vec::new();
    let mut ld_library_path_dirs = Vec::new();

    // check injection has major ABI
    for lib in &inputs.primary_libs {
        validate_regular_source_file(lib.path(), "primary library")?;
        if !lib.has_major_version() && !inputs.allow_unversioned_primary_overwrite {
            return Err(Error::message(format!(
                "primary library {} must contain at least a major ABI number",
                lib.path().display()
            )));
        }
    }

    // here we decide if we replace or add
    for host in &inputs.primary_libs {
        let candidates = container_index
            .get(host.linker_name())
            .cloned()
            .unwrap_or_default();

        if host.has_major_version()
            && !candidates.is_empty()
            && !candidates.iter().any(Library::has_major_version)
        {
            return Err(Error::message(format!(
                "container libraries matching {} must contain at least a major ABI number",
                host.path().display()
            )));
        }

        let decision = choose_primary_mounts(
            host,
            &candidates,
            &fallback_dir,
            inputs.allow_unversioned_primary_overwrite,
        )?;
        append_decision_mounts(
            &mut mounts,
            &mut ld_library_path_dirs,
            &mut warnings,
            decision,
        )?;
    }

    for host in &inputs.dependency_libs {
        validate_regular_source_file(host.path(), "dependency library")?;
        let candidates = container_index
            .get(host.linker_name())
            .cloned()
            .unwrap_or_default();

        let decision = choose_dependency_mounts(host, &candidates, &fallback_dir)?;
        append_decision_mounts(
            &mut mounts,
            &mut ld_library_path_dirs,
            &mut warnings,
            decision,
        )?;
    }

    for file in &inputs.extra_files {
        validate_extra_source_file(file)?;
        validate_mount_destination(file)?;
        mounts.push(MountEdit {
            source: file.clone(),
            destination: file.clone(),
        });
    }

    let mut extra_mounts = inputs.extra_mounts.clone();
    dedupe_extra_mounts(&mut extra_mounts)?;
    validate_mount_conflicts(&mounts, &extra_mounts)?;
    let extra_env = inputs.extra_env.clone();

    // Ensure we do not have duplicated decisions
    dedupe_mounts(&mut mounts)?;
    dedupe_paths(&mut ld_library_path_dirs);

    Ok(ConfigEdits {
        mounts,
        ld_library_path_dirs,
        extra_mounts,
        extra_env,
        warnings,
    })
}

fn append_decision_mounts(
    mounts: &mut Vec<MountEdit>,
    ld_library_path_dirs: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
    decision: MountDecision,
) -> Result<()> {
    for mount in decision.mounts {
        if !mount.destination.starts_with("/run/pc-injection") {
            validate_mount_destination(&mount.destination)?;
        }
        mounts.push(mount);
    }

    if let Some(dir) = decision.ld_library_path_dir {
        ld_library_path_dirs.push(dir);
    }

    for warning in decision.warnings {
        push_warning(warnings, warning);
    }

    Ok(())
}

fn choose_primary_mounts(
    host: &Library,
    candidates: &[Library],
    fallback_dir: &Path,
    allow_unversioned_primary_overwrite: bool,
) -> Result<MountDecision> {
    if !host.has_major_version() {
        if allow_unversioned_primary_overwrite {
            return choose_same_name_mounts(host, candidates, fallback_dir);
        }

        return Err(Error::message(format!(
            "primary library {} must contain at least a major ABI number",
            host.path().display()
        )));
    }

    choose_same_major_mounts(host, candidates, fallback_dir)
}

fn choose_dependency_mounts(
    host: &Library,
    _candidates: &[Library],
    fallback_dir: &Path,
) -> Result<MountDecision> {
    fallback_mount_decision(
        host,
        fallback_dir,
        vec![format!(
            "injecting dependency library {} through LD_LIBRARY_PATH fallback",
            host.path().display()
        )],
    )
}

fn choose_same_major_mounts(
    host: &Library,
    candidates: &[Library],
    fallback_dir: &Path,
) -> Result<MountDecision> {
    let mut warnings = Vec::new();
    let mismatched_candidates = candidates
        .iter()
        .filter(|candidate| candidate.linker_name() == host.linker_name())
        .filter(|candidate| !host.is_major_compatible_with(candidate))
        .map(|candidate| candidate.real_name().to_string())
        .collect::<Vec<_>>();
    if !mismatched_candidates.is_empty() {
        warnings.push(format!(
            "skipping same-name container libraries with different major ABI for {}: {}",
            host.real_name(),
            mismatched_candidates.join(", ")
        ));
    }

    let same_major_candidates = candidates
        .iter()
        .filter(|candidate| host.is_major_compatible_with(candidate))
        .cloned()
        .collect::<Vec<_>>();

    if same_major_candidates.is_empty() {
        warnings.push(format!(
            "no same-major container match found for host library {}; mounting {} into {} with LD_LIBRARY_PATH",
            host.real_name(),
            host.path().display(),
            fallback_dir.display()
        ));
        return fallback_mount_decision(host, fallback_dir, warnings);
    }

    overwrite_mount_decision(host, &same_major_candidates, warnings)
}

fn choose_same_name_mounts(
    host: &Library,
    candidates: &[Library],
    fallback_dir: &Path,
) -> Result<MountDecision> {
    let warnings = vec![format!(
        "unversioned primary overwrite enabled for {}; matching same-name container libraries only",
        host.real_name()
    )];
    let same_name_candidates = candidates
        .iter()
        .filter(|candidate| candidate.linker_name() == host.linker_name())
        .cloned()
        .collect::<Vec<_>>();

    if same_name_candidates.is_empty() {
        let mut warnings = warnings;
        warnings.push(format!(
            "no same-name container match found for unversioned host library {}; mounting {} into {} with LD_LIBRARY_PATH",
            host.real_name(),
            host.path().display(),
            fallback_dir.display()
        ));
        return fallback_mount_decision(host, fallback_dir, warnings);
    }

    overwrite_mount_decision(host, &same_name_candidates, warnings)
}

// Here we build the mountEdit to overwrite lib with host
fn overwrite_mount_decision(
    host: &Library,
    containers: &[Library],
    warnings: Vec<String>,
) -> Result<MountDecision> {
    Ok(MountDecision {
        mounts: containers
            .iter()
            .map(|container| MountEdit {
                source: host.path().to_path_buf(),
                destination: container.path().to_path_buf(),
            })
            .collect(),
        ld_library_path_dir: None,
        warnings,
    })
}

// inject a library through a temporal dir mount containing the right library names as symlinks to the host file, mounts that directory into the container, and tells the dynamic linker to search there
fn fallback_mount_decision(
    host: &Library,
    _dir: &Path,
    warnings: Vec<String>,
) -> Result<MountDecision> {
    let fallback = plan_fallback_staging(host.path(), host.file_name())?;
    let destination = PathBuf::from("/run/pc-injection").join(host.file_name());
    let ld_library_path_dir = Some(destination.clone());
    Ok(MountDecision {
        mounts: vec![
            MountEdit {
                source: fallback.staging_dir,
                destination: destination.clone(),
            },
            MountEdit {
                source: fallback.real_source,
                destination: destination.join(fallback.real_file_name),
            },
        ],
        ld_library_path_dir,
        warnings,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FallbackStaging {
    staging_dir: PathBuf,
    real_source: PathBuf,
    real_file_name: String,
}

fn plan_fallback_staging(source: &Path, requested_name: &str) -> Result<FallbackStaging> {
    let real_source = canonical_library_source(source)?;
    let real_file_name = file_name_to_string(&real_source)?;
    let alias_names = fallback_alias_names(requested_name, &real_file_name)?;
    let staging_dir = create_fallback_staging_dir(&alias_names, &real_file_name)?;

    Ok(FallbackStaging {
        staging_dir,
        real_source,
        real_file_name,
    })
}

fn canonical_library_source(source: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(source).map_err(|e| {
        Error::io(
            format!("failed to stat library source {}", source.display()),
            e,
        )
    })?;

    if metadata.file_type().is_symlink() {
        fs::canonicalize(source).map_err(|e| {
            Error::io(
                format!(
                    "failed to resolve canonical library source {}",
                    source.display()
                ),
                e,
            )
        })
    } else {
        Ok(source.to_path_buf())
    }
}

fn create_fallback_staging_dir(alias_names: &[String], real_file_name: &str) -> Result<PathBuf> {
    let staging_dir = unique_temp_path("staging");
    fs::create_dir_all(&staging_dir)
        .map_err(|e| Error::io(format!("failed to create {}", staging_dir.display()), e))?;

    for name in alias_names {
        let link = staging_dir.join(name);
        symlink(real_file_name, &link).map_err(|e| {
            Error::io(
                format!("failed to create fallback symlink {}", link.display()),
                e,
            )
        })?;
    }

    Ok(staging_dir)
}

fn fallback_alias_names(requested_name: &str, real_file_name: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut seen = HashSet::new();

    for chain in [
        fallback_link_names(requested_name)?,
        fallback_link_names(real_file_name)?,
    ] {
        for name in chain {
            if name != real_file_name && seen.insert(name.clone()) {
                names.push(name);
            }
        }
    }

    Ok(names)
}

fn fallback_link_names(file_name: &str) -> Result<Vec<String>> {
    let chain = Library::link_chain_names(file_name)?;
    match chain.len() {
        0 => Ok(Vec::new()),
        1 => Ok(chain),
        _ => {
            let mut names = Vec::new();
            if let Some(soname) = chain.get(1) {
                names.push(soname.clone());
            }
            if let Some(real_name) = chain.last() {
                if names.last() != Some(real_name) {
                    names.push(real_name.clone());
                }
            }
            Ok(names)
        }
    }
}

fn index_container_libraries(container_libs: &[Library]) -> HashMap<String, Vec<Library>> {
    let mut index = HashMap::<String, Vec<Library>>::new();
    for lib in container_libs {
        index
            .entry(lib.linker_name().to_string())
            .or_default()
            .push(lib.clone());
    }
    index
}

fn list_dynamic_linker_libraries(ldconfig: &Path, rootfs: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new(ldconfig)
        .arg("-r")
        .arg(rootfs)
        .arg("-p")
        .output()
        .map_err(|e| Error::io(format!("failed to execute {}", ldconfig.display()), e))?;

    if !output.status.success() {
        return Err(Error::message(format!(
            "{} -r {} -p failed with status {}",
            ldconfig.display(),
            rootfs.display(),
            output.status
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut libraries = Vec::new();
    for line in stdout.lines() {
        if let Some((_, path)) = line.split_once("=>") {
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                libraries.push(PathBuf::from(trimmed));
            }
        }
    }
    Ok(libraries)
}

fn validate_regular_source_file(source: &Path, label: &str) -> Result<()> {
    let source_metadata = fs::metadata(source)
        .map_err(|e| Error::io(format!("failed to stat {label} {}", source.display()), e))?;
    if !source_metadata.is_file() {
        return Err(Error::message(format!(
            "{label} must be a regular file: {}",
            source.display()
        )));
    }
    Ok(())
}

fn canonical_mount_source_path(source: &Path, label: &str) -> Result<PathBuf> {
    if !source.is_absolute() {
        return Err(Error::message(format!(
            "{label} must be absolute: {}",
            source.display()
        )));
    }

    let canonical = fs::canonicalize(source).map_err(|e| {
        Error::io(
            format!("failed to canonicalize {label} {}", source.display()),
            e,
        )
    })?;
    let source_metadata = fs::metadata(&canonical)
        .map_err(|e| Error::io(format!("failed to stat {label} {}", canonical.display()), e))?;

    if !(source_metadata.is_file() || source_metadata.is_dir()) {
        return Err(Error::message(format!(
            "{label} must resolve to a regular file or directory: {}",
            canonical.display()
        )));
    }

    Ok(canonical)
}

// INJECTION_EXTRA_FILES are raw file mounts and we need those to be exact
fn validate_extra_source_file(source: &Path) -> Result<()> {
    let source_metadata = fs::symlink_metadata(source).map_err(|e| {
        Error::io(
            format!("failed to stat extra-path source {}", source.display()),
            e,
        )
    })?;
    let file_type = source_metadata.file_type();

    if file_type.is_symlink() {
        return Err(Error::message(format!(
            "INJECTION_EXTRA_FILES entries must be regular files, not symlinks: {}",
            source.display()
        )));
    }

    if !file_type.is_file() {
        return Err(Error::message(format!(
            "INJECTION_EXTRA_FILES entries must be regular files: {}",
            source.display()
        )));
    }

    Ok(())
}

fn validate_mount_destination(destination: &Path) -> Result<()> {
    if !destination.is_absolute() {
        return Err(Error::message(format!(
            "mount destination must be absolute: {}",
            destination.display()
        )));
    }

    for component in destination.components() {
        match component {
            Component::Normal(_) | Component::RootDir => {}
            Component::CurDir | Component::ParentDir => {
                return Err(Error::message(format!(
                    "mount destination must not contain '.' or '..' components: {}",
                    destination.display()
                )));
            }
            Component::Prefix(_) => {
                return Err(Error::message(format!(
                    "mount destination must be a Unix-style absolute path: {}",
                    destination.display()
                )));
            }
        }
    }

    if destination.file_name().is_none() {
        return Err(Error::message(format!(
            "mount destination has no valid file name: {}",
            destination.display()
        )));
    }

    Ok(())
}

fn resolve_in_rootfs(rootfs: &Path, container_path: &Path) -> PathBuf {
    match container_path.strip_prefix("/") {
        Ok(relative) => rootfs.join(relative),
        Err(_) => rootfs.join(container_path),
    }
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if !warnings.iter().any(|existing| existing == &warning) {
        warnings.push(warning);
    }
}

fn dedupe_mounts(mounts: &mut Vec<MountEdit>) -> Result<()> {
    let mut seen = HashMap::<PathBuf, PathBuf>::new();
    let mut deduped = Vec::new();

    for mount in mounts.drain(..) {
        match seen.get(&mount.destination) {
            Some(existing) if existing != &mount.source => {
                return Err(Error::message(format!(
                    "conflicting planned mounts for {}: {} vs {}",
                    mount.destination.display(),
                    existing.display(),
                    mount.source.display()
                )));
            }
            Some(_) => {}
            None => {
                seen.insert(mount.destination.clone(), mount.source.clone());
                deduped.push(mount);
            }
        }
    }

    *mounts = deduped;
    Ok(())
}

fn dedupe_extra_mounts(mounts: &mut Vec<ExtraMountEdit>) -> Result<()> {
    let mut seen = HashMap::<PathBuf, (PathBuf, String, Vec<String>)>::new();
    let mut deduped = Vec::new();

    for mount in mounts.drain(..) {
        match seen.get(&mount.destination) {
            Some((existing_source, existing_type, existing_options))
                if existing_source != &mount.source
                    || existing_type != &mount.mount_type
                    || existing_options != &mount.options =>
            {
                return Err(Error::message(format!(
                    "conflicting planned extra mounts for {}",
                    mount.destination.display()
                )))
            }
            Some(_) => {}
            None => {
                seen.insert(
                    mount.destination.clone(),
                    (
                        mount.source.clone(),
                        mount.mount_type.clone(),
                        mount.options.clone(),
                    ),
                );
                deduped.push(mount);
            }
        }
    }

    *mounts = deduped;
    Ok(())
}

fn validate_mount_conflicts(mounts: &[MountEdit], extra_mounts: &[ExtraMountEdit]) -> Result<()> {
    let planned_mounts: HashSet<_> = mounts
        .iter()
        .map(|mount| mount.destination.clone())
        .collect();

    for mount in extra_mounts {
        if planned_mounts.contains(&mount.destination) {
            return Err(Error::message(format!(
                "conflicting planned mounts for {}: destination already used by library injection",
                mount.destination.display()
            )));
        }
    }

    Ok(())
}

fn dedupe_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
}

// Now we apply edit to OCI config
fn apply_config_edits(config: &mut Value, edits: &ConfigEdits) -> Result<()> {
    let obj = config
        .as_object_mut()
        .ok_or_else(|| Error::message("top-level OCI config JSON must be an object"))?;

    if !edits.mounts.is_empty() {
        append_mounts(obj, &edits.mounts)?;
    }

    if !edits.ld_library_path_dirs.is_empty() {
        merge_ld_library_path(obj, &edits.ld_library_path_dirs)?;
    }

    if !edits.extra_mounts.is_empty() {
        append_extra_mounts(obj, &edits.extra_mounts)?;
    }

    if !edits.extra_env.is_empty() {
        merge_process_env_strings(obj, &edits.extra_env)?;
    }

    Ok(())
}

fn append_mounts(obj: &mut Map<String, Value>, mounts_to_add: &[MountEdit]) -> Result<()> {
    let mounts = ensure_array_field(obj, "mounts")?;

    for mount in mounts_to_add {
        let mut out = Map::new();
        out.insert(
            "destination".to_string(),
            Value::String(mount.destination.display().to_string()),
        );
        out.insert("type".to_string(), Value::String("bind".to_string()));
        out.insert(
            "source".to_string(),
            Value::String(mount.source.display().to_string()),
        );
        out.insert(
            "options".to_string(),
            Value::Array(
                ["ro", "rbind", "nosuid", "nodev"]
                    .into_iter()
                    .map(|value| Value::String(value.to_string()))
                    .collect(),
            ),
        );
        mounts.push(Value::Object(out));
    }

    Ok(())
}

fn append_extra_mounts(
    obj: &mut Map<String, Value>,
    mounts_to_add: &[ExtraMountEdit],
) -> Result<()> {
    let mounts = ensure_array_field(obj, "mounts")?;

    for mount in mounts_to_add {
        let mut already_present = false;

        for existing in mounts.iter() {
            match oci_mount_matches_extra_mount(existing, mount)? {
                Some(true) => {
                    already_present = true;
                    break;
                }
                Some(false) => {
                    return Err(Error::message(format!(
                        "conflicting planned extra mounts for {}",
                        mount.destination.display()
                    )))
                }
                None => {}
            }
        }

        if already_present {
            continue;
        }

        let mut out = Map::new();
        out.insert(
            "destination".to_string(),
            Value::String(mount.destination.display().to_string()),
        );
        out.insert("type".to_string(), Value::String(mount.mount_type.clone()));
        out.insert(
            "source".to_string(),
            Value::String(mount.source.display().to_string()),
        );
        out.insert(
            "options".to_string(),
            Value::Array(mount.options.iter().cloned().map(Value::String).collect()),
        );
        mounts.push(Value::Object(out));
    }

    Ok(())
}

fn oci_mount_matches_extra_mount(existing: &Value, mount: &ExtraMountEdit) -> Result<Option<bool>> {
    let Some(obj) = existing.as_object() else {
        return Ok(None);
    };

    let Some(destination) = obj.get("destination").and_then(Value::as_str) else {
        return Ok(None);
    };

    if destination != mount.destination.to_string_lossy() {
        return Ok(None);
    }

    let source = obj.get("source").and_then(Value::as_str).ok_or_else(|| {
        Error::message(format!(
            "existing mount for {} is missing string source",
            mount.destination.display()
        ))
    })?;
    let mount_type = obj.get("type").and_then(Value::as_str).ok_or_else(|| {
        Error::message(format!(
            "existing mount for {} is missing string type",
            mount.destination.display()
        ))
    })?;
    let options = obj
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::message(format!(
                "existing mount for {} is missing array options",
                mount.destination.display()
            ))
        })?
        .iter()
        .map(|value| {
            value.as_str().map(ToString::to_string).ok_or_else(|| {
                Error::message(format!(
                    "existing mount for {} has non-string option",
                    mount.destination.display()
                ))
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Some(
        source == mount.source.to_string_lossy()
            && mount_type == mount.mount_type
            && options == mount.options,
    ))
}

fn merge_ld_library_path(obj: &mut Map<String, Value>, dirs: &[PathBuf]) -> Result<()> {
    let dirs_as_strings = dirs
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();

    // ensuring OCI json has config.process.env entry
    let process_val = obj
        .entry("process".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let process_obj = process_val
        .as_object_mut()
        .ok_or_else(|| Error::message("validation error: 'process' exists but is not an object"))?;
    let env_arr = ensure_array_field(process_obj, "env")?;

    // check if we already got an env with LD_LIBRARY_PATH
    let existing_index = env_arr.iter().rposition(|value| {
        value
            .as_str()
            .and_then(|entry| entry.split_once('=').map(|(key, _)| key))
            .is_some_and(|key| key == "LD_LIBRARY_PATH")
    });
    let existing_entries = existing_index
        .and_then(|idx| env_arr[idx].as_str())
        .and_then(|entry| entry.split_once('=').map(|(_, value)| value.to_string()))
        .unwrap_or_default();

    let mut merged = Vec::new();
    let mut seen = HashSet::new();
    // Add new libs first
    for dir in &dirs_as_strings {
        if seen.insert(dir.clone()) {
            merged.push(dir.clone());
        }
    }
    // Append existing entries
    for dir in existing_entries
        .split(':')
        .filter(|entry| !entry.is_empty())
    {
        if seen.insert(dir.to_string()) {
            merged.push(dir.to_string());
        }
    }

    // Replace or append into env_var
    let value = format!("LD_LIBRARY_PATH={}", merged.join(":"));
    match existing_index {
        Some(idx) => env_arr[idx] = Value::String(value),
        None => env_arr.push(Value::String(value)),
    }

    Ok(())
}

fn validate_env_strings(entries: &[String]) -> Result<()> {
    for entry in entries {
        validate_kv_format(entry)?;
    }

    Ok(())
}

fn validate_kv_format(entry: &str) -> Result<()> {
    if let Some((key, _)) = entry.split_once('=') {
        if key.is_empty() {
            return Err(Error::message("empty environment variable name before '='"));
        }
        Ok(())
    } else {
        Err(Error::message(format!(
            "invalid env entry (expected KEY=VALUE): {entry}"
        )))
    }
}

fn merge_process_env_strings(obj: &mut Map<String, Value>, env_entries: &[String]) -> Result<()> {
    let process_val = obj
        .entry("process".to_string())
        .or_insert_with(|| json!({}));
    let process_obj = process_val
        .as_object_mut()
        .ok_or_else(|| Error::message("validation error: 'process' exists but is not an object"))?;
    let env_arr = ensure_array_field(process_obj, "env")?;

    for new in env_entries {
        let (new_key, _) = new
            .split_once('=')
            .expect("environment entries must be validated before merging");

        if let Some(idx) = env_arr.iter().rposition(|value| {
            value
                .as_str()
                .and_then(|entry| entry.split_once('=').map(|(key, _)| key))
                .is_some_and(|key| key == new_key)
        }) {
            env_arr[idx] = Value::String(new.clone());
        } else {
            env_arr.push(Value::String(new.clone()));
        }
    }

    Ok(())
}

fn ensure_array_field<'a>(
    obj: &'a mut Map<String, Value>,
    field: &str,
) -> Result<&'a mut Vec<Value>> {
    use serde_json::map::Entry;

    match obj.entry(field.to_string()) {
        Entry::Vacant(entry) => {
            let value = entry.insert(Value::Array(Vec::new()));
            Ok(value.as_array_mut().expect("inserted an array"))
        }
        Entry::Occupied(entry) => {
            let value = entry.into_mut();
            match value {
                Value::Array(arr) => Ok(arr),
                _ => Err(Error::message(format!(
                    "validation error: '{field}' exists but is not an array"
                ))),
            }
        }
    }
}

fn unique_temp_path(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("pc_injection_hook-{label}-{nonce}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn fallback_primary_adds_mounts_and_ld_library_path() {
        let temp_root = unique_temp_path("fallback-primary");
        let rootfs = temp_root.join("rootfs");
        let host_file = temp_root.join("host/libmpi.so.12.2");

        fs::create_dir_all(&rootfs).unwrap();
        fs::create_dir_all(host_file.parent().unwrap()).unwrap();
        fs::write(&host_file, b"payload").unwrap();

        let inputs = HookInputs {
            rootfs: rootfs.clone(),
            ldconfig: "ldconfig".into(),
            primary_libs: vec![Library::parse_host(&host_file).unwrap()],
            dependency_libs: Vec::new(),
            allow_unversioned_primary_overwrite: false,
            extra_files: Vec::new(),
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
        };
        let container_libs = vec![Library::parse_host("/usr/lib/libmpi.so.13.4").unwrap()];

        let edits = plan_config_edits(&inputs, &container_libs).unwrap();
        assert_eq!(edits.mounts.len(), 2);
        assert_eq!(
            edits.mounts[0].destination,
            PathBuf::from("/run/pc-injection/libmpi.so.12.2")
        );
        assert_eq!(edits.mounts[1].source, host_file);
        assert_eq!(
            edits.mounts[1].destination,
            PathBuf::from("/run/pc-injection/libmpi.so.12.2/libmpi.so.12.2")
        );
        assert!(fs::symlink_metadata(edits.mounts[0].source.join("libmpi.so.12")).is_ok());
        assert!(fs::symlink_metadata(edits.mounts[0].source.join("libmpi.so.12.2")).is_err());
        assert_eq!(
            edits.ld_library_path_dirs,
            vec![PathBuf::from("/run/pc-injection/libmpi.so.12.2")]
        );
        assert_eq!(
            edits.warnings,
            vec![
                "skipping same-name container libraries with different major ABI for libmpi.so.12.2: libmpi.so.13.4".to_string(),
                format!(
                    "no same-major container match found for host library {}; mounting {} into {} with LD_LIBRARY_PATH",
                    "libmpi.so.12.2",
                    host_file.display(),
                    "/run/pc-injection"
                ),
            ]
        );

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn compatible_primary_overwrites_existing_container_path() {
        let temp_root = unique_temp_path("overwrite-primary");
        let rootfs = temp_root.join("rootfs");
        let host_file = temp_root.join("host/libmpi.so.12.5");

        fs::create_dir_all(rootfs.join("usr/lib64")).unwrap();
        fs::create_dir_all(rootfs.join("opt/vendor")).unwrap();
        fs::create_dir_all(host_file.parent().unwrap()).unwrap();
        fs::write(&host_file, b"payload").unwrap();

        let inputs = HookInputs {
            rootfs,
            ldconfig: "ldconfig".into(),
            primary_libs: vec![Library::parse_host(&host_file).unwrap()],
            dependency_libs: Vec::new(),
            allow_unversioned_primary_overwrite: false,
            extra_files: Vec::new(),
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
        };
        let container_libs = vec![
            Library::parse_host("/usr/lib64/libmpi.so.12.3").unwrap(),
            Library::parse_host("/opt/vendor/libmpi.so.12.7").unwrap(),
            Library::parse_host("/usr/lib64/libmpi.so.11.9").unwrap(),
        ];

        let edits = plan_config_edits(&inputs, &container_libs).unwrap();
        assert_eq!(
            edits.mounts,
            vec![
                MountEdit {
                    source: host_file.clone(),
                    destination: PathBuf::from("/usr/lib64/libmpi.so.12.3"),
                },
                MountEdit {
                    source: host_file.clone(),
                    destination: PathBuf::from("/opt/vendor/libmpi.so.12.7"),
                },
            ]
        );
        assert!(edits.ld_library_path_dirs.is_empty());
        assert_eq!(
            edits.warnings,
            vec![
                "skipping same-name container libraries with different major ABI for libmpi.so.12.5: libmpi.so.11.9".to_string()
            ]
        );

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn primary_warns_for_major_mismatch_and_only_overwrites_same_major_candidates() {
        let temp_root = unique_temp_path("warn-major-mismatch");
        let rootfs = temp_root.join("rootfs");
        let host_file = temp_root.join("host/libmpi.so.12.5");

        fs::create_dir_all(rootfs.join("usr/lib64")).unwrap();
        fs::create_dir_all(rootfs.join("opt/vendor")).unwrap();
        fs::create_dir_all(host_file.parent().unwrap()).unwrap();
        fs::write(&host_file, b"payload").unwrap();

        let inputs = HookInputs {
            rootfs,
            ldconfig: "ldconfig".into(),
            primary_libs: vec![Library::parse_host(&host_file).unwrap()],
            dependency_libs: Vec::new(),
            allow_unversioned_primary_overwrite: false,
            extra_files: Vec::new(),
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
        };
        let container_libs = vec![
            Library::parse_host("/usr/lib64/libmpi.so.12.3").unwrap(),
            Library::parse_host("/opt/vendor/libmpi.so.11.7").unwrap(),
            Library::parse_host("/usr/lib64/libmpi.so.13.1").unwrap(),
        ];

        let edits = plan_config_edits(&inputs, &container_libs).unwrap();
        assert_eq!(
            edits.mounts,
            vec![MountEdit {
                source: host_file.clone(),
                destination: PathBuf::from("/usr/lib64/libmpi.so.12.3"),
            }]
        );
        assert!(edits.ld_library_path_dirs.is_empty());
        assert_eq!(
            edits.warnings,
            vec![
                "skipping same-name container libraries with different major ABI for libmpi.so.12.5: libmpi.so.11.7, libmpi.so.13.1".to_string()
            ]
        );

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn dependency_always_uses_fallback_mount_after_primary_activation() {
        let temp_root = unique_temp_path("overwrite-dependency");
        let rootfs = temp_root.join("rootfs");
        let primary = temp_root.join("host/libmpi.so.12.5");
        let dependency = temp_root.join("host/libhwloc.so.15.2");

        fs::create_dir_all(rootfs.join("usr/lib64")).unwrap();
        fs::create_dir_all(rootfs.join("opt/vendor")).unwrap();
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        fs::write(&primary, b"payload").unwrap();
        fs::write(&dependency, b"payload").unwrap();

        let inputs = HookInputs {
            rootfs,
            ldconfig: "ldconfig".into(),
            primary_libs: vec![Library::parse_host(&primary).unwrap()],
            dependency_libs: vec![Library::parse_host(&dependency).unwrap()],
            allow_unversioned_primary_overwrite: false,
            extra_files: Vec::new(),
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
        };
        let container_libs = vec![
            Library::parse_host("/usr/lib64/libmpi.so.12.3").unwrap(),
            Library::parse_host("/usr/lib64/libhwloc.so.15.0").unwrap(),
            Library::parse_host("/opt/vendor/libhwloc.so.15.9").unwrap(),
            Library::parse_host("/usr/lib64/libhwloc.so.14.8").unwrap(),
        ];

        let edits = plan_config_edits(&inputs, &container_libs).unwrap();
        assert_eq!(
            edits.mounts,
            vec![
                MountEdit {
                    source: primary.clone(),
                    destination: PathBuf::from("/usr/lib64/libmpi.so.12.3"),
                },
                MountEdit {
                    source: edits.mounts[1].source.clone(),
                    destination: PathBuf::from("/run/pc-injection/libhwloc.so.15.2"),
                },
                MountEdit {
                    source: dependency.clone(),
                    destination: PathBuf::from(
                        "/run/pc-injection/libhwloc.so.15.2/libhwloc.so.15.2"
                    ),
                },
            ]
        );
        assert_eq!(
            edits.ld_library_path_dirs,
            vec![PathBuf::from("/run/pc-injection/libhwloc.so.15.2")]
        );
        assert_eq!(
            edits.warnings,
            vec![format!(
                "injecting dependency library {} through LD_LIBRARY_PATH fallback",
                dependency.display()
            )]
        );
        assert!(fs::symlink_metadata(edits.mounts[1].source.join("libhwloc.so.15")).is_ok());
        assert!(fs::symlink_metadata(edits.mounts[1].source.join("libhwloc.so.15.2")).is_err());

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn symlink_dependency_stages_real_file_and_relative_alias() {
        let temp_root = unique_temp_path("symlink-dependency");
        let primary = temp_root.join("host/libmpi.so.12.5");
        let dependency_real = temp_root.join("host/libcxi.so.1.5.0");
        let dependency_link = temp_root.join("host/libcxi.so.1");

        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        fs::write(&primary, b"payload").unwrap();
        fs::write(&dependency_real, b"payload").unwrap();
        symlink("libcxi.so.1.5.0", &dependency_link).unwrap();

        let fallback = plan_fallback_staging(&dependency_link, "libcxi.so.1").unwrap();
        assert_eq!(fallback.real_source, dependency_real);
        assert_eq!(fallback.real_file_name, "libcxi.so.1.5.0");
        assert_eq!(
            fs::read_link(fallback.staging_dir.join("libcxi.so.1")).unwrap(),
            PathBuf::from("libcxi.so.1.5.0")
        );
        assert!(fs::symlink_metadata(fallback.staging_dir.join("libcxi.so.1.5.0")).is_err());

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn config_edits_merge_ld_library_path_and_mounts() {
        let mut config = serde_json::json!({
            "root": { "path": "/rootfs" },
            "mounts": [],
            "process": {
                "env": ["FOO=BAR", "LD_LIBRARY_PATH=/usr/lib64"]
            }
        });
        let edits = ConfigEdits {
            mounts: vec![MountEdit {
                source: PathBuf::from("/host/libmpi.so.12"),
                destination: PathBuf::from("/lib/libmpi.so.12"),
            }],
            ld_library_path_dirs: vec![PathBuf::from("/lib")],
            extra_mounts: vec![ExtraMountEdit {
                source: PathBuf::from("/var/spool/slurmd"),
                destination: PathBuf::from("/var/spool/slurmd"),
                mount_type: "bind".to_string(),
                options: vec![
                    "bind".to_string(),
                    "rw".to_string(),
                    "nosuid".to_string(),
                    "nodev".to_string(),
                ],
            }],
            extra_env: vec!["MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000".to_string()],
            warnings: Vec::new(),
        };

        apply_config_edits(&mut config, &edits).unwrap();
        let env = config["process"]["env"].as_array().unwrap();
        assert!(env
            .iter()
            .any(|value| value == "LD_LIBRARY_PATH=/lib:/usr/lib64"));
        assert!(env
            .iter()
            .any(|value| { value == "MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000" }));
        let mounts = config["mounts"].as_array().unwrap();
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0]["type"], "bind");
        assert_eq!(mounts[0]["destination"], "/lib/libmpi.so.12");
        assert_eq!(mounts[1]["destination"], "/var/spool/slurmd");
        assert_eq!(
            mounts[1]["options"],
            serde_json::json!(["bind", "rw", "nosuid", "nodev"])
        );
    }

    #[test]
    fn apply_config_edits_skips_identical_existing_extra_mount() {
        let mut config = serde_json::json!({
            "root": { "path": "/rootfs" },
            "mounts": [{
                "destination": "/var/spool/slurmd",
                "type": "bind",
                "source": "/host/slurmd",
                "options": ["bind", "rw", "nosuid", "nodev"]
            }]
        });
        let edits = ConfigEdits {
            mounts: Vec::new(),
            ld_library_path_dirs: Vec::new(),
            extra_mounts: vec![ExtraMountEdit {
                source: PathBuf::from("/host/slurmd"),
                destination: PathBuf::from("/var/spool/slurmd"),
                mount_type: "bind".to_string(),
                options: vec![
                    "bind".to_string(),
                    "rw".to_string(),
                    "nosuid".to_string(),
                    "nodev".to_string(),
                ],
            }],
            extra_env: Vec::new(),
            warnings: Vec::new(),
        };

        apply_config_edits(&mut config, &edits).unwrap();

        let mounts = config["mounts"].as_array().unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0]["destination"], "/var/spool/slurmd");
    }

    #[test]
    fn apply_config_edits_rejects_conflicting_existing_extra_mount() {
        let mut config = serde_json::json!({
            "root": { "path": "/rootfs" },
            "mounts": [{
                "destination": "/var/spool/slurmd",
                "type": "bind",
                "source": "/host/other-slurmd",
                "options": ["bind", "rw", "nosuid", "nodev"]
            }]
        });
        let edits = ConfigEdits {
            mounts: Vec::new(),
            ld_library_path_dirs: Vec::new(),
            extra_mounts: vec![ExtraMountEdit {
                source: PathBuf::from("/host/slurmd"),
                destination: PathBuf::from("/var/spool/slurmd"),
                mount_type: "bind".to_string(),
                options: vec![
                    "bind".to_string(),
                    "rw".to_string(),
                    "nosuid".to_string(),
                    "nodev".to_string(),
                ],
            }],
            extra_env: Vec::new(),
            warnings: Vec::new(),
        };

        let error = apply_config_edits(&mut config, &edits).unwrap_err();
        assert!(error
            .to_string()
            .contains("conflicting planned extra mounts for /var/spool/slurmd"));
    }

    #[test]
    fn discovery_warns_for_unparseable_container_library() {
        let temp_root = unique_temp_path("requested-parse");
        let rootfs = temp_root.join("rootfs");
        let host_file = temp_root.join("host/libmpi.so.12");
        let ldconfig = temp_root.join("fake-ldconfig.sh");

        fs::create_dir_all(&rootfs).unwrap();
        fs::create_dir_all(host_file.parent().unwrap()).unwrap();
        fs::write(&host_file, b"payload").unwrap();
        fs::write(
            &ldconfig,
            "#!/bin/sh\nprintf '%s\n' 'libmpi.so (libc6,x86-64) => /usr/lib/libmpi.so.bad.suffix'\n",
        )
        .unwrap();
        fs::set_permissions(&ldconfig, fs::Permissions::from_mode(0o755)).unwrap();

        let inputs = HookInputs {
            rootfs,
            ldconfig,
            primary_libs: vec![Library::parse_host(&host_file).unwrap()],
            dependency_libs: Vec::new(),
            allow_unversioned_primary_overwrite: false,
            extra_files: Vec::new(),
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
        };

        let discovery = discover_container_libraries(&inputs).unwrap();
        assert!(discovery.libraries.is_empty());
        assert_eq!(discovery.warnings.len(), 1);
        assert!(discovery.warnings[0].contains("skipping unparseable container library"));

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn extra_files_require_regular_sources() {
        let temp_root = unique_temp_path("extra-files");
        let rootfs = temp_root.join("rootfs");
        let primary = temp_root.join("host/libmpi.so.12");
        let extra = temp_root.join("opt/tools/tool.sh");

        fs::create_dir_all(rootfs.join("usr/lib")).unwrap();
        fs::create_dir_all(resolve_in_rootfs(&rootfs, extra.parent().unwrap())).unwrap();
        fs::create_dir_all(extra.parent().unwrap()).unwrap();
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        fs::write(&primary, b"payload").unwrap();
        fs::write(&extra, b"#!/bin/sh\n").unwrap();

        let inputs = HookInputs {
            rootfs,
            ldconfig: "ldconfig".into(),
            primary_libs: vec![Library::parse_host(&primary).unwrap()],
            dependency_libs: Vec::new(),
            allow_unversioned_primary_overwrite: false,
            extra_files: vec![extra.clone()],
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
        };
        let container_libs = vec![Library::parse_host("/usr/lib/libmpi.so.12.1").unwrap()];

        let edits = plan_config_edits(&inputs, &container_libs).unwrap();
        assert!(edits.mounts.iter().any(|mount| mount.destination == extra));

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn relative_root_path_is_rejected() {
        let error = resolve_rootfs("rootfs").unwrap_err();
        assert!(error
            .to_string()
            .contains("requires an absolute OCI root.path"));
    }

    #[test]
    fn parse_optional_env_specs_accepts_semicolon_separated_entries() {
        std::env::set_var(
            "INJECTION_EXTRA_ENV",
            "FOO=bar;MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000",
        );

        let entries = parse_optional_env_specs("INJECTION_EXTRA_ENV").unwrap();
        assert_eq!(
            entries,
            vec![
                "FOO=bar".to_string(),
                "MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000".to_string()
            ]
        );

        std::env::remove_var("INJECTION_EXTRA_ENV");
    }

    #[test]
    fn parse_optional_mount_specs_normalizes_bind_mounts() {
        let temp_root = unique_temp_path("extra-mount-spec");
        let mount_source = temp_root.join("var/spool/slurmd");
        fs::create_dir_all(&mount_source).unwrap();

        std::env::set_var(
            "INJECTION_EXTRA_MOUNTS",
            format!(
                "{}:/var/spool/slurmd:none:x-create=dir,bind,rw,nosuid,noexec,nodev,private",
                mount_source.display()
            ),
        );

        let mounts = parse_optional_mount_specs("INJECTION_EXTRA_MOUNTS").unwrap();
        assert_eq!(
            mounts,
            vec![ExtraMountEdit {
                source: mount_source.clone(),
                destination: PathBuf::from("/var/spool/slurmd"),
                mount_type: "bind".to_string(),
                options: vec![
                    "bind".to_string(),
                    "rw".to_string(),
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                    "private".to_string(),
                ],
            }]
        );

        std::env::remove_var("INJECTION_EXTRA_MOUNTS");
        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn parse_optional_mount_specs_canonicalizes_symlink_sources() {
        let temp_root = unique_temp_path("extra-mount-symlink");
        let real_source = temp_root.join("real/slurmd");
        let symlink_source = temp_root.join("link/slurmd");

        fs::create_dir_all(&real_source).unwrap();
        fs::create_dir_all(symlink_source.parent().unwrap()).unwrap();
        symlink(&real_source, &symlink_source).unwrap();

        std::env::set_var(
            "INJECTION_EXTRA_MOUNTS",
            format!(
                "{}:/var/spool/slurmd:bind:bind,rw,nosuid,noexec,nodev,private",
                symlink_source.display()
            ),
        );

        let mounts = parse_optional_mount_specs("INJECTION_EXTRA_MOUNTS").unwrap();
        assert_eq!(
            mounts,
            vec![ExtraMountEdit {
                source: fs::canonicalize(&real_source).unwrap(),
                destination: PathBuf::from("/var/spool/slurmd"),
                mount_type: "bind".to_string(),
                options: vec![
                    "bind".to_string(),
                    "rw".to_string(),
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                    "private".to_string(),
                ],
            }]
        );

        std::env::remove_var("INJECTION_EXTRA_MOUNTS");
        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn parse_cli_mount_spec_accepts_source_destination_options() {
        let temp_root = unique_temp_path("cli-mount-spec");
        let mount_source = temp_root.join("var/spool/slurmd");
        fs::create_dir_all(&mount_source).unwrap();

        let mount = parse_cli_mount_spec(&format!(
            "{}:/var/spool/slurmd:bind,rw,nosuid,noexec,nodev,private",
            mount_source.display()
        ))
        .unwrap();

        assert_eq!(
            mount,
            ExtraMountEdit {
                source: mount_source,
                destination: PathBuf::from("/var/spool/slurmd"),
                mount_type: "bind".to_string(),
                options: vec![
                    "bind".to_string(),
                    "rw".to_string(),
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                    "private".to_string(),
                ],
            }
        );

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn cli_overrides_parse_repeated_entries() {
        let temp_root = unique_temp_path("cli-overrides");
        let primary = temp_root.join("host/libmpi.so.12.5");
        let dependency = temp_root.join("host/libpcitest.so.1.0.0");
        let extra_file = temp_root.join("etc/libibverbs.d/mlx5.driver");
        let extra_mount_source = temp_root.join("var/spool/slurmd");
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        fs::create_dir_all(extra_file.parent().unwrap()).unwrap();
        fs::create_dir_all(&extra_mount_source).unwrap();
        fs::write(&primary, b"payload").unwrap();
        fs::write(&dependency, b"payload").unwrap();
        fs::write(&extra_file, b"driver mlx5\n").unwrap();

        let overrides = parse_cli_overrides_from_args(vec![
            "--ldconfig=/sbin/ldconfig".into(),
            format!("--lib={}", primary.display()).into(),
            format!("--dependency-lib={}", dependency.display()).into(),
            format!("--file={}", extra_file.display()).into(),
            "--env=MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000".into(),
            format!(
                "--mount={}:/var/spool/slurmd:bind,rw,nosuid,noexec,nodev,private",
                extra_mount_source.display()
            )
            .into(),
        ])
        .unwrap();

        assert_eq!(overrides.ldconfig, Some(PathBuf::from("/sbin/ldconfig")));
        assert_eq!(overrides.primary_libs, vec![Library::parse_host(&primary).unwrap()]);
        assert_eq!(
            overrides.dependency_libs,
            vec![Library::parse_host(&dependency).unwrap()]
        );
        assert!(!overrides.allow_unversioned_primary_overwrite);
        assert_eq!(overrides.extra_files, vec![extra_file]);
        assert_eq!(
            overrides.extra_env,
            vec!["MPIR_CVAR_CH4_OFI_MULTI_NIC_STRIPING_THRESHOLD=100000000".to_string()]
        );
        assert_eq!(
            overrides.extra_mounts,
            vec![ExtraMountEdit {
                source: extra_mount_source,
                destination: PathBuf::from("/var/spool/slurmd"),
                mount_type: "bind".to_string(),
                options: vec![
                    "bind".to_string(),
                    "rw".to_string(),
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                    "private".to_string(),
                ],
            }]
        );

        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn load_inputs_prefers_cli_values_over_env() {
        let temp_root = unique_temp_path("load-inputs-cli-precedence");
        let rootfs = temp_root.join("rootfs");
        let cli_primary = temp_root.join("host/libmpi.so.12.5");
        let env_primary = temp_root.join("host/libenvmpi.so.9.1");
        let cli_file = temp_root.join("etc/libibverbs.d/mlx5.driver");
        let env_file = temp_root.join("etc/libibverbs.d/env.driver");
        let cli_mount_source = temp_root.join("var/spool/slurmd");
        let env_mount_source = temp_root.join("var/lib/hugetlbfs");
        fs::create_dir_all(&rootfs).unwrap();
        fs::create_dir_all(cli_primary.parent().unwrap()).unwrap();
        fs::create_dir_all(cli_file.parent().unwrap()).unwrap();
        fs::create_dir_all(env_file.parent().unwrap()).unwrap();
        fs::create_dir_all(&cli_mount_source).unwrap();
        fs::create_dir_all(&env_mount_source).unwrap();
        fs::write(&cli_primary, b"payload").unwrap();
        fs::write(&env_primary, b"payload").unwrap();
        fs::write(&cli_file, b"driver mlx5\n").unwrap();
        fs::write(&env_file, b"driver env\n").unwrap();

        std::env::set_var("LDCONFIG_PATH", "/env/ldconfig");
        std::env::set_var("INJECTION_PRIMARY_LIBS", env_primary.as_os_str());
        std::env::set_var("INJECTION_EXTRA_FILES", env_file.as_os_str());
        std::env::set_var(
            "INJECTION_EXTRA_ENV",
            "ENV_ONLY_SHOULD_BE_IGNORED=1",
        );
        std::env::set_var(
            "INJECTION_EXTRA_MOUNTS",
            format!(
                "{}:/var/lib/hugetlbfs:bind:bind,rw,nosuid,nodev,private",
                env_mount_source.display()
            ),
        );

        let config = serde_json::json!({
            "root": { "path": rootfs.display().to_string() }
        });
        let cli = parse_cli_overrides_from_args(vec![
            "--ldconfig=/cli/ldconfig".into(),
            format!("--lib={}", cli_primary.display()).into(),
            format!("--file={}", cli_file.display()).into(),
            "--env=CLI_WINS=1".into(),
            format!(
                "--mount={}:/var/spool/slurmd:bind,rw,nosuid,noexec,nodev,private",
                cli_mount_source.display()
            )
            .into(),
        ])
        .unwrap();

        let inputs = load_inputs_from_sources(&config, cli).unwrap();

        assert_eq!(inputs.ldconfig, PathBuf::from("/cli/ldconfig"));
        assert_eq!(
            inputs.primary_libs,
            vec![Library::parse_host(&cli_primary).unwrap()]
        );
        assert!(!inputs.allow_unversioned_primary_overwrite);
        assert_eq!(inputs.extra_files, vec![cli_file]);
        assert_eq!(inputs.extra_env, vec!["CLI_WINS=1".to_string()]);
        assert_eq!(
            inputs.extra_mounts,
            vec![ExtraMountEdit {
                source: cli_mount_source,
                destination: PathBuf::from("/var/spool/slurmd"),
                mount_type: "bind".to_string(),
                options: vec![
                    "bind".to_string(),
                    "rw".to_string(),
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                    "private".to_string(),
                ],
            }]
        );

        std::env::remove_var("LDCONFIG_PATH");
        std::env::remove_var("INJECTION_PRIMARY_LIBS");
        std::env::remove_var("INJECTION_EXTRA_FILES");
        std::env::remove_var("INJECTION_EXTRA_ENV");
        std::env::remove_var("INJECTION_EXTRA_MOUNTS");
        fs::remove_dir_all(&temp_root).unwrap();
    }

    #[test]
    fn validate_inputs_allows_dependency_only_and_rejects_empty_library_inputs() {
        let base = HookInputs {
            rootfs: PathBuf::from("/rootfs"),
            ldconfig: "ldconfig".into(),
            primary_libs: Vec::new(),
            dependency_libs: Vec::new(),
            allow_unversioned_primary_overwrite: false,
            extra_files: Vec::new(),
            extra_mounts: Vec::new(),
            extra_env: Vec::new(),
        };

        let error = validate_inputs(&base).unwrap_err();
        assert_eq!(
            error.to_string(),
            "at least one primary library (--lib or INJECTION_PRIMARY_LIBS) or dependency library (--dependency-lib or INJECTION_DEPENDENCY_LIBS) must be provided"
        );

        let temp_root = unique_temp_path("validate-inputs-dependency-only");
        let dependency = temp_root.join("host/libcxi.so.1.5.0");
        fs::create_dir_all(dependency.parent().unwrap()).unwrap();
        fs::write(&dependency, b"payload").unwrap();

        let dependency_only = HookInputs {
            dependency_libs: vec![Library::parse_host(&dependency).unwrap()],
            ..base
        };

        assert!(validate_inputs(&dependency_only).is_ok());

        fs::remove_dir_all(&temp_root).unwrap();
    }
}
