use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use probe::commands::merge::merge_atom_files;
use probe::types::{Atom, InputProvenance, MergedAtomEnvelope, Tool};

use crate::aeneas_config::AeneasConfig;
use crate::extract_runner;
use crate::listfuns::run_listfuns;
use crate::translate::{
    build_functions_rust_names, build_translations_json, generate_translations, load_atoms,
    load_functions, normalize_rust_name,
};

type TranslationMaps = (HashMap<String, String>, HashMap<String, String>);

/// Run the full extract pipeline with flexible input resolution.
///
/// Accepts either pre-generated JSON paths or project paths for Rust and Lean.
/// When project paths are given, the corresponding extractors are run automatically.
pub fn run_extract(
    rust_json: Option<&Path>,
    rust_project: Option<&Path>,
    lean_json: Option<&Path>,
    lean_project: Option<&Path>,
    functions_json: Option<&Path>,
    output_path: Option<&Path>,
    aeneas_config: Option<&Path>,
) -> Result<(), String> {
    // --- Validate inputs ---
    if rust_json.is_none() && rust_project.is_none() {
        return Err(
            "Must provide either --rust (JSON path) or --rust-project (project path)".to_string(),
        );
    }
    if lean_json.is_none() && lean_project.is_none() {
        return Err(
            "Must provide either --lean (JSON path) or --lean-project (project path)".to_string(),
        );
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
    let functions_path = resolve_functions(functions_json, lean_project)?;

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
    )
}

/// Resolve Rust and Lean inputs, running extractors in parallel when both are
/// project paths.
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

    if need_rust_extract && need_lean_extract {
        let rust_proj = rust_project.unwrap();
        let lean_proj = lean_project.unwrap();

        println!("Extracting Rust and Lean atoms in parallel...\n");
        let (rust_result, lean_result) = std::thread::scope(|s| {
            let rust_handle = s.spawn(|| extract_runner::run_probe_rust_extract(rust_proj));
            let lean_handle = s.spawn(|| extract_runner::run_probe_lean_extract(lean_proj));
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
            extract_runner::run_probe_rust_extract(rust_project.unwrap())?
        };

        let lean_path = if let Some(json) = lean_json {
            json.to_path_buf()
        } else {
            extract_runner::run_probe_lean_extract(lean_project.unwrap())?
        };

        Ok((rust_path, lean_path))
    }
}

/// Resolve functions.json: use provided path or auto-generate via listfuns.
fn resolve_functions(
    functions_json: Option<&Path>,
    lean_project: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some(path) = functions_json {
        return Ok(path.to_path_buf());
    }

    let lean_proj =
        lean_project.ok_or("Cannot auto-generate functions.json without --lean-project")?;
    let functions_path = lean_proj.join("functions.json");
    run_listfuns(lean_proj, &functions_path)?;
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
/// When `output_path` is `None`, derives `aeneas_{package}_{version}.json`
/// from the Rust input's envelope metadata.
fn run_extract_with_translations(
    rust_path: &Path,
    lean_path: &Path,
    translations: &TranslationMaps,
    funs_rust_names: &HashSet<String>,
    output_path: Option<&Path>,
    config: &AeneasConfig,
) -> Result<(), String> {
    // Phase 1: Merge (generic probe operation)
    println!("\nMerging atoms with translations...");
    let (mut merged, provenance, stats) =
        merge_atom_files(&[rust_path, lean_path], Some(translations))?;

    let output_path_buf;
    let output_path = match output_path {
        Some(p) => p,
        None => {
            output_path_buf = default_output_path(&provenance);
            &output_path_buf
        }
    };

    let rust_crate_name = provenance
        .iter()
        .find(|p| p.source.language == "rust")
        .map(|p| p.source.package.as_str())
        .unwrap_or("");

    // Phase 2: Enrich (Aeneas-specific)
    enrich_with_aeneas_metadata(&mut merged, &translations.0, funs_rust_names);
    enrich_lean_atom_flags(&mut merged, rust_crate_name, config);

    // Phase 3: Write envelope
    write_aeneas_envelope(merged, provenance, output_path, &stats)
}

/// Add Aeneas-specific metadata to merged atoms.
///
/// Two enrichment passes:
/// 1. For each Rust atom with a Lean translation, set `translation-name`,
///    `translation-path`, and `translation-text` from the Lean atom.
/// 2. For every Rust atom, set `is-disabled` based on whether its
///    `rust-qualified-name` appears in `functions.json`.
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
            atom.extensions.insert(
                "translation-text".to_string(),
                serde_json::json!({
                    "lines-start": start,
                    "lines-end": end,
                }),
            );
        }
    }

    for atom in merged.values_mut() {
        if atom.language == "rust" {
            let in_functions = atom
                .extensions
                .get("rust-qualified-name")
                .and_then(|v| v.as_str())
                .is_some_and(|rqn| funs_rust_names.contains(&normalize_rust_name(rqn)));
            atom.extensions
                .insert("is-disabled".to_string(), serde_json::json!(!in_functions));
            atom.extensions
                .insert("is-relevant".to_string(), serde_json::json!(in_functions));
        }
    }
}

