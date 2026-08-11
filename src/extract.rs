//! `extract` subcommand: orchestrates the full Rust-to-Lean pipeline.
//!
//! ## Error model
//!
//! Functions return [`Result<T, ExtractError>`]. The enum is deliberately
//! lean — only `ThreadPanicked` carries structured info; everything else
//! is either propagation from a submodule (`ExtractRunner`, `Listfuns`)
//! or `Other(anyhow::Error)` for context-chained ad-hoc errors and the
//! `String` bridge from `merge_atom_files` (upstream `probe` crate, not yet
//! migrated) via `.map_err(anyhow::Error::msg)`.
//!
//! CLI input-validation messages and YAML/JSON parse errors flow through
//! `.context("...")?` or `anyhow::anyhow!("...").into()` — the typed enum
//! stays tight, anyhow does the heavy lifting for context.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use probe::commands::merge::merge_atom_files;
use probe::types::{Atom, InputProvenance, MergedAtomEnvelope, Tool};
use serde::Deserialize;

use crate::aeneas_config::AeneasConfig;
use crate::enrich;
use crate::extract_runner::{self, ExtractRunnerError};
use crate::function_source::{self, RecordSource};
use crate::gen_functions::write_functions_json;
use crate::listfuns::ListfunsError;
use crate::translate::{build_translations_json, generate_translations, load_atoms};
use crate::translation_manifest;
use crate::types::FunctionRecord;

type MappingMaps = (HashMap<String, Vec<String>>, HashMap<String, Vec<String>>);

// ---------------------------------------------------------------------------
// Typed error
// ---------------------------------------------------------------------------

/// Errors produced by the `extract` module.
///
/// Only `ThreadPanicked` carries structured fields. All other errors flow
/// through `#[from]` propagation or the `Other(anyhow::Error)` catch-all,
/// which lets `.context()` chains do the work of attaching messages.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// A parallel extraction thread panicked while joining.
    #[error("{language} extraction thread panicked")]
    ThreadPanicked { language: &'static str },

    /// Errors propagated from `extract_runner`.
    #[error(transparent)]
    ExtractRunner(#[from] ExtractRunnerError),

    /// Errors propagated from `listfuns`.
    #[error(transparent)]
    Listfuns(#[from] ListfunsError),

    /// Catch-all wrapping `anyhow::Error`. Used for:
    /// - io::Error / serde_json::Error / serde_yaml::Error via `.context()?`
    /// - ad-hoc CLI input validation messages via `anyhow::anyhow!(...).into()`
    /// - `Result<_, String>` from `merge_atom_files` (upstream `probe` crate)
    ///   via `.map_err(anyhow::Error::msg)?`
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout this module.
pub type Result<T> = std::result::Result<T, ExtractError>;

// ---------------------------------------------------------------------------
// aeneas-config.yml parsing (minimal: only fields probe-aeneas needs)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AeneasProjectConfig {
    #[serde(rename = "crate")]
    crate_config: CrateConfig,
    aeneas_args: Option<AeneasArgsConfig>,
    charon: Option<CharonConfig>,
}

#[derive(Debug, Deserialize)]
struct CrateConfig {
    dir: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AeneasArgsConfig {
    dest: Option<String>,
    #[allow(dead_code)]
    backend: Option<String>,
    #[allow(dead_code)]
    options: Option<Vec<String>>,
}

/// Charon configuration parsed from `aeneas-config.yml`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CharonConfig {
    pub preset: Option<String>,
    pub package: Option<String>,
    pub cargo_args: Option<Vec<String>>,
    pub start_from: Option<Vec<String>>,
    pub start_from_pub: Option<bool>,
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
    pub opaque: Option<Vec<String>>,
}

/// Resolved paths derived from an Aeneas project directory.
#[derive(Debug)]
pub struct ResolvedProject {
    pub rust_project: PathBuf,
    pub lean_project: PathBuf,
    pub functions_json: Option<PathBuf>,
    /// Aeneas `translation.json` (from the `emit-json` arg), if present at the
    /// configured `aeneas_args.dest` (default: project root). Used as the
    /// authoritative loop/primary classification overlay.
    pub translation_json: Option<PathBuf>,
    /// Effective crate directory relative to the project root.
    /// `"."` when the Rust project IS the project root (including workspace
    /// layouts where probe-rust runs at the workspace root).
    /// Non-`"."` for standalone subdirectory crates, where Rust atom
    /// `code-path` values need prefixing to become project-relative.
    pub crate_dir: String,
    /// Charon configuration from `aeneas-config.yml`, used to pre-generate
    /// the LLBC file with the correct cargo args, start_from, and exclude lists.
    pub charon_config: Option<CharonConfig>,
}

/// Check whether a directory contains a Cargo workspace (its `Cargo.toml`
/// has a `[workspace]` table). A quick text check avoids pulling in a TOML
/// parser just for this.
fn is_cargo_workspace(dir: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
        return false;
    };
    content.lines().any(|l| l.trim() == "[workspace]")
}

/// Resolve the target package name from available config sources, in priority
/// order: `charon.package`, `-p`/`--package` in `charon.cargo_args`, then
/// `crate.name` with Cargo's underscore-to-hyphen convention.
fn resolve_target_package_name(
    crate_name: Option<&str>,
    charon_config: Option<&CharonConfig>,
) -> Option<String> {
    if let Some(pkg) = charon_config.and_then(|c| c.package.as_deref()) {
        return Some(pkg.to_string());
    }

    if let Some(args) = charon_config.and_then(|c| c.cargo_args.as_deref()) {
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if arg == "-p" || arg == "--package" {
                if let Some(name) = iter.next() {
                    return Some(name.clone());
                }
            }
            if let Some(name) = arg
                .strip_prefix("-p=")
                .or_else(|| arg.strip_prefix("--package="))
            {
                return Some(name.to_string());
            }
        }
    }

    crate_name.map(|n| n.replace('_', "-"))
}

/// Check whether `cargo_args` already contains `-p` or `--package`.
fn has_package_in_cargo_args(config: &CharonConfig) -> bool {
    let Some(ref args) = config.cargo_args else {
        return false;
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if (arg == "-p" || arg == "--package") && iter.next().is_some() {
            return true;
        }
        if arg.starts_with("-p=") || arg.starts_with("--package=") {
            return true;
        }
    }
    false
}

/// Resolve a workspace member crate directory using `cargo metadata`.
///
/// Given a workspace root, finds the member matching the target package name
/// and returns `(member_directory, relative_path_from_project_root)`. Used
/// by `resolve_project()` for validation (confirming the package exists).
fn resolve_workspace_member(
    project: &Path,
    crate_name: Option<&str>,
    charon_config: Option<&CharonConfig>,
) -> Result<(PathBuf, String)> {
    let target = resolve_target_package_name(crate_name, charon_config).ok_or_else(|| {
        anyhow::anyhow!(
            "Workspace at {} but cannot determine target package \
             (need crate.name, charon.package, or -p in charon.cargo_args)",
            project.display()
        )
    })?;

    let output = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(project)
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output()
        .context("spawn `cargo metadata`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("cargo metadata failed:\n{stderr}").into());
    }

    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parse `cargo metadata` output")?;

    let packages = metadata["packages"]
        .as_array()
        .context("cargo metadata: missing 'packages' array")?;

    let normalized_target = target.replace('-', "_");

    for pkg in packages {
        let name = pkg["name"].as_str().unwrap_or("");
        if name.replace('-', "_") != normalized_target {
            continue;
        }

        let manifest_path = pkg["manifest_path"]
            .as_str()
            .context("cargo metadata: package missing manifest_path")?;
        let member_dir = Path::new(manifest_path)
            .parent()
            .context("Invalid manifest_path")?;

        let project_abs = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
        let member_abs =
            std::fs::canonicalize(member_dir).unwrap_or_else(|_| member_dir.to_path_buf());

        let rel = member_abs.strip_prefix(&project_abs).map_err(|_| {
            anyhow::anyhow!(
                "Workspace member {} is not under project root {}",
                member_dir.display(),
                project.display()
            )
        })?;

        let crate_dir = rel.to_string_lossy().to_string();
        let crate_dir = if crate_dir.is_empty() {
            ".".to_string()
        } else {
            crate_dir
        };

        println!(
            "  Resolved workspace member {:?} at {}",
            name,
            member_dir.display()
        );

        return Ok((member_dir.to_path_buf(), crate_dir));
    }

    Err(anyhow::anyhow!(
        "Package {target:?} not found in workspace at {}",
        project.display()
    )
    .into())
}

/// Collect the feature names passed via `--features`/`-F` in `cargo_args`.
///
/// Handles `--features a,b`, `--features=a,b`, `-F a`, and `-F=a`; values are
/// split on commas and whitespace.
fn parse_explicit_features(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = args.iter().peekable();
    let split = |s: &str, out: &mut Vec<String>| {
        out.extend(
            s.split([',', ' '])
                .filter(|t| !t.is_empty())
                .map(str::to_string),
        );
    };
    while let Some(arg) = iter.next() {
        if arg == "--features" || arg == "-F" {
            if let Some(val) = iter.next() {
                split(val, &mut out);
            }
        } else if let Some(val) = arg.strip_prefix("--features=") {
            split(val, &mut out);
        } else if let Some(val) = arg.strip_prefix("-F=") {
            split(val, &mut out);
        } else if let Some(val) = arg.strip_prefix("-F") {
            // Attached short form `-Ffoo` (the exact `-F` and `-F=` cases are
            // handled above, so `val` here is a non-empty, non-`=` value).
            split(val, &mut out);
        }
    }
    out
}

