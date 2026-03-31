use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use probe::commands::merge::merge_atom_files;
use probe::types::{Atom, InputProvenance, MergedAtomEnvelope, Tool};
use serde::Deserialize;

use crate::aeneas_config::AeneasConfig;
use crate::enrich;
use crate::extract_runner;
use crate::gen_functions::generate_functions_json;
use crate::listfuns::run_listfuns;
use crate::translate::{
    build_functions_rust_names, build_translations_json, generate_translations, load_atoms,
    load_functions, normalize_rust_name,
};

type TranslationMaps = (HashMap<String, String>, HashMap<String, String>);

// ---------------------------------------------------------------------------
// aeneas-config.yml parsing (minimal: only fields probe-aeneas needs)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AeneasProjectConfig {
    #[serde(rename = "crate")]
    crate_config: CrateConfig,
    aeneas_args: Option<AeneasArgsConfig>,
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

/// Resolved paths derived from an Aeneas project directory.
#[derive(Debug)]
pub struct ResolvedProject {
    pub rust_project: PathBuf,
    pub lean_project: PathBuf,
    pub functions_json: Option<PathBuf>,
    /// The `crate.dir` value from `aeneas-config.yml`.
    /// When not `"."`, Rust atoms need their `code-path` prefixed with this
    /// directory so paths are relative to the repository root, not the crate.
    pub crate_dir: String,
}

