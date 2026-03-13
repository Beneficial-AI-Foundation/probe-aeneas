use std::collections::HashMap;
use std::path::{Path, PathBuf};

use probe::commands::merge::merge_atom_maps;
use probe::types::{InputProvenance, MergedAtomEnvelope, Tool};

use crate::extract_runner;
use crate::listfuns::run_listfuns;
use crate::translate::{
    build_translations_json, generate_translations, load_atoms, load_functions,
};

type TranslationMaps = (HashMap<String, String>, HashMap<String, String>);

/// Run the full merge pipeline with the new flexible input resolution.
///
/// Accepts either pre-generated JSON paths or project paths for Rust and Lean.
/// When project paths are given, the corresponding extractors are run automatically.
pub fn run_merge(
    rust_json: Option<&Path>,
    rust_project: Option<&Path>,
    lean_json: Option<&Path>,
    lean_project: Option<&Path>,
    functions_json: Option<&Path>,
    output_path: &Path,
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
        return Err(
            "--functions is required when --lean-project is not given \
             (cannot auto-generate functions.json without a Lean project path)"
                .to_string(),
        );
    }

    // --- Resolve inputs (extract if needed) ---
    let (rust_path, lean_path) =
        resolve_inputs(rust_json, rust_project, lean_json, lean_project)?;

    // --- Resolve functions.json ---
    let functions_path = resolve_functions(functions_json, lean_project)?;

    // --- Generate translations ---
    let translations_result = run_translate(&rust_path, &lean_path, &functions_path)?;

    // --- Merge ---
    run_merge_with_translations(&rust_path, &lean_path, &translations_result, output_path)
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

    let lean_proj = lean_project
        .ok_or("Cannot auto-generate functions.json without --lean-project")?;
    let functions_path = lean_proj.join("functions.json");
    run_listfuns(lean_proj, &functions_path)?;
    Ok(functions_path)
}

/// Run the translate step, returning bidirectional maps.
fn run_translate(
    rust_path: &Path,
    lean_path: &Path,
    functions_path: &Path,
) -> Result<TranslationMaps, String> {
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

    let mut from_to = HashMap::new();
    let mut to_from = HashMap::new();
    for m in &mappings {
        from_to.insert(m.from.clone(), m.to.clone());
        to_from.insert(m.to.clone(), m.from.clone());
    }

    Ok((from_to, to_from))
}

/// Merge atoms with pre-computed translations.
fn run_merge_with_translations(
    rust_path: &Path,
    lean_path: &Path,
    translations: &TranslationMaps,
    output_path: &Path,
) -> Result<(), String> {
    let (rust_atoms, rust_prov) = probe::types::load_atom_file(rust_path)?;
    let (lean_atoms, lean_prov) = probe::types::load_atom_file(lean_path)?;

    println!(
        "\nMerging {} + {} atoms with translations...",
        rust_atoms.len(),
        lean_atoms.len()
    );

    let (merged, stats) = merge_atom_maps(vec![rust_atoms, lean_atoms], Some(translations));

    let timestamp = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let mut all_prov: Vec<InputProvenance> = Vec::new();
    all_prov.extend(rust_prov);
    all_prov.extend(lean_prov);

    let envelope = MergedAtomEnvelope {
        schema: "probe/merged-atoms".to_string(),
        schema_version: "2.0".to_string(),
        tool: Tool {
            name: "probe-aeneas".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: "merge".to_string(),
        },
        inputs: all_prov,
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

/// Public entry point for the `translate` subcommand (no merge, just translations).
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