/// Standard Aeneas extraction artifact suffixes.
const AENEAS_ARTIFACT_SUFFIXES: &[&str] = &["_body", "_loop", "_loop0", "_loop1", "_loop2", "_loop3"];

/// Aeneas-specific enrichment pass for Lean atoms.
///
/// Computes `is-hidden`, `is-extraction-artifact`, `is-relevant` (refined),
/// `is-externally-verified`, and applies optional config overrides for
/// `is-hidden` (project tail) and `is-ignored` (always manual).
fn enrich_lean_atom_flags(
    merged: &mut BTreeMap<String, Atom>,
    rust_crate_name: &str,
    config: &AeneasConfig,
) {
    let mut stats = AeneasEnrichStats::default();

    for (key, atom) in merged.iter_mut() {
        if atom.language != "lean" {
            continue;
        }

        let display_name = &atom.display_name;
        let name_no_prefix = key.strip_prefix("probe:").unwrap_or(key);

        let attrs: Vec<String> = atom
            .extensions
            .get("attributes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // --- is-extraction-artifact ---
        let is_artifact = AENEAS_ARTIFACT_SUFFIXES
            .iter()
            .any(|sfx| display_name.ends_with(sfx));
        if is_artifact {
            stats.artifacts += 1;
        }
        atom.extensions.insert(
            "is-extraction-artifact".to_string(),
            serde_json::json!(is_artifact),
        );

        // --- is-hidden ---
        let has_trait_attr = attrs.iter().any(|a| a == "rust_trait_impl");
        let has_insts_pattern = name_no_prefix.contains(".Insts.");
        let is_mutual_loop = name_no_prefix.ends_with(".mutual");
        let in_config_hidden = config.hidden.contains(name_no_prefix);
        let is_hidden = has_trait_attr || has_insts_pattern || is_mutual_loop || in_config_hidden;
        if is_hidden {
            stats.hidden += 1;
        }
        atom.extensions
            .insert("is-hidden".to_string(), serde_json::json!(is_hidden));

        // --- is-ignored (manual only, from config) ---
        let is_ignored = config.ignored.contains(name_no_prefix);
        if is_ignored {
            stats.ignored += 1;
        }
        atom.extensions
            .insert("is-ignored".to_string(), serde_json::json!(is_ignored));

        // --- is-relevant (refined for Aeneas) ---
        let rust_source = atom
            .extensions
            .get("rust-source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let is_relevant = if rust_source.is_empty() || rust_source == "null" {
            atom.extensions
                .get("is-in-package")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        } else if !rust_crate_name.is_empty() {
            rust_source.contains(rust_crate_name)
                && !rust_source.starts_with('/')
                && !rust_source.contains("/cargo/registry/")
        } else {
            true
        };
        if is_relevant {
            stats.relevant += 1;
        }
        atom.extensions
            .insert("is-relevant".to_string(), serde_json::json!(is_relevant));

        // --- is-externally-verified ---
        let is_ext_verified = attrs.iter().any(|a| a == "externally_verified");
        if is_ext_verified {
            stats.externally_verified += 1;
            atom.extensions.insert(
                "is-externally-verified".to_string(),
                serde_json::json!(true),
            );
        }
    }

    println!("\nAeneas enrichment (Lean atoms):");
    println!("  Extraction artifacts: {}", stats.artifacts);
    println!("  Hidden:              {}", stats.hidden);
    println!("  Ignored:             {}", stats.ignored);
    println!("  Relevant:            {}", stats.relevant);
    if stats.externally_verified > 0 {
        println!("  Externally verified: {}", stats.externally_verified);
    }
}

#[derive(Default)]
struct AeneasEnrichStats {
    artifacts: usize,
    hidden: usize,
    ignored: usize,
    relevant: usize,
    externally_verified: usize,
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
    std::fs::write(output_path, format!("{json}\n"))
        .map_err(|e| format!("Failed to write {}: {e}", output_path.display()))?;

    println!("\nOutput: {}", output_path.display());
    println!("  Total entries:    {}", stats.total_entries);
    println!("  Stubs remaining:  {}", stats.stubs_remaining);
    println!("  New entries added: {}", stats.entries_added);
    println!("  Cross-lang edges: {}", stats.translations_applied);

    Ok(())
}

/// Derive `aeneas_{package}_{version}.json` from Rust input provenance.
fn default_output_path(rust_prov: &[InputProvenance]) -> PathBuf {
    let (pkg, ver) = rust_prov
        .first()
        .map(|p| (p.source.package.as_str(), p.source.package_version.as_str()))
        .unwrap_or(("unknown", "0.0.0"));

    let name = if ver.is_empty() {
        format!("aeneas_{pkg}.json")
    } else {
        format!("aeneas_{pkg}_{ver}.json")
    };
    PathBuf::from(name)
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
