//! `function_source` module: the single dispatch that resolves Aeneas
//! [`FunctionRecord`]s from their source, plus the auxiliary-def name set from
//! Aeneas's `translation.json`.
//!
//! This is the **only** module that chooses between the `translation.json`
//! manifest and the legacy docstring scraper. Downstream code (`translate`,
//! `enrich`, the merge/envelope steps) only ever sees `Vec<FunctionRecord>` +
//! the auxiliary-def set, never the manifest path — so once every Aeneas
//! project ships a `translation.json`, retiring the legacy path is a localized
//! change here: drop the [`RecordSource::LegacyScrape`] arm and the
//! `gen_functions` scraper, and nothing downstream needs to change.
//!
//! ## Error model
//!
//! [`resolve`] returns [`anyhow::Result`]; sub-module errors
//! ([`crate::gen_functions`], [`crate::listfuns`], [`crate::translate`]) are
//! lifted into `anyhow` so callers propagate them through their own
//! `Other(anyhow::Error)` catch-alls.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Context as _;

use crate::gen_functions;
use crate::listfuns;
use crate::translate;
use crate::translation_manifest;
use crate::types::FunctionRecord;

/// Which source produced the records. Lets callers decide artifact handling
/// (e.g. whether to (re)write `functions.json`) without re-deriving the branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordSource {
    /// User-supplied `functions.json` (`--functions`); already on disk.
    Override,
    /// Built directly from Aeneas's `translation.json`.
    Manifest,
    /// `lake exe listfuns`; writes `<lean_project>/functions.json`.
    Lake,
    /// Legacy: parsed from Aeneas-generated `.lean` docstrings; nothing written.
    LegacyScrape,
}

/// Resolved function records plus derived metadata, source-blind for downstream.
pub struct ResolvedRecords {
    pub records: Vec<FunctionRecord>,
    /// Auxiliary-def Lean names from `translation.json` (empty when absent).
    pub aux_defs: HashSet<String>,
    /// How `records` were produced.
    pub source: RecordSource,
    /// charon version recorded in `translation.json`, for provenance-gating the
    /// `def_id` join. `None` when no manifest is available (lake/legacy arms).
    pub charon_version: Option<String>,
    /// Crate name recorded in `translation.json`, for the template-name
    /// resolver. `None` when no manifest is available (lake/legacy arms).
    pub crate_name: Option<String>,
}

