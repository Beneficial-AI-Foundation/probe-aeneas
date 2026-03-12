use std::collections::BTreeMap;
use std::path::Path;

use probe::commands::merge::merge_atom_maps;
use probe::types::{Atom, InputProvenance, MergedAtomEnvelope, Tool};

use crate::listfuns::run_listfuns;
use crate::translate::{
    build_translations_json, generate_translations, load_atoms, load_functions,
};

/// Run the full merge pipeline: listfuns -> translate -> merge.
pub fn run_merge(
    rust_path: &Path,
    lean_path: &Path,
    lean_project: &Path,
    output_path: &Path,
) -> Result<(), String> {
    // Step 1: Generate functions.json via lake exe listfuns
    let functions_path = lean_project.join("functions.json");
    run_listfuns(lean_project, &functions_path)?;

    // Step 2: Generate translations
    let translations_result =
        run_translate(rust_path, lean_path, &functions_path)?;

    // Step 3: Merge with translations
    run_merge_with_translations(rust_path, lean_path, &translations_result, output_path)
}

/// Run just the translate step, returning the bidirectional maps and the translations JSON.
fn run_translate(
    rust_path: &Path,
    lean_path: &Path,
    functions_path: &Path,
) -> Result<(std::collections::HashMap<String, String>, std::collections::HashMap<String, String>), String> {
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

    // Build bidirectional maps
    let mut from_to = std::collections::HashMap::new();
    let mut to_from = std::collections::HashMap::new();
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
    translations: &(std::collections::HashMap<String, String>, std::collections::HashMap<String, String>),
    output_path: &Path,
) -> Result<(), String> {
    // Load atom files with provenance
    let (rust_atoms, rust_prov) = probe::types::load_atom_file(rust_path)?;
    let (lean_atoms, lean_prov) = probe::types::load_atom_file(lean_path)?;

    println!("\nMerging {} + {} atoms with translations...", rust_atoms.len(), lean_atoms.len());

    let (merged, stats) = merge_atom_maps(vec![rust_atoms, lean_atoms], Some(translations));

    // Build merged envelope
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

    // Read raw envelopes for source metadata
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
