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
    println!(
        "Parsed {} function entries from Aeneas files",
        records.len()
    );

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
        None => {
            extract_runner::run_probe_lean_extract_with_opts(lean_project, module_prefix, None)?
        }
    };

    crate::translate::load_atoms(&json_path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionRecord;

    fn rec(source: Option<&str>) -> FunctionRecord {
        FunctionRecord {
            lean_name: "test".to_string(),
            rust_name: None,
            source: source.map(String::from),
            lines: None,
            is_hidden: false,
            is_extraction_artifact: false,
        }
    }

    #[test]
    fn detect_crate_from_crate_prefixed_path() {
        let records = vec![rec(Some("curve25519-dalek/src/scalar.rs"))];
        assert_eq!(detect_crate_name(&records), "curve25519-dalek");
    }

    #[test]
    fn detect_crate_from_src_path() {
        let records = vec![rec(Some("src/backend/serial/u64/field.rs"))];
        assert_eq!(detect_crate_name(&records), "backend");
    }

    #[test]
    fn detect_crate_skips_absolute_paths() {
        let records = vec![
            rec(Some("/rustc/library/core/src/borrow.rs")),
            rec(Some("mycrate/src/lib.rs")),
        ];
        assert_eq!(detect_crate_name(&records), "mycrate");
    }

    #[test]
    fn detect_crate_skips_cargo_registry() {
        let records = vec![
            rec(Some("foo/cargo/registry/src/dep/lib.rs")),
            rec(Some("mycrate/src/lib.rs")),
        ];
        assert_eq!(detect_crate_name(&records), "mycrate");
    }

    #[test]
    fn detect_crate_empty_when_no_sources() {
        let records = vec![rec(None), rec(None)];
        assert_eq!(detect_crate_name(&records), "");
    }

    #[test]
    fn detect_crate_empty_for_empty_records() {
        let records: Vec<FunctionRecord> = vec![];
        assert_eq!(detect_crate_name(&records), "");
    }
}
