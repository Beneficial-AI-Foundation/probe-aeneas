use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use probe::types::Atom;

use crate::aeneas_config::AeneasConfig;
use crate::enrich::{self, EnrichedFunctionsFile};
use crate::extract_runner;
use crate::gen_functions;

/// Run `lake exe listfuns <output>` in the given Lean project directory.
pub fn run_listfuns(lean_project: &Path, output: &Path) -> Result<(), String> {
    let output_str = output
        .to_str()
        .ok_or_else(|| "Output path is not valid UTF-8".to_string())?;

    println!(
        "Running `lake exe listfuns {output_str}` in {}...",
        lean_project.display()
    );

    let status = Command::new("lake")
        .args(["exe", "listfuns", output_str])
        .current_dir(lean_project)
        .status()
        .map_err(|e| format!("Failed to run `lake exe listfuns`: {e}"))?;

    if !status.success() {
        return Err(format!(
            "`lake exe listfuns` exited with status {}",
            status.code().unwrap_or(-1)
        ));
    }

    if !output.exists() {
        return Err(format!(
            "`lake exe listfuns` completed but {} was not created",
            output.display()
        ));
    }

    println!("  Generated {}", output.display());
    Ok(())
}

/// Generate an enriched functions.json: parse Aeneas files, run probe-lean
/// extract internally, and enrich with verification data.
///
/// When `atoms_path` is provided, skips the internal probe-lean call.
/// When `module_prefix` is provided, passes `-m <prefix>` to probe-lean.
pub fn run_enriched_listfuns(
    lean_project: &Path,
    output: &Path,
    atoms_path: Option<&Path>,
    module_prefix: Option<&str>,
    aeneas_config_path: Option<&Path>,
) -> Result<(), String> {
    let records = gen_functions::parse_aeneas_project(lean_project)?;
    println!("Parsed {} function entries from Aeneas files", records.len());

    let atoms = load_atoms(lean_project, atoms_path, module_prefix)?;
    println!("Loaded {} atoms from probe-lean", atoms.len());

    let config = AeneasConfig::load(aeneas_config_path, Some(lean_project))?;

    let rust_crate_name = detect_crate_name(&records);
    println!("Detected crate name: {rust_crate_name:?}");

    let enriched = enrich::enrich_function_records(&records, &atoms, &rust_crate_name, &config);

    let output_json = EnrichedFunctionsFile {
        functions: enriched,
    };
    let json = serde_json::to_string_pretty(&output_json)
        .map_err(|e| format!("Failed to serialize enriched functions.json: {e}"))?;
    std::fs::write(output, format!("{json}\n"))
        .map_err(|e| format!("Failed to write {}: {e}", output.display()))?;

    println!("\nWritten to {}", output.display());
    Ok(())
}

/// Load atoms either from a pre-computed file or by running probe-lean extract.
fn load_atoms(
    lean_project: &Path,
    atoms_path: Option<&Path>,
    module_prefix: Option<&str>,
) -> Result<BTreeMap<String, Atom>, String> {
    let json_path = match atoms_path {
        Some(p) => {
            println!("Using pre-computed atoms from {}", p.display());
            p.to_path_buf()
        }
        None => extract_runner::run_probe_lean_extract_with_opts(lean_project, module_prefix)?,
    };

    let content = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("Failed to read atoms JSON {}: {e}", json_path.display()))?;

    let envelope: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse atoms JSON: {e}"))?;

    let data = envelope
        .get("data")
        .ok_or_else(|| "Atoms JSON missing 'data' field".to_string())?;

    let atoms: BTreeMap<String, Atom> = serde_json::from_value(data.clone())
        .map_err(|e| format!("Failed to deserialize atoms data: {e}"))?;

    Ok(atoms)
}

/// Heuristically detect the Rust crate name from function records' source paths.
fn detect_crate_name(records: &[crate::types::FunctionRecord]) -> String {
    for rec in records {
        if let Some(src) = &rec.source {
            if !src.starts_with('/') && !src.contains("/cargo/registry/") {
                if let Some(first_dir) = src.split('/').next() {
                    if first_dir != "src" {
                        return first_dir.to_string();
                    }
                }
                return src
                    .strip_prefix("src/")
                    .and_then(|s| s.split('/').next())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }
    String::new()
}
