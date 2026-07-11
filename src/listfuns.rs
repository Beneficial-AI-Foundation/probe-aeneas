//! `listfuns` subcommand: generate `functions.json` from a Lean project.
//!
//! Either delegates to `lake exe listfuns` directly ([`run_listfuns`]) or
//! parses Aeneas-generated Lean sources and enriches them with verification
//! data from `probe-lean` ([`run_enriched_listfuns`]).
//!
//! ## Error model
//!
//! Functions return [`Result<T, ListfunsError>`]. Categorical failures
//! (subprocess exit, missing output, non-UTF-8 path) are typed variants.
//! Open-ended errors flow through `Other(#[from] anyhow::Error)` via
//! `.context("...")?`. Errors from `extract_runner` propagate through the
//! `ExtractRunner` variant via `#[from]`.
//!
//! Errors propagate to `main.rs` via `.map_err(anyhow::Error::new)`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use probe::types::Atom;

use crate::aeneas_config::AeneasConfig;
use crate::enrich::{self, EnrichedFunctionsFile};
use crate::extract_runner::{self, ExtractRunnerError};
use crate::gen_functions;

// ---------------------------------------------------------------------------
// Typed error
// ---------------------------------------------------------------------------

/// Errors produced by the `listfuns` module.
#[derive(Debug, thiserror::Error)]
pub enum ListfunsError {
    /// A subprocess exited with a non-zero status code.
    #[error("{command} exited with status {code}")]
    SubprocessFailed { command: String, code: i32 },

    /// A subprocess completed but the expected output file is missing.
    #[error("{command} completed but {} was not created", path.display())]
    MissingOutput { command: String, path: PathBuf },

    /// A path could not be represented as UTF-8.
    #[error("{label} path is not valid UTF-8")]
    NonUtf8Path { label: &'static str },

    /// Errors propagated from `extract_runner`.
    #[error(transparent)]
    ExtractRunner(#[from] ExtractRunnerError),

    /// Catch-all for context-chained errors built via `anyhow`.
    ///
    /// Used for io::Error, serde_json::Error, and the `String`-error bridge
    /// from `probe::commands::merge::merge_atom_files` (not yet migrated).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout this module.
pub type Result<T> = std::result::Result<T, ListfunsError>;

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Run `lake exe listfuns <output>` in the given Lean project directory.
pub fn run_listfuns(lean_project: &Path, output: &Path) -> Result<()> {
    let output_str = output
        .to_str()
        .ok_or(ListfunsError::NonUtf8Path { label: "Output" })?;

    println!(
        "Running `lake exe listfuns {output_str}` in {}...",
        lean_project.display()
    );

    let status = Command::new("lake")
        .args(["exe", "listfuns", output_str])
        .current_dir(lean_project)
        .status()
        .context("spawn `lake exe listfuns`")?;

    if !status.success() {
        return Err(ListfunsError::SubprocessFailed {
            command: "lake exe listfuns".to_string(),
            code: status.code().unwrap_or(-1),
        });
    }

    if !output.exists() {
        return Err(ListfunsError::MissingOutput {
            command: "lake exe listfuns".to_string(),
            path: output.to_path_buf(),
        });
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
) -> Result<()> {
    let mut records =
        gen_functions::parse_aeneas_project(lean_project).context("parse Aeneas project")?;
    println!(
        "Parsed {} function entries from Aeneas files",
        records.len()
    );

    // Overlay Aeneas's translation.json (authoritative loop/primary
    // classification) when present. Optional: heuristic fallback otherwise.
    // `resolve_path` honors `aeneas_args.dest`, matching the `extract` flow.
    let translation_path = crate::translation_manifest::resolve_path(lean_project);
    let aux_defs = crate::translation_manifest::apply(&mut records, translation_path.as_deref());

    let atoms = load_atoms(lean_project, atoms_path, module_prefix)?;
    println!("Loaded {} atoms from probe-lean", atoms.len());

    let config =
        AeneasConfig::load(aeneas_config_path, Some(lean_project)).context("load aeneas config")?;

    let rust_crate_name = detect_crate_name(&records);
    println!("Detected crate name: {rust_crate_name:?}");

    let enriched =
        enrich::enrich_function_records(&records, &atoms, &rust_crate_name, &config, &aux_defs);

    let output_json = EnrichedFunctionsFile {
        functions: enriched,
    };
    let json =
        serde_json::to_string_pretty(&output_json).context("serialize enriched functions.json")?;
    std::fs::write(output, format!("{json}\n"))
        .with_context(|| format!("write {}", output.display()))?;

    println!("\nWritten to {}", output.display());
    Ok(())
}

/// Load atoms either from a pre-computed file or by running probe-lean extract.
fn load_atoms(
    lean_project: &Path,
    atoms_path: Option<&Path>,
    module_prefix: Option<&str>,
) -> Result<BTreeMap<String, Atom>> {
    let json_path = match atoms_path {
        Some(p) => {
            println!("Using pre-computed atoms from {}", p.display());
            p.to_path_buf()
        }
        None => {
            // ExtractRunnerError -> ListfunsError::ExtractRunner via #[from].
            extract_runner::run_probe_lean_extract_with_opts(lean_project, module_prefix, None)?
        }
    };

    let atoms = crate::translate::load_atoms(&json_path)
        .with_context(|| format!("load atoms from {}", json_path.display()))?;
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
            ..Default::default()
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