/// Resolve the active cargo feature set for the Aeneas build, so cfg predicates
/// on Rust atoms can be evaluated (KB P25). Uses `cargo metadata` to read the
/// target package's declared features, computes the default-feature closure,
/// then overlays `charon.cargo_args` (`--no-default-features`, `--all-features`,
/// `--features`).
///
/// Returns `None` when the feature set cannot be determined (cargo metadata
/// unavailable, or an ambiguous workspace with no target package). Callers then
/// skip cfg-based scope classification entirely — conservative, never disabling
/// a backlog atom on a guess.
pub fn resolve_active_features(
    rust_project: &Path,
    charon_config: Option<&CharonConfig>,
) -> Option<crate::cfg_eval::CfgConfig> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(rust_project)
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let packages = metadata["packages"].as_array()?;

    // Select the target package: by resolved name when known, else the sole
    // package. An ambiguous workspace without a target name is unresolvable.
    let target = resolve_target_package_name(None, charon_config);
    let pkg = match &target {
        Some(t) => {
            let nt = t.replace('-', "_");
            packages
                .iter()
                .find(|p| p["name"].as_str().unwrap_or("").replace('-', "_") == nt)?
        }
        None if packages.len() == 1 => &packages[0],
        None => return None,
    };

    let features_obj = pkg["features"].as_object()?;
    let edges: HashMap<String, Vec<String>> = features_obj
        .iter()
        .map(|(k, v)| {
            let list = v
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            (k.clone(), list)
        })
        .collect();

    let args: Vec<String> = charon_config
        .and_then(|c| c.cargo_args.clone())
        .unwrap_or_default();

    let features = resolve_feature_set(&edges, &args);

    Some(crate::cfg_eval::CfgConfig { features })
}

/// Resolve the active feature set from the package's feature graph and the
/// charon cargo args. Seeds with the feature NAMED "default" (not its
/// contents): code can legally gate on `#[cfg(feature = "default")]`, and the
/// closure expands the name through its edges anyway.
fn resolve_feature_set(
    edges: &HashMap<String, Vec<String>>,
    args: &[String],
) -> std::collections::HashSet<String> {
    if args.iter().any(|a| a == "--all-features") {
        return edges.keys().cloned().collect();
    }
    let mut seeds: Vec<String> = if args.iter().any(|a| a == "--no-default-features") {
        Vec::new()
    } else if edges.contains_key("default") {
        vec!["default".to_string()]
    } else {
        Vec::new()
    };
    seeds.extend(parse_explicit_features(args));
    crate::cfg_eval::feature_closure(edges, &seeds)
}

/// Parse `aeneas-config.yml` in the given project directory and derive
/// the Rust project path, Lean project path, and optional functions.json.
pub fn resolve_project(project: &Path) -> Result<ResolvedProject> {
    let config_path = project.join("aeneas-config.yml");
    if !config_path.exists() {
        return Err(anyhow::anyhow!(
            "No aeneas-config.yml found in {}\n\
             Expected an Aeneas project directory containing aeneas-config.yml.\n\
             Use --rust-project / --lean-project for manual input.",
            project.display()
        )
        .into());
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("read {}", config_path.display()))?;
    let mut config: AeneasProjectConfig = serde_yaml::from_str(&content)
        .with_context(|| format!("parse {}", config_path.display()))?;

    let raw_crate_dir = &config.crate_config.dir;
    let (rust_project, crate_dir) = if raw_crate_dir == "." {
        (project.to_path_buf(), ".".to_string())
    } else {
        let candidate = project.join(raw_crate_dir);
        if candidate.join("Cargo.toml").exists() {
            (candidate, raw_crate_dir.clone())
        } else if is_cargo_workspace(project) {
            // Workspace layout: crate.dir doesn't have its own Cargo.toml
            // (e.g. libsignal with crate.dir = "rust" and workspace root at
            // project). Validate the member exists, but use the workspace
            // root as rust_project so probe-rust indexes all crates.
            resolve_workspace_member(
                project,
                config.crate_config.name.as_deref(),
                config.charon.as_ref(),
            )?;
            // Backfill charon package so ensure_charon_llbc() emits
            // --package when running at the workspace root.
            if let Some(ref mut charon) = config.charon {
                if charon.package.is_none() && !has_package_in_cargo_args(charon) {
                    charon.package =
                        resolve_target_package_name(config.crate_config.name.as_deref(), None);
                }
            } else if let Some(pkg) =
                resolve_target_package_name(config.crate_config.name.as_deref(), None)
            {
                config.charon = Some(CharonConfig {
                    package: Some(pkg),
                    ..Default::default()
                });
            }
            (project.to_path_buf(), ".".to_string())
        } else {
            return Err(anyhow::anyhow!(
                "No Cargo.toml found at {} (derived from crate.dir = {:?} in aeneas-config.yml)",
                candidate.display(),
                raw_crate_dir,
            )
            .into());
        }
    };
    let lean_project = project.to_path_buf();

    if !rust_project.join("Cargo.toml").exists() {
        return Err(anyhow::anyhow!(
            "No Cargo.toml found at {} (derived from crate.dir = {:?} in aeneas-config.yml)",
            rust_project.display(),
            raw_crate_dir,
        )
        .into());
    }

    if !lean_project.join("lakefile.toml").exists() && !lean_project.join("lakefile.lean").exists()
    {
        return Err(anyhow::anyhow!(
            "No lakefile.toml or lakefile.lean found in {}\n\
             The project root should be a Lean/Lake project.",
            lean_project.display()
        )
        .into());
    }

    if let Some(name) = &config.crate_config.name {
        println!("Aeneas project: crate {:?} at {}", name, project.display());
    } else {
        println!("Aeneas project: {}", project.display());
    }
    println!("  Rust project: {}", rust_project.display());
    println!("  Lean project: {}", lean_project.display());

    if let Some(args) = &config.aeneas_args {
        if let Some(dest) = &args.dest {
            println!("  Aeneas dest:  {dest}");
        }
    }

    let functions_path = project.join("functions.json");
    let functions_json = if functions_path.exists() {
        println!(
            "  Using existing functions.json from {}",
            functions_path.display()
        );
        Some(functions_path)
    } else {
        None
    };

    // translation.json is written by Aeneas to `aeneas_args.dest` (default: the
    // project root). Optional: absent when the project doesn't emit it. The
    // actual load + overlay (with success/warning reporting) happens later in
    // `translation_manifest::apply`, so we only resolve the path here.
    let translation_json = translation_manifest::resolve_path(project);

    Ok(ResolvedProject {
        rust_project,
        lean_project,
        functions_json,
        translation_json,
        crate_dir,
        charon_config: config.charon,
    })
}

/// Run the full extract pipeline with flexible input resolution.
///
/// Accepts either pre-generated JSON paths or project paths for Rust and Lean.
/// When project paths are given, the corresponding extractors are run automatically.
///
/// When `use_lake` is true, `lake exe listfuns` is used to generate
/// `functions.json` (requires the Lean project to define a `listfuns`
/// executable). Otherwise, Aeneas-generated `.lean` files are parsed directly.
#[allow(clippy::too_many_arguments)]
pub fn run_extract(
    rust_json: Option<&Path>,
    rust_project: Option<&Path>,
    lean_json: Option<&Path>,
    lean_project: Option<&Path>,
    functions_json: Option<&Path>,
    translation_json: Option<&Path>,
    output_path: Option<&Path>,
    aeneas_config: Option<&Path>,
    use_lake: bool,
    rust_path_prefix: Option<&str>,
    with_public_api: bool,
    skip_enrich: bool,
    cfg_config: Option<&crate::cfg_eval::CfgConfig>,
) -> Result<()> {
    // --- Validate inputs ---
    if rust_json.is_none() && rust_project.is_none() {
        return Err(anyhow::anyhow!(
            "No Rust input provided. Use one of:\n  \
             probe-aeneas extract <project_path>          (auto-detect from aeneas-config.yml)\n  \
             probe-aeneas extract --rust-project <path>   (Rust project directory)\n  \
             probe-aeneas extract --rust <json>            (pre-generated atoms JSON)"
        )
        .into());
    }
    if lean_json.is_none() && lean_project.is_none() {
        return Err(anyhow::anyhow!(
            "No Lean input provided. Use one of:\n  \
             probe-aeneas extract <project_path>          (auto-detect from aeneas-config.yml)\n  \
             probe-aeneas extract --lean-project <path>   (Lean project directory)\n  \
             probe-aeneas extract --lean <json>            (pre-generated atoms JSON)"
        )
        .into());
    }
    if functions_json.is_none() && lean_project.is_none() {
        return Err(anyhow::anyhow!(
            "--functions is required when --lean-project is not given \
             (cannot auto-generate functions.json without a Lean project path)"
        )
        .into());
    }

    // --- Resolve inputs (extract if needed) ---
    // When both --lean and --lean-project are given, skip Lean extraction
    // (use the pre-computed JSON) but keep the project dir for listfuns.
    let (rust_path, lean_path) = resolve_inputs(
        rust_json,
        rust_project,
        lean_json,
        lean_project,
        with_public_api,
        translation_json,
    )?;

    // --- Resolve function records (single source dispatch) ---
    let resolved =
        function_source::resolve(lean_project, functions_json, translation_json, use_lake)?;

    // Preserve the `functions.json` artifact when probe-aeneas produced the
    // records itself. `Override` already has a file on disk (the user's), and
    // `Lake` wrote `<lean_project>/functions.json`; the in-memory sources
    // (`Manifest`, `LegacyScrape`) need serializing — but only when we have a
    // destination. The `Manifest` arm can resolve from `translation.json` with
    // no `lean_project`, in which case there's nowhere to write and we skip.
    if matches!(
        resolved.source,
        RecordSource::Manifest | RecordSource::LegacyScrape
    ) {
        if let Some(lean_proj) = lean_project {
            write_functions_json(&resolved.records, &lean_proj.join("functions.json"))
                .map_err(anyhow::Error::new)
                .context("write functions.json")?;
        }
    }

    // --- Load Aeneas config (optional) ---
    let config = AeneasConfig::load(aeneas_config, lean_project).context("load aeneas config")?;

    // --- Generate translations ---
    let translations_result = run_translate(
        &rust_path,
        &lean_path,
        &resolved.records,
        resolved.charon_version.as_deref(),
    )?;
    let aux_defs = resolved.aux_defs;

    // --- Merge atom maps ---
    run_extract_with_translations(
        &rust_path,
        &lean_path,
        &translations_result,
        cfg_config,
        &aux_defs,
        output_path,
        &config,
        rust_path_prefix,
        lean_project,
        skip_enrich,
    )
}

