use serde_json::{Map, Value};
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
    extra_files: Vec<PathBuf>,
    _compatibility_policy: CompatibilityPolicy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompatibilityPolicy {
    Major,
    Full,
    Strict,
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
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MountEdit {
    source: PathBuf,
    destination: PathBuf,
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
    let root_path = config
        .get("root")
        .and_then(Value::as_object)
        .and_then(|root| root.get("path"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::message("OCI config is missing root.path"))?;

    Ok(HookInputs {
        rootfs: resolve_rootfs(root_path)?,
        ldconfig: PathBuf::from(env::var_os("LDCONFIG_PATH").unwrap_or_else(|| "ldconfig".into())),
        primary_libs: parse_required_library_list("INJECTION_PRIMARY_LIBS")?,
        dependency_libs: parse_optional_library_list("INJECTION_DEPENDENCY_LIBS")?,
        extra_files: parse_optional_path_list("INJECTION_EXTRA_FILES"),
        _compatibility_policy: CompatibilityPolicy::from_env("INJECTION_COMPATIBILITY")?,
    })
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

fn parse_required_library_list(var: &'static str) -> Result<Vec<Library>> {
    let raw = env::var_os(var).ok_or_else(|| {
        Error::message(format!(
            "the environment variable {var} is expected to be a non-empty colon-separated list of paths"
        ))
    })?;

    let paths: Vec<_> = env::split_paths(&raw).collect();
    if paths.is_empty() {
        return Err(Error::message(format!(
            "the environment variable {var} is expected to be a non-empty colon-separated list of paths"
        )));
    }

    paths.into_iter().map(Library::parse_host).collect()
}

fn parse_optional_library_list(var: &'static str) -> Result<Vec<Library>> {
    match env::var_os(var) {
        Some(value) if !value.is_empty() => {
            env::split_paths(&value).map(Library::parse_host).collect()
        }
        _ => Ok(Vec::new()),
    }
}

fn parse_optional_path_list(var: &'static str) -> Vec<PathBuf> {
    match env::var_os(var) {
        Some(value) if !value.is_empty() => env::split_paths(&value).collect(),
        _ => Vec::new(),
    }
}

impl CompatibilityPolicy {
    fn from_env(var: &'static str) -> Result<Self> {
        match env::var(var) {
            Ok(value) => Self::parse(&value),
            Err(_) => Ok(Self::Major),
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "major" => Ok(Self::Major),
            "full" => Ok(Self::Full),
            "strict" => Ok(Self::Strict),
            other => Err(Error::message(format!(
                "unsupported compatibility policy '{other}'"
            ))),
        }
    }
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

    // at least 1 library to inject needs to exist in container
    if !inputs
        .primary_libs
        .iter()
        .any(|lib| container_index.contains_key(lib.linker_name()))
    {
        return Err(Error::message(
            "failed to activate library injection: no primary libraries found in the container linker cache",
        ));
    }

    // check injection has major ABI
    for lib in &inputs.primary_libs {
        validate_regular_source_file(lib.path(), "primary library")?;
        if !lib.has_major_version() {
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

        if !candidates.is_empty() && !candidates.iter().any(Library::has_major_version) {
            return Err(Error::message(format!(
                "container libraries matching {} must contain at least a major ABI number",
                host.path().display()
            )));
        }

        let decision = choose_primary_mounts(host, &candidates, &fallback_dir)?;
        append_decision_mounts(
            &mut mounts,
            &mut ld_library_path_dirs,
            &mut warnings,
            &inputs.rootfs,
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
            &inputs.rootfs,
            decision,
        )?;
    }

    for file in &inputs.extra_files {
        validate_extra_source_file(file)?;
        validate_mount_destination(&inputs.rootfs, file)?;
        mounts.push(MountEdit {
            source: file.clone(),
            destination: file.clone(),
        });
    }

    dedupe_mounts(&mut mounts)?;
    dedupe_paths(&mut ld_library_path_dirs);

    Ok(ConfigEdits {
        mounts,
        ld_library_path_dirs,
        warnings,
    })
}

fn append_decision_mounts(
    mounts: &mut Vec<MountEdit>,
    ld_library_path_dirs: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
    rootfs: &Path,
    decision: MountDecision,
) -> Result<()> {
    for mount in decision.mounts {
        if !mount.destination.starts_with("/run/pc-injection") {
            validate_mount_destination(rootfs, &mount.destination)?;
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
) -> Result<MountDecision> {
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
        return fallback_mount_decision(
            host,
            fallback_dir,
            warnings,
        );
    }

    overwrite_mount_decision(host, &same_major_candidates, warnings)
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
    let names = fallback_link_names(host.file_name())?;
    let source = create_fallback_staging_dir(host.path(), &names)?;
    let destination = PathBuf::from("/run/pc-injection").join(host.file_name());
    let ld_library_path_dir = Some(destination.clone());
    Ok(MountDecision {
        mounts: vec![MountEdit {
            source,
            destination,
        }],
        ld_library_path_dir,
        warnings,
    })
}

fn create_fallback_staging_dir(source: &Path, names: &[String]) -> Result<PathBuf> {
    let staging_dir = unique_temp_path("staging");
    fs::create_dir_all(&staging_dir)
        .map_err(|e| Error::io(format!("failed to create {}", staging_dir.display()), e))?;

    for name in names {
        let link = staging_dir.join(name);
        symlink(source, &link).map_err(|e| {
            Error::io(
                format!("failed to create fallback symlink {}", link.display()),
                e,
            )
        })?;
    }

    Ok(staging_dir)
}

fn fallback_link_names(file_name: &str) -> Result<Vec<String>> {
    let chain = Library::link_chain_names(file_name)?;
    if chain.len() <= 1 {
        return Ok(chain);
    }

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

fn validate_mount_destination(rootfs: &Path, destination: &Path) -> Result<()> {
    let rootfs_real = fs::canonicalize(rootfs)
        .map_err(|e| Error::io(format!("failed to resolve rootfs {}", rootfs.display()), e))?;
    let relative = normalize_container_relative_path(destination)?;
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let parent_real = resolve_existing_directory_in_rootfs(&rootfs_real, parent)?;
    if !parent_real.starts_with(&rootfs_real) {
        return Err(Error::message(format!(
            "mount destination escapes the rootfs: {}",
            destination.display()
        )));
    }

    let target = parent_real.join(relative.file_name().ok_or_else(|| {
        Error::message(format!(
            "mount destination has no file name: {}",
            destination.display()
        ))
    })?);
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        let file_type = metadata.file_type();
        if file_type.is_dir() && !file_type.is_symlink() {
            return Err(Error::message(format!(
                "mount destination already exists as a directory: {}",
                destination.display()
            )));
        }
    }

    Ok(())
}

fn normalize_container_relative_path(destination: &Path) -> Result<PathBuf> {
    let relative = match destination.strip_prefix("/") {
        Ok(relative) => relative,
        Err(_) => destination,
    };

    let mut normalized = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir | Component::ParentDir => {
                return Err(Error::message(format!(
                    "mount destination must not contain '.' or '..' components: {}",
                    destination.display()
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::message(format!(
                    "mount destination must be a normalized container path: {}",
                    destination.display()
                )));
            }
        }
    }

    if normalized.file_name().is_none() {
        return Err(Error::message(format!(
            "mount destination has no valid file name: {}",
            destination.display()
        )));
    }

    Ok(normalized)
}

fn resolve_existing_directory_in_rootfs(rootfs: &Path, relative: &Path) -> Result<PathBuf> {
    let mut current = rootfs.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .map_err(|e| Error::io(format!("failed to inspect {}", current.display()), e))?;
        let file_type = metadata.file_type();
        if !file_type.is_dir() && !file_type.is_symlink() {
            return Err(Error::message(format!(
                "mount destination parent is not a directory: {}",
                current.display()
            )));
        }

        current = fs::canonicalize(&current)
            .map_err(|e| Error::io(format!("failed to resolve {}", current.display()), e))?;
        if !current.starts_with(rootfs) {
            return Err(Error::message(format!(
                "mount destination escapes the rootfs through {}",
                current.display()
            )));
        }
    }

    Ok(current)
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

fn dedupe_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
}

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

fn merge_ld_library_path(obj: &mut Map<String, Value>, dirs: &[PathBuf]) -> Result<()> {
    let dirs_as_strings = dirs
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();

    let process_val = obj
        .entry("process".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let process_obj = process_val
        .as_object_mut()
        .ok_or_else(|| Error::message("validation error: 'process' exists but is not an object"))?;
    let env_arr = ensure_array_field(process_obj, "env")?;

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
    for dir in &dirs_as_strings {
        if seen.insert(dir.clone()) {
            merged.push(dir.clone());
        }
    }
    for dir in existing_entries
        .split(':')
        .filter(|entry| !entry.is_empty())
    {
        if seen.insert(dir.to_string()) {
            merged.push(dir.to_string());
        }
    }

    let value = format!("LD_LIBRARY_PATH={}", merged.join(":"));
    match existing_index {
        Some(idx) => env_arr[idx] = Value::String(value),
        None => env_arr.push(Value::String(value)),
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
            extra_files: Vec::new(),
            _compatibility_policy: CompatibilityPolicy::Full,
        };
        let container_libs = vec![Library::parse_host("/usr/lib/libmpi.so.13.4").unwrap()];

        let edits = plan_config_edits(&inputs, &container_libs).unwrap();
        assert_eq!(edits.mounts.len(), 1);
        assert_eq!(
            edits.mounts[0].destination,
            PathBuf::from("/run/pc-injection/libmpi.so.12.2")
        );
        assert!(fs::symlink_metadata(edits.mounts[0].source.join("libmpi.so.12")).is_ok());
        assert!(fs::symlink_metadata(edits.mounts[0].source.join("libmpi.so.12.2")).is_ok());
        assert_eq!(
            edits.ld_library_path_dirs,
            vec![PathBuf::from("/run/pc-injection/libmpi.so.12.2")]
        );
        assert_eq!(
            edits.warnings,
            vec![
                "skipping same-name container libraries with different major ABI for libmpi.so.12.2: libmpi.so.13.4".to_string(),
                format!(
                    "no same-major container match found for host library {}; mounting {} into {} with LD_LIBRARY_PATH fallback",
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
            extra_files: Vec::new(),
            _compatibility_policy: CompatibilityPolicy::Major,
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
            extra_files: Vec::new(),
            _compatibility_policy: CompatibilityPolicy::Major,
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
            extra_files: Vec::new(),
            _compatibility_policy: CompatibilityPolicy::Full,
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
        assert!(fs::symlink_metadata(edits.mounts[1].source.join("libhwloc.so.15.2")).is_ok());

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
            warnings: Vec::new(),
        };

        apply_config_edits(&mut config, &edits).unwrap();
        let env = config["process"]["env"].as_array().unwrap();
        assert!(env
            .iter()
            .any(|value| value == "LD_LIBRARY_PATH=/lib:/usr/lib64"));
        let mounts = config["mounts"].as_array().unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0]["type"], "bind");
        assert_eq!(mounts[0]["destination"], "/lib/libmpi.so.12");
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
            extra_files: Vec::new(),
            _compatibility_policy: CompatibilityPolicy::Major,
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
            extra_files: vec![extra.clone()],
            _compatibility_policy: CompatibilityPolicy::Major,
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
}