/// Parse `aeneas-config.yml` in the given project directory and derive
/// the Rust project path, Lean project path, and optional functions.json.
pub fn resolve_project(project: &Path) -> Result<ResolvedProject, String> {
    let config_path = project.join("aeneas-config.yml");
    if !config_path.exists() {
        return Err(format!(
            "No aeneas-config.yml found in {}\n\
             Expected an Aeneas project directory containing aeneas-config.yml.\n\
             Use --rust-project / --lean-project for manual input.",
            project.display()
        ));
    }

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {e}", config_path.display()))?;
    let config: AeneasProjectConfig = serde_yaml::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {e}", config_path.display()))?;

    let crate_dir = &config.crate_config.dir;
    let rust_project = if crate_dir == "." {
        project.to_path_buf()
    } else {
        project.join(crate_dir)
    };
    let lean_project = project.to_path_buf();

    if !rust_project.join("Cargo.toml").exists() {
        return Err(format!(
            "No Cargo.toml found at {} (derived from crate.dir = {:?} in aeneas-config.yml)",
            rust_project.display(),
            crate_dir,
        ));
    }

    if !lean_project.join("lakefile.toml").exists() && !lean_project.join("lakefile.lean").exists()
    {
        return Err(format!(
            "No lakefile.toml or lakefile.lean found in {}\n\
             The project root should be a Lean/Lake project.",
            lean_project.display()
        ));
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

    Ok(ResolvedProject {
        rust_project,
        lean_project,
        functions_json,
        crate_dir: config.crate_config.dir.clone(),
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
    output_path: Option<&Path>,
    aeneas_config: Option<&Path>,
    use_lake: bool,
    rust_path_prefix: Option<&str>,
) -> Result<(), String> {
    // --- Validate inputs ---
    if rust_json.is_none() && rust_project.is_none() {
        return Err("No Rust input provided. Use one of:\n  \
             probe-aeneas extract <project_path>          (auto-detect from aeneas-config.yml)\n  \
             probe-aeneas extract --rust-project <path>   (Rust project directory)\n  \
             probe-aeneas extract --rust <json>            (pre-generated atoms JSON)"
            .to_string());
    }
    if lean_json.is_none() && lean_project.is_none() {
        return Err("No Lean input provided. Use one of:\n  \
             probe-aeneas extract <project_path>          (auto-detect from aeneas-config.yml)\n  \
             probe-aeneas extract --lean-project <path>   (Lean project directory)\n  \
             probe-aeneas extract --lean <json>            (pre-generated atoms JSON)"
            .to_string());
    }
    if functions_json.is_none() && lean_project.is_none() {
        return Err("--functions is required when --lean-project is not given \
             (cannot auto-generate functions.json without a Lean project path)"
            .to_string());
    }

    // --- Resolve inputs (extract if needed) ---
    // When both --lean and --lean-project are given, skip Lean extraction
    // (use the pre-computed JSON) but keep the project dir for listfuns.
    let (rust_path, lean_path) = resolve_inputs(rust_json, rust_project, lean_json, lean_project)?;

    // --- Resolve functions.json ---
    let functions_path = resolve_functions(functions_json, lean_project, use_lake)?;

    // --- Load Aeneas config (optional) ---
    let config = AeneasConfig::load(aeneas_config, lean_project)?;

    // --- Generate translations ---
    let (translations_result, funs_rust_names) =
        run_translate(&rust_path, &lean_path, &functions_path)?;

    // --- Merge atom maps ---
    run_extract_with_translations(
        &rust_path,
        &lean_path,
        &translations_result,
        &funs_rust_names,
        output_path,
        &config,
        rust_path_prefix,
        lean_project,
    )
}

/// Resolve Rust and Lean inputs, running extractors in parallel when both are
/// project paths.
///
/// When `lean_project` is available, intermediate extractor outputs are saved
/// to `<lean_project>/.verilib/probes/` alongside the final merged output.
fn resolve_inputs(
    rust_json: Option<&Path>,
    rust_project: Option<&Path>,
    lean_json: Option<&Path>,
    lean_project: Option<&Path>,
) -> Result<(PathBuf, PathBuf), String> {
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
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
        }

        println!("Extracting Rust and Lean atoms in parallel...\n");
        let (rust_result, lean_result) = std::thread::scope(|s| {
            let rust_handle =
                s.spawn(|| extract_runner::run_probe_rust_extract(rust_proj, probes_dir_ref));
            let lean_handle =
                s.spawn(|| extract_runner::run_probe_lean_extract(lean_proj, probes_dir_ref));
            (rust_handle.join(), lean_handle.join())
        });

        let rust_path =
            rust_result.map_err(|_| "Rust extraction thread panicked".to_string())??;
        let lean_path =
            lean_result.map_err(|_| "Lean extraction thread panicked".to_string())??;
        Ok((rust_path, lean_path))
    } else {
        let rust_path = if let Some(json) = rust_json {
            json.to_path_buf()
        } else {
            if let Some(dir) = probes_dir_ref {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
            }
            extract_runner::run_probe_rust_extract(rust_project.unwrap(), probes_dir_ref)?
        };

        let lean_path = if let Some(json) = lean_json {
            json.to_path_buf()
        } else {
            if let Some(dir) = probes_dir_ref {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
            }
            extract_runner::run_probe_lean_extract(lean_project.unwrap(), probes_dir_ref)?
        };

        Ok((rust_path, lean_path))
    }
}

/// Resolve functions.json: use provided path, generate from Lean source, or
/// fall back to `lake exe listfuns`.
fn resolve_functions(
    functions_json: Option<&Path>,
    lean_project: Option<&Path>,
    use_lake: bool,
) -> Result<PathBuf, String> {
    if let Some(path) = functions_json {
        return Ok(path.to_path_buf());
    }

    let lean_proj =
        lean_project.ok_or("Cannot auto-generate functions.json without --lean-project")?;
    let functions_path = lean_proj.join("functions.json");

    if use_lake {
        run_listfuns(lean_proj, &functions_path)?;
    } else {
        generate_functions_json(lean_proj, &functions_path)?;
    }
    Ok(functions_path)
}

/// Run the translate step, returning bidirectional maps and the set of
/// normalized Rust names found in `functions.json`.
fn run_translate(
    rust_path: &Path,
    lean_path: &Path,
    functions_path: &Path,
) -> Result<(TranslationMaps, HashSet<String>), String> {
    println!("Loading Rust atoms from {}...", rust_path.display());
    let rust_data = load_atoms(rust_path)?;
    println!("  {} atoms", rust_data.len());

    println!("Loading Lean atoms from {}...", lean_path.display());
    let lean_data = load_atoms(lean_path)?;
    println!("  {} atoms", lean_data.len());

    println!("Loading functions from {}...", functions_path.display());
    let functions = load_functions(functions_path)?;
    println!("  {} entries", functions.len());

    let funs_rust_names = build_functions_rust_names(&functions);

    println!("\nGenerating translations...");
    let (mappings, stats) = generate_translations(&rust_data, &lean_data, &functions);

    println!("  {} translations generated", mappings.len());
    for (conf, count) in &stats.by_confidence {
        println!("    {conf}: {count}");
    }

    let mut from_to = HashMap::new();
    let mut to_from = HashMap::new();
    for m in &mappings {
        from_to.insert(m.from.clone(), m.to.clone());
        to_from.insert(m.to.clone(), m.from.clone());
    }

    Ok(((from_to, to_from), funs_rust_names))
}

/// Merge atoms with pre-computed translations and produce the final output.
///
/// The pipeline has three clearly separated phases:
/// 1. **Merge** — generic `probe merge` operation via `merge_atom_files`.
/// 2. **Enrich** — Aeneas-specific metadata (`translation-*`, `is-disabled`).
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
    translations: &TranslationMaps,
    funs_rust_names: &HashSet<String>,
    output_path: Option<&Path>,
    config: &AeneasConfig,
    rust_path_prefix: Option<&str>,
    project_root: Option<&Path>,
) -> Result<(), String> {
    // Phase 1: Merge (generic probe operation)
    println!("\nMerging atoms with translations...");
    let (mut merged, provenance, stats) =
        merge_atom_files(&[rust_path, lean_path], Some(translations))?;

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
    enrich_with_aeneas_metadata(&mut merged, &translations.0, funs_rust_names);
    enrich::enrich_lean_atom_flags(&mut merged, rust_crate_name, config);

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

/// Add Aeneas-specific metadata to merged atoms.
///
/// Two enrichment passes:
/// 1. For each Rust atom with a Lean translation, set `translation-name`,
///    `translation-path`, and `translation-text` from the Lean atom.
/// 2. For every Rust atom, set `is-disabled` to `false` when its
///    `rust-qualified-name` appears in `functions.json` **or** it already
///    has a `translation-name` from pass 1 (defensive: a translation found
///    by any strategy means Aeneas processed the function).
fn enrich_with_aeneas_metadata(
    merged: &mut std::collections::BTreeMap<String, Atom>,
    from_to: &HashMap<String, String>,
    funs_rust_names: &HashSet<String>,
) {
    let enrichments: Vec<_> = from_to
        .iter()
        .filter_map(|(rust_name, lean_name)| {
            merged.get(lean_name).map(|lean_atom| {
                (
                    rust_name.clone(),
                    lean_name.clone(),
                    lean_atom.code_path.clone(),
                    lean_atom.code_text.lines_start,
                    lean_atom.code_text.lines_end,
                )
            })
        })
        .collect();

    for (rust_name, lean_name, path, start, end) in enrichments {
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
        }
    }

    for atom in merged.values_mut() {
        if atom.language == "rust" {
            let in_functions = atom
                .extensions
                .get("rust-qualified-name")
                .and_then(|v| v.as_str())
                .is_some_and(|rqn| funs_rust_names.contains(&normalize_rust_name(rqn)));
            let has_translation = atom.extensions.contains_key("translation-name");
            let aeneas_processed = in_functions || has_translation;
            atom.extensions.insert(
                "is-disabled".to_string(),
                serde_json::json!(!aeneas_processed),
            );
            atom.extensions.insert(
                "is-relevant".to_string(),
                serde_json::json!(aeneas_processed),
            );
            if !atom.extensions.contains_key("is-public") {
                atom.extensions
                    .insert("is-public".to_string(), serde_json::json!(false));
            }
        }
    }
}