/// Resolve Rust and Lean inputs, running extractors in parallel when both are
/// project paths.
///
/// When `lean_project` is available, intermediate extractor outputs are saved
/// to `<lean_project>/.verilib/probes/` alongside the final merged output.
#[allow(clippy::too_many_arguments)]
fn resolve_inputs(
    rust_json: Option<&Path>,
    rust_project: Option<&Path>,
    lean_json: Option<&Path>,
    lean_project: Option<&Path>,
    with_public_api: bool,
    translation_json: Option<&Path>,
) -> Result<(PathBuf, PathBuf)> {
    let need_rust_extract = rust_json.is_none();
    // When --lean is given (pre-computed JSON), skip Lean extraction even if
    // --lean-project is also present.
    let need_lean_extract = lean_json.is_none();

    let probes_dir = lean_project.map(|p| p.join(".verilib").join("probes"));
    let probes_dir_ref = probes_dir.as_deref();

    if need_rust_extract && need_lean_extract {
        let rust_proj = rust_project.unwrap();
        let lean_proj = lean_project.unwrap();

        if let Some(dir) = probes_dir_ref {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }

        println!("Extracting Rust and Lean atoms in parallel...\n");
        let (rust_result, lean_result) = std::thread::scope(|s| {
            let rust_handle = s.spawn(|| {
                extract_runner::run_probe_rust_extract(
                    rust_proj,
                    probes_dir_ref,
                    with_public_api,
                    translation_json,
                )
            });
            let lean_handle =
                s.spawn(|| extract_runner::run_probe_lean_extract(lean_proj, probes_dir_ref));
            (rust_handle.join(), lean_handle.join())
        });

        // The first `?` handles the panic (Result<_, Box<dyn Any>>) by
        // mapping it to a typed ThreadPanicked. The second `?` propagates
        // the inner ExtractRunnerError via #[from].
        let rust_path =
            rust_result.map_err(|_| ExtractError::ThreadPanicked { language: "Rust" })??;
        let lean_path =
            lean_result.map_err(|_| ExtractError::ThreadPanicked { language: "Lean" })??;
        Ok((rust_path, lean_path))
    } else {
        let rust_path = if let Some(json) = rust_json {
            json.to_path_buf()
        } else {
            if let Some(dir) = probes_dir_ref {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("create {}", dir.display()))?;
            }
            extract_runner::run_probe_rust_extract(
                rust_project.unwrap(),
                probes_dir_ref,
                with_public_api,
                translation_json,
            )?
        };

        let lean_path = if let Some(json) = lean_json {
            json.to_path_buf()
        } else {
            if let Some(dir) = probes_dir_ref {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("create {}", dir.display()))?;
            }
            extract_runner::run_probe_lean_extract(lean_project.unwrap(), probes_dir_ref)?
        };

        Ok((rust_path, lean_path))
    }
}

/// Run the translate step, returning bidirectional cross-language maps.
///
/// `functions` are the resolved records (source-blind: manifest-built or
/// legacy-scraped, already carrying any `translation.json` overlay).
/// `manifest_charon_version` provenance-gates the charon-`def_id` join (see
/// [`generate_translations`]).
fn run_translate(
    rust_path: &Path,
    lean_path: &Path,
    functions: &[FunctionRecord],
    manifest_charon_version: Option<&str>,
) -> Result<MappingMaps> {
    println!("Loading Rust atoms from {}...", rust_path.display());
    let rust_data = load_atoms(rust_path)
        .with_context(|| format!("load Rust atoms from {}", rust_path.display()))?;
    println!("  {} atoms", rust_data.len());

    println!("Loading Lean atoms from {}...", lean_path.display());
    let lean_data = load_atoms(lean_path)
        .with_context(|| format!("load Lean atoms from {}", lean_path.display()))?;
    println!("  {} atoms", lean_data.len());

    println!("\nGenerating translations...");
    let (mappings, stats) =
        generate_translations(&rust_data, &lean_data, functions, manifest_charon_version);

    println!("  {} translations generated", mappings.len());
    for (conf, count) in &stats.by_confidence {
        println!("    {conf}: {count}");
    }

    let mut from_to: HashMap<String, Vec<String>> = HashMap::new();
    let mut to_from: HashMap<String, Vec<String>> = HashMap::new();
    for m in &mappings {
        from_to
            .entry(m.from.clone())
            .or_default()
            .push(m.to.clone());
        to_from
            .entry(m.to.clone())
            .or_default()
            .push(m.from.clone());
    }

    Ok((from_to, to_from))
}

/// Merge atoms with pre-computed translations and produce the final output.
///
/// The pipeline has three clearly separated phases:
/// 1. **Merge** — generic `probe merge` operation via `merge_atom_files`.
/// 2. **Enrich** — Aeneas-specific metadata (`translation-*`, `untracked`).
/// 3. **Write** — envelope construction and output.
///
/// When `output_path` is `None`, writes to
/// `<project_root>/.verilib/probes/aeneas_{package}_{version}.json`
/// (matching the probe ecosystem convention). Falls back to the current
/// directory when no project root is available.
#[allow(clippy::too_many_arguments)]
fn run_extract_with_translations(
    rust_path: &Path,
    lean_path: &Path,
    translations: &MappingMaps,
    cfg_config: Option<&crate::cfg_eval::CfgConfig>,
    aux_defs: &HashSet<String>,
    output_path: Option<&Path>,
    config: &AeneasConfig,
    rust_path_prefix: Option<&str>,
    project_root: Option<&Path>,
    skip_enrich: bool,
) -> Result<()> {
    warn_on_old_probe_rust(rust_path);
    // Phase 1: Merge (generic probe operation)
    // merge_atom_files still returns Result<_, String>; bridge via anyhow.
    println!("\nMerging atoms with translations...");
    let (mut merged, provenance, stats) =
        merge_atom_files(&[rust_path, lean_path], Some(translations))
            .map_err(anyhow::Error::msg)
            .context("merge atom files")?;

    let output_path_buf;
    let output_path = match output_path {
        Some(p) => p,
        None => {
            output_path_buf = default_output_path(&provenance, project_root);
            &output_path_buf
        }
    };

    let rust_crate_name = provenance
        .iter()
        .find(|p| p.source.language == "rust")
        .map(|p| p.source.package.as_str())
        .unwrap_or("");

    // Phase 1.5: Prefix Rust code-paths with crate directory when the Rust
    // crate lives in a subdirectory of the repository root (e.g. crate.dir =
    // "curve25519-dalek" → "src/foo.rs" becomes "curve25519-dalek/src/foo.rs").
    if let Some(prefix) = rust_path_prefix {
        prefix_rust_code_paths(&mut merged, prefix);
    }

    // Phase 2: Enrich (Aeneas-specific)
    enrich_with_aeneas_metadata(
        &mut merged,
        &translations.0,
        cfg_config,
        &config.out_of_scope,
    );
    enrich::enrich_lean_atom_flags(&mut merged, rust_crate_name, config, aux_defs);

    // Phase 2.5: Enrich verification status (transitive propagation, P23)
    if !skip_enrich {
        let (transitive, local, _) =
            probe::commands::propagate::enrich_verification_status(&mut merged);
        println!("  enrich: ✓ {transitive} transitively-verified, {local} locally verified");
    }

    // Phase 3: Write envelope
    write_aeneas_envelope(merged, provenance, output_path, &stats)
}

/// Prefix `code-path` on Rust atoms so paths are relative to the repository
/// root rather than the Rust crate root.
///
/// When the Rust crate is a subdirectory of the Aeneas project (e.g.
/// `crate.dir = "curve25519-dalek"`), probe-rust produces crate-relative
/// paths like `src/backend/mod.rs`. This function prepends the crate
/// directory so the final output uses `curve25519-dalek/src/backend/mod.rs`,
/// matching the file paths stored when the full repository is ingested.
fn prefix_rust_code_paths(merged: &mut std::collections::BTreeMap<String, Atom>, prefix: &str) {
    for atom in merged.values_mut() {
        if atom.language == "rust" && !atom.code_path.is_empty() {
            atom.code_path = format!("{prefix}/{}", atom.code_path);
        }
    }
}

/// Resolve the `verification-status` to propagate onto a Rust atom.
///
/// - `"trusted"` / `"failed"` on the Lean def are preserved as-is.
/// - Otherwise, the primary spec theorem is looked up: if found, its
///   `verification-status` is used; if absent, `"unverified"` is returned.
fn resolve_verification_status(
    lean_name: &str,
    lean_atom: &Atom,
    atoms: &std::collections::BTreeMap<String, Atom>,
) -> serde_json::Value {
    let lean_vs = lean_atom
        .extensions
        .get("verification-status")
        .and_then(|v| v.as_str());

    match lean_vs {
        Some("trusted") | Some("failed") => lean_atom.extensions["verification-status"].clone(),
        _ => {
            let stripped = enrich::strip_prefix(lean_name);
            let (_, spec_atom) = enrich::find_primary_spec(stripped, atoms);
            match spec_atom.and_then(|s| s.extensions.get("verification-status")) {
                Some(vs) => vs.clone(),
                None => serde_json::json!("unverified"),
            }
        }
    }
}