/// Resolve function records in memory.
///
/// Precedence: explicit `--functions` override, then Aeneas's `translation.json`
/// (built directly), then `lake exe listfuns`, then the legacy docstring scrape.
/// `lean_project` is required for the lake/legacy arms (not for override, and
/// not for the manifest arm, which reads the manifest file directly).
///
/// The `Override` arm still overlays the manifest onto the loaded file (a
/// user-supplied `functions.json` lacks the authoritative loop/primary fields).
/// The `Manifest` arm needs no overlay — its records carry those fields at build
/// time. Once every project ships a `translation.json`, the `Lake`/`LegacyScrape`
/// arms (and the `gen_functions` scraper) can be deleted without touching
/// downstream code.
pub fn resolve(
    lean_project: Option<&Path>,
    functions_override: Option<&Path>,
    translation_json: Option<&Path>,
    use_lake: bool,
) -> anyhow::Result<ResolvedRecords> {
    // Override: honor the user's file, enriched by the manifest overlay if present.
    if let Some(path) = functions_override {
        let mut records = translate::load_functions(path)?;
        let overlay = translation_manifest::apply(&mut records, translation_json);
        return Ok(ResolvedRecords {
            records,
            aux_defs: overlay.aux_defs,
            source: RecordSource::Override,
            charon_version: overlay.charon_version,
            crate_name: overlay.crate_name,
        });
    }

    // Manifest: the authoritative producer. On a load error, warn and fall
    // through to the legacy arms rather than aborting the pipeline.
    if let Some(tj) = translation_json {
        match translation_manifest::load(tj) {
            Ok(manifest) => {
                let records = translation_manifest::records_from_manifest(&manifest);
                let aux_defs = manifest.auxiliary_lean_names();
                println!(
                    "  Built {} function records from {}",
                    records.len(),
                    tj.display()
                );
                return Ok(ResolvedRecords {
                    records,
                    aux_defs,
                    source: RecordSource::Manifest,
                    charon_version: manifest.charon_version,
                    crate_name: manifest.crate_name,
                });
            }
            Err(e) => {
                eprintln!("  Warning: could not read {}: {e:#}", tj.display());
                eprintln!(
                    "  Falling back to {}.",
                    if use_lake {
                        "lake exe listfuns"
                    } else {
                        "legacy docstring scraping"
                    }
                );
            }
        }
    }

    // Legacy / lake (manifest absent or unreadable): no overlay applies here.
    let lean_project = lean_project.context(
        "function_source::resolve needs a Lean project when no --functions path is given",
    )?;
    let (records, source) = if use_lake {
        let path = lean_project.join("functions.json");
        listfuns::run_listfuns(lean_project, &path).map_err(anyhow::Error::new)?;
        (translate::load_functions(&path)?, RecordSource::Lake)
    } else {
        (
            gen_functions::parse_aeneas_project(lean_project).map_err(anyhow::Error::new)?,
            RecordSource::LegacyScrape,
        )
    };

    Ok(ResolvedRecords {
        records,
        aux_defs: HashSet::new(),
        source,
        charon_version: None,
        crate_name: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_functions::write_functions_json;

    const MANIFEST: &str = r#"{
        "charon_version": "0.1.304",
        "crate": "spqr",
        "functions": [
            {"def_id": 43, "lean_name": "kdf.hkdf_to_slice",
             "lean_file": "SrcTranslated/FunsExternal_Template.lean",
             "rust_name": "spqr::kdf::hkdf_to_slice", "is_local": true,
             "source": {"file": "src/kdf.rs", "begin_line": 14, "end_line": 18}},
            {"def_id": 42, "lean_name": "spqr.kdf.hkdf_to_vec",
             "lean_file": "SrcTranslated/Funs.lean",
             "rust_name": "spqr::kdf::hkdf_to_vec", "is_local": true,
             "source": {"file": "src/kdf.rs", "begin_line": 5, "end_line": 12}},
            {"def_id": 41, "lean_name": "spqr.a.first",
             "lean_file": "SrcTranslated/Funs.lean",
             "rust_name": "spqr::a::first", "is_local": true,
             "source": {"file": "src/a.rs", "begin_line": 1, "end_line": 3}}
        ]
    }"#;

    /// `functions` and template records compared on the fields that matter
    /// for matching.
    fn summary(records: &[FunctionRecord]) -> Vec<(String, Option<u64>, bool, bool)> {
        records
            .iter()
            .map(|r| {
                (
                    r.lean_name.clone(),
                    r.def_id,
                    r.def_id_is_fun_decl,
                    r.is_local_template,
                )
            })
            .collect()
    }

    fn lean_atom() -> probe::types::Atom {
        serde_json::from_value(serde_json::json!({
            "display-name": "f", "dependencies": [], "code-module": "M",
            "code-path": "SrcTranslated/Funs.lean",
            "code-text": {"lines-start": 1, "lines-end": 2},
            "kind": "def", "language": "lean",
        }))
        .unwrap()
    }

    /// SPQR's `hkdf_to_slice` Rust atom, joinable by `charon-def-id` 43.
    fn slice_rust_atoms() -> std::collections::BTreeMap<String, probe::types::Atom> {
        let mut atom = lean_atom();
        atom.language = "rust".to_string();
        atom.code_path = "src/kdf.rs".to_string();
        atom.code_text.lines_start = 14;
        atom.code_text.lines_end = 18;
        atom.extensions
            .insert("charon-def-id".to_string(), serde_json::json!(43));
        atom.extensions
            .insert("charon-version".to_string(), serde_json::json!("0.1.304"));
        std::collections::BTreeMap::from([(
            "probe:spqr/0.1.0/kdf/hkdf_to_slice()".to_string(),
            atom,
        )])
    }

    #[test]
    fn first_and_repeat_runs_see_the_same_records() {
        let dir = tempfile::tempdir().unwrap();
        let tj = dir.path().join("translation.json");
        std::fs::write(&tj, MANIFEST).unwrap();

        // First run: Manifest arm, then the extract pipeline writes the file.
        let first = resolve(None, None, Some(&tj), false).unwrap();
        assert_eq!(first.source, RecordSource::Manifest);
        assert_eq!(first.crate_name.as_deref(), Some("spqr"));
        let fj = dir.path().join("functions.json");
        write_functions_json(&first.records, &fj).unwrap();

        // Repeat run: Override arm on that file, same manifest.
        let repeat = resolve(None, Some(&fj), Some(&tj), false).unwrap();
        assert_eq!(repeat.source, RecordSource::Override);
        assert_eq!(repeat.crate_name.as_deref(), Some("spqr"));

        // The written file lacks the template, so the Override arm appends it
        // last; the resolver puts templates last on both runs, so the order
        // the strategies see is the same.
        let lean: std::collections::BTreeMap<String, probe::types::Atom> = [
            "probe:spqr.kdf.hkdf_to_slice",
            "probe:spqr.kdf.hkdf_to_vec",
            "probe:spqr.a.first",
        ]
        .iter()
        .map(|k| (k.to_string(), lean_atom()))
        .collect();
        let first_resolved =
            translate::resolve_template_lean_names(&first.records, &lean, Some("spqr"));
        let repeat_resolved =
            translate::resolve_template_lean_names(&repeat.records, &lean, Some("spqr"));
        assert_eq!(summary(&first_resolved), summary(&repeat_resolved));

        // And the strategies produce the same mappings on both runs.
        let rust = slice_rust_atoms();
        let mappings = |r: &ResolvedRecords| {
            translate::generate_translations(
                &rust,
                &lean,
                &r.records,
                r.charon_version.as_deref(),
                r.crate_name.as_deref(),
            )
            .0
        };
        let tuples = |ms: Vec<probe::types::Mapping>| {
            ms.into_iter()
                .map(|m| (m.from, m.to, m.confidence, m.method))
                .collect::<Vec<_>>()
        };
        let first_mappings = mappings(&first);
        assert_eq!(tuples(first_mappings.clone()), tuples(mappings(&repeat)));
        assert_eq!(first_mappings.len(), 1);
        assert_eq!(first_mappings[0].to, "probe:spqr.kdf.hkdf_to_slice");
        assert_eq!(
            summary(&first_resolved).last().unwrap(),
            &("spqr.kdf.hkdf_to_slice".to_string(), Some(43), true, true)
        );
    }

    #[test]
    fn override_arm_marks_a_listed_template_name_without_duplicating_it() {
        let dir = tempfile::tempdir().unwrap();
        let tj = dir.path().join("translation.json");
        std::fs::write(&tj, MANIFEST).unwrap();
        // A lake/user file that already lists the template entry by its
        // manifest name.
        let fj = dir.path().join("functions.json");
        std::fs::write(
            &fj,
            r#"{"functions": [{"lean_name": "kdf.hkdf_to_slice",
                "rust_name": "spqr::kdf::hkdf_to_slice", "source": "src/kdf.rs",
                "lines": "L14-L18"}]}"#,
        )
        .unwrap();
        let resolved = resolve(None, Some(&fj), Some(&tj), false).unwrap();
        let slices: Vec<&FunctionRecord> = resolved
            .records
            .iter()
            .filter(|r| r.lean_name == "kdf.hkdf_to_slice")
            .collect();
        assert_eq!(slices.len(), 1);
        assert!(!slices[0].is_local_template);
        assert_eq!(slices[0].def_id, Some(43), "annotate still overlays def_id");
        assert!(slices[0].is_template_name);

        // The listed record maps to the namespaced atom (Copilot review on
        // #83): its name is resolved, and the renamed record takes the
        // template matching rules (its range contains the atom's).
        let lean = std::collections::BTreeMap::from([(
            "probe:spqr.kdf.hkdf_to_slice".to_string(),
            lean_atom(),
        )]);
        let (mappings, _) = translate::generate_translations(
            &slice_rust_atoms(),
            &lean,
            &resolved.records,
            resolved.charon_version.as_deref(),
            resolved.crate_name.as_deref(),
        );
        assert_eq!(mappings.len(), 1, "{mappings:?}");
        assert_eq!(mappings[0].to, "probe:spqr.kdf.hkdf_to_slice");
    }
}