/// Construct and write the Aeneas extract envelope.
fn write_aeneas_envelope(
    merged: std::collections::BTreeMap<String, Atom>,
    provenance: Vec<InputProvenance>,
    output_path: &Path,
    stats: &probe::commands::merge::MergeStats,
) -> Result<(), String> {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let envelope = MergedAtomEnvelope {
        schema: "probe-aeneas/extract".to_string(),
        schema_version: "2.0".to_string(),
        tool: Tool {
            name: "probe-aeneas".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: "extract".to_string(),
        },
        inputs: provenance,
        timestamp,
        data: merged,
    };

    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| format!("Failed to serialize output: {e}"))?;

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }

    std::fs::write(output_path, format!("{json}\n"))
        .map_err(|e| format!("Failed to write {}: {e}", output_path.display()))?;

    println!("\nOutput: {}", output_path.display());
    println!("  Total entries:    {}", stats.total_entries);
    println!("  Stubs remaining:  {}", stats.stubs_remaining);
    println!("  New entries added: {}", stats.entries_added);
    println!("  Cross-lang edges: {}", stats.translations_applied);

    Ok(())
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
    output_path: &Path,
) -> Result<(), String> {
    println!("Loading Rust atoms from {}...", rust_path.display());
    let rust_data = load_atoms(rust_path)?;
    println!("  {} atoms", rust_data.len());

    println!("Loading Lean atoms from {}...", lean_path.display());
    let lean_data = load_atoms(lean_path)?;
    println!("  {} atoms", lean_data.len());

    println!("Loading functions from {}...", functions_path.display());
    let functions = load_functions(functions_path)?;
    println!("  {} entries", functions.len());

    println!("\nGenerating translations...");
    let (mappings, stats) = generate_translations(&rust_data, &lean_data, &functions);

    println!("  {} translations generated", mappings.len());
    for (conf, count) in &stats.by_confidence {
        println!("    {conf}: {count}");
    }

    let rust_raw: serde_json::Value = {
        let content = std::fs::read_to_string(rust_path)
            .map_err(|e| format!("Failed to read {}: {e}", rust_path.display()))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {e}", rust_path.display()))?
    };
    let lean_raw: serde_json::Value = {
        let content = std::fs::read_to_string(lean_path)
            .map_err(|e| format!("Failed to read {}: {e}", lean_path.display()))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {e}", lean_path.display()))?
    };

    let json_value = build_translations_json(&mappings, &rust_raw, &lean_raw);
    let json = serde_json::to_string_pretty(&json_value)
        .map_err(|e| format!("Failed to serialize translations: {e}"))?;
    std::fs::write(output_path, format!("{json}\n"))
        .map_err(|e| format!("Failed to write {}: {e}", output_path.display()))?;

    println!("\nWritten to {}", output_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
        let err = resolve_project(tmp.path()).unwrap_err();
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

        let err = resolve_project(&project).unwrap_err();
        assert!(err.contains("Cargo.toml"), "Error: {err}");
    }

    #[test]
    fn resolve_project_missing_lakefile() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("nolake");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("Cargo.toml"), "[package]\nname = \"t\"").unwrap();
        fs::write(project.join("aeneas-config.yml"), "crate:\n  dir: \".\"\n").unwrap();

        let err = resolve_project(&project).unwrap_err();
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
        let funs_rust_names = &HashSet::new();

        enrich_with_aeneas_metadata(&mut merged, &from_to, funs_rust_names);

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
        let funs_rust_names = &HashSet::new();

        enrich_with_aeneas_metadata(&mut merged, &from_to, funs_rust_names);

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
        let funs_rust_names = &HashSet::new();

        enrich_with_aeneas_metadata(&mut merged, &from_to, funs_rust_names);

        let atom = merged.get("probe:module.lean_fn").unwrap();
        assert!(
            !atom.extensions.contains_key("is-public"),
            "Lean atoms should not get is-public"
        );
    }

    #[test]
    fn enrich_translation_overrides_is_disabled() {
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

        let mut from_to = HashMap::new();
        from_to.insert(
            "probe:my-crate/1.0/ristretto/decompress/step_2()".to_string(),
            "probe:my_crate.ristretto.decompress.step_2".to_string(),
        );
        // The rust-qualified-name does NOT appear in funs_rust_names (name mismatch).
        let funs_rust_names = HashSet::new();

        enrich_with_aeneas_metadata(&mut merged, &from_to, &funs_rust_names);

        let atom = merged
            .get("probe:my-crate/1.0/ristretto/decompress/step_2()")
            .unwrap();
        assert_eq!(
            atom.extensions.get("is-disabled"),
            Some(&serde_json::json!(false)),
            "atom with translation should not be disabled even if RQN not in functions.json"
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

    fn make_provenance(pkg: &str, ver: &str) -> InputProvenance {
        InputProvenance {
            schema: "probe-rust/extract".to_string(),
            source: probe::types::Source {
                repo: String::new(),
                commit: String::new(),
                language: "rust".to_string(),
                package: pkg.to_string(),
                package_version: ver.to_string(),
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