/// Add Aeneas-specific metadata to merged atoms, and set `untracked` per the
/// two-state scope model (KB P24/P25).
///
/// Two enrichment passes:
/// 1. For each Rust atom with a Lean translation, set `translation-name`,
///    `translation-path`, `translation-text`, and `verification-status`
///    (spec-based) from the Lean atom and its primary spec — **unless** the
///    translation carries `@[out_of_scope]`, in which case no status is set
///    (an out-of-scope atom carries no `verification-status`).
/// 2. For every Rust atom, default `untracked` to `false` (tracked backlog),
///    and set it to `true` only when the atom has **no** (string-typed)
///    `verification-status` **and** it is genuinely out of scope: a foreign
///    declaration (`is-foreign`), in a file no lib/bin `mod` chain reaches
///    (`is-unmounted`), cfg-inactive in the Aeneas build (its complete `cfg`
///    predicate evaluates definitively false; `file-cfg` only refines the
///    reason), its translation is `@[out_of_scope]`, a non-library target, or
///    config-curated out of scope. A status-bearing atom is never disabled
///    (P24); disagreements between a status and out-of-scope source facts are
///    counted and reported. Membership in `functions.json` no longer affects
///    scope — a compiled function Aeneas has not translated is unverified
///    backlog, not out of scope.
///
/// `cfg_config` is `None` when the active feature set could not be resolved; cfg
/// scope classification is then skipped entirely (conservative — never disables
/// a backlog atom on a guess). `out_of_scope_patterns` is the project config's
/// curated glob list (matched against `rust-qualified-name` / `display-name`)
/// for functions Aeneas structurally does not translate.
///
/// The source facts this pass evaluates come from probe-rust (>= 0.10.0) on
/// each atom: `cfg` (the complete gating predicate, parent-file mod-chain
/// gates included), `file-cfg` (the chain component alone, for reason
/// granularity), `is-unmounted` (no `mod` chain from the package's lib/bin
/// roots reaches the file), and `is-foreign` (extern-block member). Older
/// probe-rust output simply lacks the fields; classification then degrades to
/// the per-function `cfg` evaluation alone (never guesses).
fn enrich_with_aeneas_metadata(
    merged: &mut std::collections::BTreeMap<String, Atom>,
    from_to: &HashMap<String, Vec<String>>,
    cfg_config: Option<&crate::cfg_eval::CfgConfig>,
    out_of_scope_patterns: &[String],
) {
    let enrichments: Vec<_> = from_to
        .iter()
        .flat_map(|(rust_name, lean_names)| {
            lean_names.iter().filter_map(|lean_name| {
                merged.get(lean_name).map(|lean_atom| {
                    (
                        rust_name.clone(),
                        lean_name.clone(),
                        lean_atom.code_path.clone(),
                        lean_atom.code_text.lines_start,
                        lean_atom.code_text.lines_end,
                        resolve_verification_status(lean_name, lean_atom, merged),
                        enrich::is_out_of_scope(&enrich::atom_attrs(lean_atom)),
                    )
                })
            })
        })
        .collect();

    // Rust atom keys whose Lean translation is annotated `@[out_of_scope]`.
    let mut out_of_scope_rust: HashSet<String> = HashSet::new();

    for (rust_name, lean_name, path, start, end, vs, out_of_scope) in enrichments {
        if out_of_scope {
            out_of_scope_rust.insert(rust_name.clone());
        }
        if let Some(atom) = merged.get_mut(&rust_name) {
            atom.extensions
                .insert("translation-name".to_string(), serde_json::json!(lean_name));
            atom.extensions
                .insert("translation-path".to_string(), serde_json::json!(path));
            if start > 0 && end > 0 {
                atom.extensions.insert(
                    "translation-text".to_string(),
                    serde_json::json!({
                        "lines-start": start,
                        "lines-end": end,
                    }),
                );
            }
            // An out-of-scope translation carries no verification-status
            // (P24: has-status ⟹ ¬untracked).
            if !out_of_scope {
                atom.extensions
                    .insert("verification-status".to_string(), vs);
            }
        }
    }

    // Precompile the curated out-of-scope globs once (matched against each Rust
    // atom's rust-qualified-name / display-name below).
    let oos_globs: Vec<regex::Regex> = out_of_scope_patterns
        .iter()
        .filter_map(|p| enrich::glob_to_regex(p))
        .collect();

    // Diagnostics: source facts and statuses can disagree (stale facts
    // against fresher Lean progress, or a malformed producer). P24 decides
    // the outcome, but the disagreement itself must be visible.
    let mut stale_fact_conflicts = 0usize;
    let mut malformed_facts = 0usize;

    for (key, atom) in merged.iter_mut() {
        if atom.language != "rust" {
            continue;
        }
        // Only a string-typed status counts: a stray `null` or malformed
        // value must not shield an atom from scope classification.
        let has_status = atom
            .extensions
            .get("verification-status")
            .and_then(|v| v.as_str())
            .is_some();
        let is_inactive_ext = |field: &str| {
            cfg_config.is_some_and(|cfg| {
                atom.extensions
                    .get(field)
                    .and_then(|v| v.as_str())
                    .is_some_and(|pred| cfg.is_inactive(pred))
            })
        };
        // The complete gating predicate (own gate, same-file enclosing gates,
        // and the parent-file mod-chain gates probe-rust folds in) is false
        // under the Aeneas build's feature set: not compiled, out of scope
        // (KB P25). This is the SOLE cfg classification authority.
        let cfg_inactive = is_inactive_ext("cfg");
        // The chain component alone is false: the whole FILE is gated off at
        // the module level. Never classifies on its own (`cfg` already
        // contains this gate) — it only refines the reason string.
        let file_cfg_inactive = cfg_inactive && is_inactive_ext("file-cfg");
        // No `mod` chain from the package's lib/bin roots reaches the file:
        // rustc never compiles it into any lib or bin build (KB P25).
        // Configuration-independent, so no cfg evaluation is involved.
        let unmounted = atom
            .extensions
            .get("is-unmounted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // A bodyless declaration (extern-block member): the implementation
        // lives outside Rust, so there is nothing to verify here (KB P25).
        let foreign = atom
            .extensions
            .get("is-foreign")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let out_of_scope = out_of_scope_rust.contains(key);
        // A present-but-mistyped fact is indistinguishable from absence to
        // the classifier (conservative), but it means a broken producer or a
        // corrupted cache — count it.
        for field in ["is-unmounted", "is-foreign", "trait-required"] {
            if atom
                .extensions
                .get(field)
                .is_some_and(|v| v.as_bool().is_none())
            {
                malformed_facts += 1;
            }
        }
        if atom.extensions.contains_key("file-cfg")
            && !atom.extensions.get("cfg").is_some_and(|v| v.is_string())
        {
            malformed_facts += 1;
        }
        if has_status && (foreign || unmounted || cfg_inactive) {
            stale_fact_conflicts += 1;
        }
        // Non-library targets (build.rs / tests / examples / benches) are
        // compiled outside the verified library, so Aeneas never translates
        // them — out of scope, not backlog (KB P25).
        let non_lib_target = enrich::is_non_library_target(&atom.code_path);
        // Curated config opt-out: functions Aeneas structurally does not
        // translate (Debug/Display fmt, Zeroize, …) which therefore never carry
        // a Lean translation to annotate `@[out_of_scope]` (KB P25).
        let config_oos = !oos_globs.is_empty() && {
            let rqn = atom
                .extensions
                .get("rust-qualified-name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            enrich::is_config_out_of_scope(rqn, &atom.display_name, &oos_globs)
        };
        // Tracked backlog by default; disabled only when out of scope and not
        // status-bearing (P24/P25).
        // Most intrinsic cause first: a C binding is out of scope regardless
        // of configuration; an unmounted file regardless of features; then
        // the cfg evaluation (with the file-level refinement of the reason);
        // then the policy opt-outs.
        let reason = if has_status {
            None
        } else if foreign {
            Some("foreign-declaration")
        } else if unmounted {
            Some("unmounted")
        } else if file_cfg_inactive {
            Some("file-cfg-inactive")
        } else if cfg_inactive {
            Some("cfg-inactive")
        } else if out_of_scope {
            Some("out-of-scope-translation")
        } else if non_lib_target {
            Some("non-library-target")
        } else if config_oos {
            Some("config-out-of-scope")
        } else {
            None
        };
        let untracked = reason.is_some();
        atom.extensions
            .insert("untracked".to_string(), serde_json::json!(untracked));
        match reason {
            // Why the atom is out of scope — for humans and tooling tracing
            // a grey atom back to its cause.
            Some(r) => {
                atom.extensions
                    .insert("untracked-reason".to_string(), serde_json::json!(r));
            }
            None => {
                atom.extensions.remove("untracked-reason");
            }
        }
        // Relevance is crate membership, independent of scope: external stubs
        // (empty code-path) reference other crates and are not relevant.
        atom.extensions.insert(
            "is-relevant".to_string(),
            serde_json::json!(!atom.code_path.is_empty()),
        );
        if !atom.extensions.contains_key("is-public") {
            atom.extensions
                .insert("is-public".to_string(), serde_json::json!(false));
        }
    }

    if stale_fact_conflicts > 0 {
        println!(
            "  scope: {} status-bearing atom(s) also carry out-of-scope source facts — kept tracked (P24); \
             probe-rust facts or the translation match may be stale",
            stale_fact_conflicts
        );
    }
    if malformed_facts > 0 {
        println!(
            "  scope: {} malformed source-fact field(s) ignored (wrong JSON type) — check the probe-rust input",
            malformed_facts
        );
    }
}

/// Warn when the Rust atoms were produced by a probe-rust older than 0.10.0:
/// the source-fact fields (`file-cfg`, `is-unmounted`, `is-foreign`,
/// `trait-required`) are then absent and scope classification silently
/// degrades to the per-function `cfg` evaluation. Degradation is by design,
/// but it must never be invisible — an operator upgrading probe-aeneas while
/// an old probe-rust sits first on PATH would otherwise see none of the new
/// classifications and no hint why.
fn warn_on_old_probe_rust(rust_path: &Path) {
    #[derive(serde::Deserialize)]
    struct ToolOnly {
        tool: Option<ToolInfo>,
    }
    #[derive(serde::Deserialize)]
    struct ToolInfo {
        name: Option<String>,
        version: Option<String>,
    }
    let Some(tool) = std::fs::read_to_string(rust_path)
        .ok()
        .and_then(|c| serde_json::from_str::<ToolOnly>(&c).ok())
        .and_then(|e| e.tool)
    else {
        return;
    };
    if tool.name.as_deref() != Some("probe-rust") {
        return;
    }
    let Some(version) = tool.version else { return };
    let mut parts = version.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if (major, minor) < (0, 10) {
        println!(
            "  Warning: Rust atoms come from probe-rust {version} (< 0.10.0) — no source-fact \
             fields (file-cfg/is-unmounted/is-foreign); scope classification is limited to the \
             per-function cfg predicate. Upgrade probe-rust for module-level and foreign-decl \
             classification."
        );
    }
}

/// Construct and write the Aeneas extract envelope.
fn write_aeneas_envelope(
    merged: std::collections::BTreeMap<String, Atom>,
    provenance: Vec<InputProvenance>,
    output_path: &Path,
    stats: &probe::commands::merge::MergeStats,
) -> Result<()> {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    print_public_api_coverage(&merged);

    let envelope = MergedAtomEnvelope {
        schema: "probe-aeneas/extract".to_string(),
        schema_version: "3.0".to_string(),
        tool: Tool {
            name: "probe-aeneas".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: "extract".to_string(),
        },
        inputs: provenance,
        timestamp,
        data: merged,
    };

    let json = serde_json::to_string_pretty(&envelope).context("serialize output")?;

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    std::fs::write(output_path, format!("{json}\n"))
        .with_context(|| format!("write {}", output_path.display()))?;

    println!("\nOutput: {}", output_path.display());
    println!("  Total entries:    {}", stats.total_entries);
    println!("  Stubs remaining:  {}", stats.stubs_remaining);
    println!("  New entries added: {}", stats.entries_added);
    println!("  Cross-lang edges: {}", stats.mappings_applied);

    Ok(())
}

/// Print public API verification coverage for Rust atoms that have
/// `is-public-api: true` (set by probe-rust). Uses the `verification-status`
/// already propagated onto Rust atoms from their Lean translations.
fn print_public_api_coverage(merged: &std::collections::BTreeMap<String, Atom>) {
    let public_api: Vec<&Atom> = merged
        .values()
        .filter(|a| {
            a.language == "rust"
                && a.extensions.get("is-public-api").and_then(|v| v.as_bool()) == Some(true)
        })
        .collect();

    if public_api.is_empty() {
        return;
    }

    let mut verified = 0u32;
    let mut unverified = 0u32;
    let mut trusted = 0u32;
    let mut other_status = 0u32;
    let mut no_translation = 0u32;

    for atom in &public_api {
        let status = atom
            .extensions
            .get("verification-status")
            .and_then(|v| v.as_str());

        match status {
            Some("verified") => verified += 1,
            Some("unverified") => unverified += 1,
            Some("trusted") => trusted += 1,
            Some(_) => other_status += 1,
            None => no_translation += 1,
        }
    }

    let total = public_api.len() as u32;

    println!("\nPublic API coverage:");
    println!("  {total} public API functions");
    if verified > 0 {
        println!("    {verified} verified");
    }
    if unverified > 0 {
        println!("    {unverified} unverified");
    }
    if trusted > 0 {
        println!("    {trusted} trusted");
    }
    if other_status > 0 {
        println!("    {other_status} other");
    }
    if no_translation > 0 {
        println!("    {no_translation} not in scope (no Lean translation)");
    }
}

/// Derive the default output path: `<project>/.verilib/probes/aeneas_<pkg>_<ver>.json`.
///
/// Follows the probe ecosystem convention (same layout as probe-rust, probe-verus).
/// Falls back to `aeneas_<pkg>_<ver>.json` in the current directory when no
/// project root is available.
fn default_output_path(rust_prov: &[InputProvenance], project_root: Option<&Path>) -> PathBuf {
    let (pkg, ver) = rust_prov
        .first()
        .map(|p| (p.source.package.as_str(), p.source.package_version.as_str()))
        .unwrap_or(("unknown", "0.0.0"));

    let safe_pkg = sanitize_for_filename(pkg);
    let safe_ver = sanitize_for_filename(ver);

    let name = if safe_ver.is_empty() {
        format!("aeneas_{safe_pkg}.json")
    } else {
        format!("aeneas_{safe_pkg}_{safe_ver}.json")
    };

    match project_root {
        Some(root) => root.join(".verilib").join("probes").join(name),
        None => PathBuf::from(name),
    }
}

/// Sanitize a string for use in a filename: replace `/`, `\` with `_`, and
/// collapse `..` to `_`.
fn sanitize_for_filename(s: &str) -> String {
    s.replace(['/', '\\'], "_").replace("..", "_")
}

/// Public entry point for the `translate` subcommand (translations only, no merge).
pub fn run_translate_only(
    rust_path: &Path,
    lean_path: &Path,
    functions_path: &Path,
    translation_json: Option<&Path>,
    output_path: &Path,
) -> Result<()> {
    println!("Loading Rust atoms from {}...", rust_path.display());
    let rust_data = load_atoms(rust_path)
        .with_context(|| format!("load Rust atoms from {}", rust_path.display()))?;
    println!("  {} atoms", rust_data.len());

    println!("Loading Lean atoms from {}...", lean_path.display());
    let lean_data = load_atoms(lean_path)
        .with_context(|| format!("load Lean atoms from {}", lean_path.display()))?;
    println!("  {} atoms", lean_data.len());

    // Resolve records through the single source dispatch (override arm: the
    // `translate` subcommand always supplies a `functions.json`). `translate`
    // emits only mappings, so the auxiliary-def set is intentionally unused.
    let resolved = function_source::resolve(None, Some(functions_path), translation_json, false)?;
    println!("  {} entries", resolved.records.len());

    println!("\nGenerating translations...");
    let (mappings, stats) = generate_translations(
        &rust_data,
        &lean_data,
        &resolved.records,
        resolved.charon_version.as_deref(),
    );

    println!("  {} translations generated", mappings.len());
    for (conf, count) in &stats.by_confidence {
        println!("    {conf}: {count}");
    }

    let rust_raw: serde_json::Value = {
        let content = std::fs::read_to_string(rust_path)
            .with_context(|| format!("read {}", rust_path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parse {}", rust_path.display()))?
    };
    let lean_raw: serde_json::Value = {
        let content = std::fs::read_to_string(lean_path)
            .with_context(|| format!("read {}", lean_path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parse {}", lean_path.display()))?
    };

    let json_value = build_translations_json(&mappings, &rust_raw, &lean_raw);
    let json = serde_json::to_string_pretty(&json_value).context("serialize translations")?;
    std::fs::write(output_path, format!("{json}\n"))
        .with_context(|| format!("write {}", output_path.display()))?;

    println!("\nWritten to {}", output_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parse_explicit_features_forms() {
        let f = |args: &[&str]| {
            let mut v =
                parse_explicit_features(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            v.sort();
            v
        };
        // Separate-value long and short forms.
        assert_eq!(f(&["--features", "alloc,serde"]), vec!["alloc", "serde"]);
        assert_eq!(f(&["-F", "alloc"]), vec!["alloc"]);
        // Attached `=` forms.
        assert_eq!(f(&["--features=alloc"]), vec!["alloc"]);
        assert_eq!(f(&["-F=alloc,serde"]), vec!["alloc", "serde"]);
        // Attached short form `-Ffoo`.
        assert_eq!(f(&["-Falloc"]), vec!["alloc"]);
        assert_eq!(f(&["-Falloc,serde"]), vec!["alloc", "serde"]);
        // Space-separated within one value.
        assert_eq!(f(&["--features", "alloc serde"]), vec!["alloc", "serde"]);
        // Unrelated args ignored; bare `-F` with no value is a no-op.
        assert_eq!(f(&["-p", "curve25519-dalek", "-F"]), Vec::<String>::new());
    }

    fn create_aeneas_project(dir: &Path, crate_dir: &str, crate_name: Option<&str>, dest: &str) {
        fs::create_dir_all(dir).unwrap();

        let rust_dir = if crate_dir == "." {
            dir.to_path_buf()
        } else {
            dir.join(crate_dir)
        };
        fs::create_dir_all(&rust_dir).unwrap();
        fs::write(rust_dir.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();

        fs::write(
            dir.join("lakefile.toml"),
            "name = \"Test\"\nversion = \"0.1.0\"",
        )
        .unwrap();

        let name_line = match crate_name {
            Some(n) => format!("  name: \"{n}\""),
            None => String::new(),
        };
        let config = format!(
            "crate:\n  dir: \"{crate_dir}\"\n{name_line}\naeneas_args:\n  dest: \"{dest}\"\n"
        );
        fs::write(dir.join("aeneas-config.yml"), config).unwrap();
    }

    #[test]
    fn resolve_project_subdirectory_crate() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("dalek");
        create_aeneas_project(
            &project,
            "curve25519-dalek",
            Some("curve25519_dalek"),
            "Curve25519Dalek",
        );

        let resolved = resolve_project(&project).unwrap();
        assert_eq!(resolved.rust_project, project.join("curve25519-dalek"));
        assert_eq!(resolved.lean_project, project);
        assert!(resolved.functions_json.is_none());
        assert_eq!(resolved.crate_dir, "curve25519-dalek");
    }

    #[test]
    fn resolve_project_dot_crate_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("spqr");
        create_aeneas_project(&project, ".", Some("spqr"), "Extraction");

        let resolved = resolve_project(&project).unwrap();
        assert_eq!(resolved.rust_project, project);
        assert_eq!(resolved.lean_project, project);
        assert!(resolved.functions_json.is_none());
        assert_eq!(resolved.crate_dir, ".");
    }

    #[test]
    fn resolve_project_picks_up_existing_functions_json() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        create_aeneas_project(&project, ".", None, "Out");

        let fj = project.join("functions.json");
        fs::write(&fj, r#"{"functions":[]}"#).unwrap();

        let resolved = resolve_project(&project).unwrap();
        assert_eq!(resolved.functions_json, Some(fj));
    }

    #[test]
    fn resolve_project_missing_config() {
        let tmp = tempfile::tempdir().unwrap();
        let err = resolve_project(tmp.path()).unwrap_err().to_string();
        assert!(err.contains("aeneas-config.yml"), "Error: {err}");
    }

    #[test]
    fn resolve_project_missing_cargo_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("bad");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("lakefile.toml"),
            "name = \"X\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        fs::write(
            project.join("aeneas-config.yml"),
            "crate:\n  dir: \"nonexistent\"\n",
        )
        .unwrap();

        let err = resolve_project(&project).unwrap_err().to_string();
        assert!(err.contains("Cargo.toml"), "Error: {err}");
    }

    /// Create a minimal Cargo workspace in a temp directory for testing
    /// `resolve_workspace_member`. The workspace has a single member crate.
    fn create_workspace_project(
        project: &Path,
        member_rel: &str,
        member_pkg_name: &str,
        crate_dir: &str,
        crate_name: &str,
    ) {
        fs::create_dir_all(project.join(member_rel)).unwrap();

        // Workspace Cargo.toml at project root
        fs::write(
            project.join("Cargo.toml"),
            format!("[workspace]\nresolver = \"2\"\nmembers = [\"{member_rel}\"]\n"),
        )
        .unwrap();

        // Member crate Cargo.toml
        fs::write(
            project.join(member_rel).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{member_pkg_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"
            ),
        )
        .unwrap();

        // Member needs at least a lib.rs or main.rs for cargo metadata to be happy
        let src = project.join(member_rel).join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("lib.rs"), "").unwrap();

        // Lean project files
        fs::write(
            project.join("lakefile.toml"),
            "name = \"T\"\nversion = \"0.1.0\"",
        )
        .unwrap();

        // aeneas-config.yml
        fs::write(
            project.join("aeneas-config.yml"),
            format!(
                "crate:\n  dir: \"{crate_dir}\"\n  name: \"{crate_name}\"\n\
                 charon:\n  cargo_args: [\"-p\", \"{member_pkg_name}\"]\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn resolve_project_workspace_member() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("workspace");
        create_workspace_project(
            &project,
            "rust/crypto",
            "signal-crypto",
            "rust",
            "signal_crypto",
        );

        let resolved = resolve_project(&project).unwrap();

        let expected_root = std::fs::canonicalize(&project).unwrap();
        let actual = std::fs::canonicalize(&resolved.rust_project).unwrap();
        assert_eq!(
            actual, expected_root,
            "Should resolve to workspace root, not member crate"
        );
        assert_eq!(
            resolved.crate_dir, ".",
            "crate_dir should be '.' when running at workspace root"
        );
    }

    #[test]
    fn resolve_project_non_workspace_root_still_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("nonws");
        fs::create_dir_all(project.join("rust")).unwrap();

        // Root Cargo.toml is a regular package, NOT a workspace
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2021\"",
        )
        .unwrap();
        fs::write(
            project.join("lakefile.toml"),
            "name = \"T\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        fs::write(
            project.join("aeneas-config.yml"),
            "crate:\n  dir: \"rust\"\n",
        )
        .unwrap();

        let err = resolve_project(&project).unwrap_err().to_string();
        assert!(
            err.contains("Cargo.toml"),
            "Non-workspace root should still error: {err}"
        );
    }

    #[test]
    fn resolve_project_workspace_unknown_package_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("workspace");
        fs::create_dir_all(project.join("rust/crypto")).unwrap();

        fs::write(
            project.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"rust/crypto\"]\n",
        )
        .unwrap();
        fs::write(
            project.join("rust/crypto/Cargo.toml"),
            "[package]\nname = \"signal-crypto\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let src = project.join("rust/crypto/src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("lib.rs"), "").unwrap();
        fs::write(
            project.join("lakefile.toml"),
            "name = \"T\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        // crate.name resolves to "nonexistent-crate" which doesn't match any member
        fs::write(
            project.join("aeneas-config.yml"),
            "crate:\n  dir: \"rust\"\n  name: \"nonexistent_crate\"\n",
        )
        .unwrap();

        let err = resolve_project(&project).unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "Unknown package should error: {err}"
        );
    }

    #[test]
    fn resolve_project_workspace_backfills_charon_package() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("workspace");
        fs::create_dir_all(project.join("rust/crypto")).unwrap();

        fs::write(
            project.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"rust/crypto\"]\n",
        )
        .unwrap();
        fs::write(
            project.join("rust/crypto/Cargo.toml"),
            "[package]\nname = \"signal-crypto\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        let src = project.join("rust/crypto/src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("lib.rs"), "").unwrap();
        fs::write(
            project.join("lakefile.toml"),
            "name = \"T\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        // Only crate.name set, no charon.package or cargo_args -p
        fs::write(
            project.join("aeneas-config.yml"),
            "crate:\n  dir: \"rust\"\n  name: \"signal_crypto\"\ncharon:\n  preset: aeneas\n",
        )
        .unwrap();

        let resolved = resolve_project(&project).unwrap();

        assert_eq!(
            resolved
                .charon_config
                .as_ref()
                .and_then(|c| c.package.as_deref()),
            Some("signal-crypto"),
            "charon.package should be backfilled from crate.name"
        );
    }

    #[test]
    fn resolve_project_workspace_preserves_existing_charon_package() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("workspace");
        create_workspace_project(
            &project,
            "rust/crypto",
            "signal-crypto",
            "rust",
            "signal_crypto",
        );

        let resolved = resolve_project(&project).unwrap();

        // create_workspace_project sets cargo_args: ["-p", "signal-crypto"],
        // so charon.package should NOT be backfilled (already in cargo_args).
        assert!(
            resolved
                .charon_config
                .as_ref()
                .and_then(|c| c.package.as_deref())
                .is_none(),
            "charon.package should not be set when -p is already in cargo_args"
        );
    }

    #[test]
    fn resolve_target_package_name_from_charon_package() {
        let cc = CharonConfig {
            package: Some("signal-crypto".to_string()),
            cargo_args: Some(vec!["-p".to_string(), "other".to_string()]),
            ..Default::default()
        };
        assert_eq!(
            resolve_target_package_name(Some("crate_name"), Some(&cc)),
            Some("signal-crypto".to_string()),
            "charon.package takes highest priority"
        );
    }

    #[test]
    fn resolve_target_package_name_from_cargo_args() {
        let cc = CharonConfig {
            cargo_args: Some(vec![
                "-p".to_string(),
                "signal-crypto".to_string(),
                "--features".to_string(),
                "extraction".to_string(),
            ]),
            ..Default::default()
        };
        assert_eq!(
            resolve_target_package_name(None, Some(&cc)),
            Some("signal-crypto".to_string()),
        );
    }

    #[test]
    fn resolve_target_package_name_from_crate_name() {
        assert_eq!(
            resolve_target_package_name(Some("signal_crypto"), None),
            Some("signal-crypto".to_string()),
            "crate.name underscores should be converted to hyphens"
        );
    }

    #[test]
    fn resolve_target_package_name_none_when_empty() {
        assert_eq!(resolve_target_package_name(None, None), None);
    }

    #[test]
    fn resolve_project_missing_lakefile() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("nolake");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "[package]\nname = \"t\"").unwrap();
        fs::write(project.join("aeneas-config.yml"), "crate:\n  dir: \".\"\n").unwrap();

        let err = resolve_project(&project).unwrap_err().to_string();
        assert!(err.contains("lakefile"), "Error: {err}");
    }

    #[test]
    fn yaml_parse_minimal_config() {
        let yaml = "crate:\n  dir: \"src-rust\"\n";
        let config: AeneasProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.crate_config.dir, "src-rust");
        assert!(config.crate_config.name.is_none());
        assert!(config.aeneas_args.is_none());
    }

    #[test]
    fn yaml_parse_full_config() {
        let yaml = r#"
crate:
  dir: "curve25519-dalek"
  name: "curve25519_dalek"
aeneas_args:
  dest: "Curve25519Dalek"
  backend: lean
  options:
    - split-files
"#;
        let config: AeneasProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.crate_config.dir, "curve25519-dalek");
        assert_eq!(
            config.crate_config.name.as_deref(),
            Some("curve25519_dalek")
        );
        let args = config.aeneas_args.unwrap();
        assert_eq!(args.dest.as_deref(), Some("Curve25519Dalek"));
    }

    #[test]
    fn yaml_parse_ignores_extra_fields() {
        let yaml = r#"
aeneas:
  commit: "abc123"
  repo: "https://example.com"
crate:
  dir: "."
  name: "test"
charon:
  preset: aeneas
  start_from:
    - "test::foo"
aeneas_args:
  dest: "Out"
tweaks:
  files: ["Types.lean"]
"#;
        let config: AeneasProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.crate_config.dir, ".");
        assert_eq!(config.crate_config.name.as_deref(), Some("test"));
    }

    #[test]
    fn yaml_parse_charon_config() {
        let yaml = r#"
crate:
  dir: "curve25519-dalek"
  name: "curve25519_dalek"
charon:
  preset: aeneas
  package: "curve25519-dalek"
  cargo_args:
    - "--no-default-features"
    - "--features"
    - "alloc,zeroize"
  start_from:
    - "curve25519_dalek::scalar"
    - "curve25519_dalek::field"
  exclude:
    - "curve25519_dalek::backend::vector"
  opaque:
    - "core::*"
"#;
        let config: AeneasProjectConfig = serde_yaml::from_str(yaml).unwrap();
        let charon = config.charon.expect("charon section should be parsed");
        assert_eq!(charon.preset.as_deref(), Some("aeneas"));
        assert_eq!(charon.package.as_deref(), Some("curve25519-dalek"));
        assert_eq!(
            charon.cargo_args.as_deref(),
            Some(
                ["--no-default-features", "--features", "alloc,zeroize"]
                    .map(String::from)
                    .as_slice()
            )
        );
        assert_eq!(charon.start_from.as_ref().map(|v| v.len()), Some(2));
        assert_eq!(charon.exclude.as_ref().map(|v| v.len()), Some(1));
        assert_eq!(charon.opaque.as_ref().map(|v| v.len()), Some(1));
    }

    #[test]
    fn yaml_parse_no_charon_section() {
        let yaml = "crate:\n  dir: \".\"\n";
        let config: AeneasProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.charon.is_none());
    }

    #[test]
    fn resolve_project_carries_charon_config() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "[package]\nname = \"t\"").unwrap();
        fs::write(
            project.join("lakefile.toml"),
            "name = \"T\"\nversion = \"0.1.0\"",
        )
        .unwrap();
        fs::write(
            project.join("aeneas-config.yml"),
            "crate:\n  dir: \".\"\ncharon:\n  preset: aeneas\n  exclude:\n    - \"x::y\"\n",
        )
        .unwrap();

        let resolved = resolve_project(&project).unwrap();
        let cc = resolved
            .charon_config
            .expect("charon_config should be Some");
        assert_eq!(cc.preset.as_deref(), Some("aeneas"));
        assert_eq!(cc.exclude.as_ref().map(|v| v.len()), Some(1));
    }

    fn make_rust_atom(name: &str) -> Atom {
        Atom {
            display_name: name.to_string(),
            dependencies: std::collections::BTreeSet::new(),
            code_module: "module".to_string(),
            code_path: "src/lib.rs".to_string(),
            code_text: probe::types::CodeText {
                lines_start: 1,
                lines_end: 10,
            },
            kind: "exec".to_string(),
            language: "rust".to_string(),
            extensions: std::collections::BTreeMap::new(),
        }
    }

    fn make_lean_atom(name: &str) -> Atom {
        Atom {
            display_name: name.to_string(),
            dependencies: std::collections::BTreeSet::new(),
            code_module: "Module".to_string(),
            code_path: "Module/Funs.lean".to_string(),
            code_text: probe::types::CodeText {
                lines_start: 100,
                lines_end: 110,
            },
            kind: "def".to_string(),
            language: "lean".to_string(),
            extensions: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn enrich_defaults_is_public_false_for_rust_atoms() {
        let mut merged = std::collections::BTreeMap::new();
        merged.insert("probe:crate/1.0/foo()".to_string(), make_rust_atom("foo"));

        let from_to = HashMap::new();

        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:crate/1.0/foo()").unwrap();
        assert_eq!(
            atom.extensions.get("is-public"),
            Some(&serde_json::json!(false)),
            "Rust atom without Charon data should default is-public to false"
        );
    }

    #[test]
    fn enrich_preserves_existing_is_public_true() {
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("bar");
        atom.extensions
            .insert("is-public".to_string(), serde_json::json!(true));
        merged.insert("probe:crate/1.0/bar()".to_string(), atom);

        let from_to = HashMap::new();

        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:crate/1.0/bar()").unwrap();
        assert_eq!(
            atom.extensions.get("is-public"),
            Some(&serde_json::json!(true)),
            "Existing is-public: true from probe-rust should be preserved"
        );
    }

    #[test]
    fn enrich_does_not_add_is_public_to_lean_atoms() {
        let mut merged = std::collections::BTreeMap::new();
        merged.insert(
            "probe:module.lean_fn".to_string(),
            make_lean_atom("lean_fn"),
        );

        let from_to = HashMap::new();

        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:module.lean_fn").unwrap();
        assert!(
            !atom.extensions.contains_key("is-public"),
            "Lean atoms should not get is-public"
        );
    }

    #[test]
    fn enrich_translated_atom_is_tracked() {
        let mut merged = std::collections::BTreeMap::new();

        let mut rust_atom = make_rust_atom("step_2");
        rust_atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::ristretto::step_2"),
        );
        merged.insert(
            "probe:my-crate/1.0/ristretto/decompress/step_2()".to_string(),
            rust_atom,
        );
        merged.insert(
            "probe:my_crate.ristretto.decompress.step_2".to_string(),
            make_lean_atom("step_2"),
        );

        let mut from_to: HashMap<String, Vec<String>> = HashMap::new();
        from_to
            .entry("probe:my-crate/1.0/ristretto/decompress/step_2()".to_string())
            .or_default()
            .push("probe:my_crate.ristretto.decompress.step_2".to_string());

        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged
            .get("probe:my-crate/1.0/ristretto/decompress/step_2()")
            .unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(false)),
            "a translated (status-bearing) atom is tracked, never disabled"
        );
        assert_eq!(
            atom.extensions.get("is-relevant"),
            Some(&serde_json::json!(true)),
            "atom with translation should be relevant"
        );
        assert_eq!(
            atom.extensions.get("translation-name"),
            Some(&serde_json::json!(
                "probe:my_crate.ristretto.decompress.step_2"
            )),
            "translation-name should be set from Lean atom"
        );
        assert_eq!(
            atom.extensions.get("verification-status"),
            Some(&serde_json::json!("unverified")),
            "translated atom without spec should get unverified status"
        );
    }

    #[test]
    fn enrich_untranslated_compiled_atom_is_backlog() {
        // A compiled Rust function Aeneas did not translate (no from_to entry,
        // no cfg gate) is unverified backlog: untracked false, no status.
        let mut merged = std::collections::BTreeMap::new();
        merged.insert("probe:crate/1.0/foo()".to_string(), make_rust_atom("foo"));

        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:crate/1.0/foo()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(false)),
            "untranslated compiled function is tracked backlog, not disabled"
        );
        assert!(
            !atom.extensions.contains_key("verification-status"),
            "backlog atom carries no verification-status"
        );
    }

    #[test]
    fn enrich_cfg_inactive_atom_untracked() {
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("serde_only");
        atom.extensions
            .insert("cfg".to_string(), serde_json::json!(r#"feature = "serde""#));
        merged.insert("probe:crate/1.0/serde_only()".to_string(), atom);

        // Active features do not include `serde` → predicate inactive.
        let cfg = crate::cfg_eval::CfgConfig {
            features: ["alloc"].iter().map(|s| s.to_string()).collect(),
        };
        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, Some(&cfg), &[]);

        let atom = merged.get("probe:crate/1.0/serde_only()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(true)),
            "a cfg-inactive function is out of scope"
        );
        assert!(!atom.extensions.contains_key("verification-status"));
    }

    #[test]
    fn enrich_file_cfg_inactive_atom_untracked() {
        // probe-rust folds the parent-file mod-chain gate into `cfg` and
        // reports the chain component alone as `file-cfg`; an inactive chain
        // means the whole file is gated off in the Aeneas build.
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("in_test_module");
        atom.code_path = "src/sha3/tests.rs".to_string();
        atom.extensions
            .insert("cfg".to_string(), serde_json::json!("test"));
        atom.extensions
            .insert("file-cfg".to_string(), serde_json::json!("test"));
        merged.insert("probe:crate/1.0/in_test_module()".to_string(), atom);

        let cfg = crate::cfg_eval::CfgConfig {
            features: std::collections::HashSet::new(),
        };
        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, Some(&cfg), &[]);

        let atom = merged.get("probe:crate/1.0/in_test_module()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(true)),
            "a function in a cfg-inactive file is out of scope"
        );
        assert_eq!(
            atom.extensions.get("untracked-reason"),
            Some(&serde_json::json!("file-cfg-inactive")),
            "the file-level gate wins the reason over the cfg catch-all"
        );
    }

    #[test]
    fn enrich_unmounted_atom_untracked() {
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("dead_fn");
        atom.code_path = "src/verify/tests/harness.rs".to_string();
        atom.extensions
            .insert("is-unmounted".to_string(), serde_json::json!(true));
        merged.insert("probe:crate/1.0/dead_fn()".to_string(), atom);

        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:crate/1.0/dead_fn()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(true)),
            "an unmounted file's function is out of scope even without a resolved feature set"
        );
        assert_eq!(
            atom.extensions.get("untracked-reason"),
            Some(&serde_json::json!("unmounted"))
        );
    }

    #[test]
    fn enrich_unmounted_but_status_bearing_stays_tracked() {
        // P24: a status-bearing atom is never disabled, even when probe-rust
        // flags its file (e.g. a stale fact against fresher Lean progress).
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("verified_fn");
        atom.code_path = "src/sha3/tests.rs".to_string();
        atom.extensions
            .insert("is-unmounted".to_string(), serde_json::json!(true));
        atom.extensions
            .insert("is-foreign".to_string(), serde_json::json!(true));
        atom.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("verified"),
        );
        merged.insert("probe:crate/1.0/verified_fn()".to_string(), atom);

        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:crate/1.0/verified_fn()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(false)),
            "has-status implies in-scope (P24)"
        );
    }

    #[test]
    fn feature_default_name_is_active() {
        // `#[cfg(feature = "default")]` is active in a default cargo build:
        // the closure must be seeded with the feature NAME, not its contents.
        let mut edges = HashMap::new();
        edges.insert("default".to_string(), vec!["std".to_string()]);
        edges.insert("std".to_string(), Vec::new());
        let set = resolve_feature_set(&edges, &[]);
        assert!(set.contains("default"), "{set:?}");
        assert!(set.contains("std"), "{set:?}");

        let none = resolve_feature_set(&edges, &["--no-default-features".to_string()]);
        assert!(none.is_empty(), "{none:?}");
    }

    #[test]
    fn file_cfg_alone_never_untracks() {
        // `file-cfg` is provenance only: without a `cfg` that evaluates
        // definitively false, it must not classify (a malformed producer
        // could otherwise grey compiled code).
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("odd_one");
        atom.extensions
            .insert("file-cfg".to_string(), serde_json::json!("test"));
        merged.insert("probe:crate/1.0/odd_one()".to_string(), atom);

        let cfg = crate::cfg_eval::CfgConfig {
            features: std::collections::HashSet::new(),
        };
        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, Some(&cfg), &[]);

        let atom = merged.get("probe:crate/1.0/odd_one()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(false)),
            "file-cfg without an inactive cfg must not untrack"
        );
    }

    #[test]
    fn mistyped_fact_is_ignored() {
        // A string "true" is not a boolean fact: conservative absence.
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("weird");
        atom.extensions
            .insert("is-unmounted".to_string(), serde_json::json!("true"));
        atom.extensions
            .insert("is-foreign".to_string(), serde_json::json!(1));
        merged.insert("probe:crate/1.0/weird()".to_string(), atom);

        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:crate/1.0/weird()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(false))
        );
    }

    #[test]
    fn null_status_does_not_shield_from_scope() {
        // Only a string-typed verification-status counts as a status (P24
        // protects real progress, not a stray null).
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("SymCryptWipe");
        atom.extensions
            .insert("verification-status".to_string(), serde_json::json!(null));
        atom.extensions
            .insert("is-foreign".to_string(), serde_json::json!(true));
        merged.insert("probe:crate/1.0/SymCryptWipe()".to_string(), atom);

        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:crate/1.0/SymCryptWipe()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn enrich_foreign_decl_untracked() {
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("SymCryptInit");
        atom.extensions
            .insert("is-foreign".to_string(), serde_json::json!(true));
        merged.insert("probe:crate/1.0/SymCryptInit()".to_string(), atom);

        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:crate/1.0/SymCryptInit()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(true)),
            "a bodyless foreign declaration is out of scope"
        );
        assert_eq!(
            atom.extensions.get("untracked-reason"),
            Some(&serde_json::json!("foreign-declaration")),
            "foreign wins the reason over any cfg signal"
        );
    }

    #[test]
    fn enrich_active_cfg_atom_stays_backlog() {
        let mut merged = std::collections::BTreeMap::new();
        let mut atom = make_rust_atom("alloc_gated");
        atom.extensions
            .insert("cfg".to_string(), serde_json::json!(r#"feature = "alloc""#));
        merged.insert("probe:crate/1.0/alloc_gated()".to_string(), atom);

        let cfg = crate::cfg_eval::CfgConfig {
            features: ["alloc"].iter().map(|s| s.to_string()).collect(),
        };
        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, Some(&cfg), &[]);

        let atom = merged.get("probe:crate/1.0/alloc_gated()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(false)),
            "an active-feature-gated function stays tracked backlog"
        );
    }

    #[test]
    fn enrich_non_library_target_untracked() {
        let mut merged = std::collections::BTreeMap::new();
        // A benchmark function: compiled outside the verified library, no cfg,
        // no translation → out of scope, not backlog (KB P25).
        let mut bench = make_rust_atom("bench_fn");
        bench.code_path = "curve25519-dalek/benches/dalek_benchmarks.rs".to_string();
        merged.insert("probe:crate/1.0/benches/bench_fn()".to_string(), bench);
        // A build-script function.
        let mut build = make_rust_atom("main");
        build.code_path = "curve25519-dalek/build.rs".to_string();
        merged.insert("probe:crate/1.0/build/main()".to_string(), build);
        // A genuine library function stays tracked backlog.
        let lib = make_rust_atom("lib_fn"); // code_path defaults to src/lib.rs
        merged.insert("probe:crate/1.0/lib_fn()".to_string(), lib);

        let from_to = HashMap::new();
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        for k in [
            "probe:crate/1.0/benches/bench_fn()",
            "probe:crate/1.0/build/main()",
        ] {
            assert_eq!(
                merged[k].extensions.get("untracked"),
                Some(&serde_json::json!(true)),
                "non-library target {k} should be out of scope"
            );
            assert!(!merged[k].extensions.contains_key("verification-status"));
        }
        assert_eq!(
            merged["probe:crate/1.0/lib_fn()"]
                .extensions
                .get("untracked"),
            Some(&serde_json::json!(false)),
            "library function stays tracked backlog"
        );
    }

    #[test]
    fn enrich_config_out_of_scope_pattern_disables() {
        let mut merged = std::collections::BTreeMap::new();
        // A Debug `fmt` the config opts out of; no translation, no cfg.
        let mut fmt = make_rust_atom("EdwardsPoint::fmt");
        fmt.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!(
                "curve25519_dalek::edwards::{core::fmt::Debug for curve25519_dalek::edwards::EdwardsPoint}::fmt"
            ),
        );
        merged.insert(
            "probe:crate/1.0/edwards/EdwardsPoint_fmt()".to_string(),
            fmt,
        );
        // A normal library function the pattern must NOT touch.
        let lib = make_rust_atom("EdwardsPoint::compress");
        merged.insert("probe:crate/1.0/edwards/compress()".to_string(), lib);

        let from_to = HashMap::new();
        let patterns = ["*core::fmt::Debug*".to_string(), "*::fmt".to_string()];
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &patterns);

        assert_eq!(
            merged["probe:crate/1.0/edwards/EdwardsPoint_fmt()"]
                .extensions
                .get("untracked"),
            Some(&serde_json::json!(true)),
            "config out-of-scope pattern should disable the matching fn"
        );
        assert_eq!(
            merged["probe:crate/1.0/edwards/compress()"]
                .extensions
                .get("untracked"),
            Some(&serde_json::json!(false)),
            "a non-matching library fn stays tracked backlog"
        );
    }

    #[test]
    fn enrich_out_of_scope_translation_untracked() {
        let mut merged = std::collections::BTreeMap::new();

        let rust_atom = make_rust_atom("opt_out");
        merged.insert("probe:my-crate/1.0/opt_out()".to_string(), rust_atom);

        let mut lean_atom = make_lean_atom("opt_out");
        lean_atom.extensions.insert(
            "attributes".to_string(),
            serde_json::json!(["out_of_scope"]),
        );
        merged.insert("probe:my_crate.opt_out".to_string(), lean_atom);

        let mut from_to: HashMap<String, Vec<String>> = HashMap::new();
        from_to
            .entry("probe:my-crate/1.0/opt_out()".to_string())
            .or_default()
            .push("probe:my_crate.opt_out".to_string());

        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:my-crate/1.0/opt_out()").unwrap();
        assert_eq!(
            atom.extensions.get("untracked"),
            Some(&serde_json::json!(true)),
            "@[out_of_scope] translation marks the Rust function out of scope"
        );
        assert!(
            !atom.extensions.contains_key("verification-status"),
            "out-of-scope atom carries no verification-status (P24)"
        );
        // Translation metadata is still recorded.
        assert_eq!(
            atom.extensions.get("translation-name"),
            Some(&serde_json::json!("probe:my_crate.opt_out"))
        );
    }

    #[test]
    fn prefix_rust_code_paths_adds_crate_dir() {
        let mut merged = std::collections::BTreeMap::new();
        merged.insert("probe:crate/1.0/foo()".to_string(), make_rust_atom("foo"));
        merged.insert(
            "probe:module.lean_fn".to_string(),
            make_lean_atom("lean_fn"),
        );

        prefix_rust_code_paths(&mut merged, "curve25519-dalek");

        assert_eq!(
            merged["probe:crate/1.0/foo()"].code_path, "curve25519-dalek/src/lib.rs",
            "Rust atom code-path should be prefixed with crate directory"
        );
        assert_eq!(
            merged["probe:module.lean_fn"].code_path, "Module/Funs.lean",
            "Lean atom code-path should not be modified"
        );
    }

    #[test]
    fn prefix_rust_code_paths_skips_empty_paths() {
        let mut merged = std::collections::BTreeMap::new();
        let mut stub = make_rust_atom("stub");
        stub.code_path = String::new();
        merged.insert("probe:crate/1.0/stub()".to_string(), stub);

        prefix_rust_code_paths(&mut merged, "curve25519-dalek");

        assert_eq!(
            merged["probe:crate/1.0/stub()"].code_path, "",
            "Empty code-path (stdlib stubs) should not be prefixed"
        );
    }

    fn make_spec_atom(name: &str, vs: &str) -> Atom {
        let mut atom = Atom {
            display_name: format!("{name}_spec"),
            dependencies: std::collections::BTreeSet::new(),
            code_module: "Module".to_string(),
            code_path: "Module/Spec.lean".to_string(),
            code_text: probe::types::CodeText {
                lines_start: 200,
                lines_end: 210,
            },
            kind: "theorem".to_string(),
            language: "lean".to_string(),
            extensions: std::collections::BTreeMap::new(),
        };
        atom.extensions
            .insert("verification-status".to_string(), serde_json::json!(vs));
        atom
    }

    fn setup_translation(
        merged: &mut std::collections::BTreeMap<String, Atom>,
        lean_vs: Option<&str>,
        spec_vs: Option<&str>,
    ) -> HashMap<String, Vec<String>> {
        let mut rust_atom = make_rust_atom("my_fn");
        rust_atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::my_fn"),
        );
        merged.insert("probe:my-crate/1.0/my_fn()".to_string(), rust_atom);

        let mut lean_atom = make_lean_atom("my_fn");
        if let Some(vs) = lean_vs {
            lean_atom
                .extensions
                .insert("verification-status".to_string(), serde_json::json!(vs));
        }
        merged.insert("probe:my_crate.my_fn".to_string(), lean_atom);

        if let Some(svs) = spec_vs {
            merged.insert(
                "probe:my_crate.my_fn_spec".to_string(),
                make_spec_atom("my_crate.my_fn", svs),
            );
        }

        let mut from_to: HashMap<String, Vec<String>> = HashMap::new();
        from_to
            .entry("probe:my-crate/1.0/my_fn()".to_string())
            .or_default()
            .push("probe:my_crate.my_fn".to_string());
        from_to
    }

    #[test]
    fn vs_verified_def_with_verified_spec() {
        let mut merged = std::collections::BTreeMap::new();
        let from_to = setup_translation(&mut merged, Some("verified"), Some("verified"));
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:my-crate/1.0/my_fn()").unwrap();
        assert_eq!(
            atom.extensions.get("verification-status"),
            Some(&serde_json::json!("verified")),
            "def with verified spec should propagate verified"
        );
    }

    #[test]
    fn vs_verified_def_without_spec() {
        let mut merged = std::collections::BTreeMap::new();
        let from_to = setup_translation(&mut merged, Some("verified"), None);
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:my-crate/1.0/my_fn()").unwrap();
        assert_eq!(
            atom.extensions.get("verification-status"),
            Some(&serde_json::json!("unverified")),
            "def without spec should be unverified even if lean def is verified"
        );
    }

    #[test]
    fn vs_trusted_def_preserved() {
        let mut merged = std::collections::BTreeMap::new();
        let from_to = setup_translation(&mut merged, Some("trusted"), Some("verified"));
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:my-crate/1.0/my_fn()").unwrap();
        assert_eq!(
            atom.extensions.get("verification-status"),
            Some(&serde_json::json!("trusted")),
            "trusted status should be preserved regardless of spec"
        );
    }

    #[test]
    fn vs_failed_def_preserved() {
        let mut merged = std::collections::BTreeMap::new();
        let from_to = setup_translation(&mut merged, Some("failed"), Some("verified"));
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:my-crate/1.0/my_fn()").unwrap();
        assert_eq!(
            atom.extensions.get("verification-status"),
            Some(&serde_json::json!("failed")),
            "failed status should be preserved regardless of spec"
        );
    }

    #[test]
    fn vs_verified_def_with_failed_spec() {
        let mut merged = std::collections::BTreeMap::new();
        let from_to = setup_translation(&mut merged, Some("verified"), Some("failed"));
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:my-crate/1.0/my_fn()").unwrap();
        assert_eq!(
            atom.extensions.get("verification-status"),
            Some(&serde_json::json!("failed")),
            "spec's failed status should propagate to rust atom"
        );
    }

    #[test]
    fn vs_no_lean_status_no_spec() {
        let mut merged = std::collections::BTreeMap::new();
        let from_to = setup_translation(&mut merged, None, None);
        enrich_with_aeneas_metadata(&mut merged, &from_to, None, &[]);

        let atom = merged.get("probe:my-crate/1.0/my_fn()").unwrap();
        assert_eq!(
            atom.extensions.get("verification-status"),
            Some(&serde_json::json!("unverified")),
            "lean def without status and no spec should be unverified"
        );
    }

    fn make_provenance(pkg: &str, ver: &str) -> InputProvenance {
        InputProvenance {
            schema: "probe-rust/extract".to_string(),
            source: probe::types::Source {
                repo: String::new(),
                commit: String::new(),
                language: "rust".to_string(),
                package: pkg.to_string(),
                package_version: ver.to_string(),
                extensions: Default::default(),
            },
        }
    }

    #[test]
    fn default_output_path_with_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let prov = vec![make_provenance("curve25519-dalek", "4.1.3")];

        let path = default_output_path(&prov, Some(tmp.path()));
        assert_eq!(
            path,
            tmp.path()
                .join(".verilib/probes/aeneas_curve25519-dalek_4.1.3.json")
        );
    }

    #[test]
    fn default_output_path_without_project_root() {
        let prov = vec![make_provenance("my-crate", "1.0.0")];

        let path = default_output_path(&prov, None);
        assert_eq!(path, PathBuf::from("aeneas_my-crate_1.0.0.json"));
    }

    #[test]
    fn sanitize_for_filename_replaces_slashes() {
        assert_eq!(sanitize_for_filename("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_for_filename("foo..bar"), "foo_bar");
        assert_eq!(sanitize_for_filename("normal-name"), "normal-name");
    }
}
