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
        let aux_defs = translation_manifest::apply(&mut records, translation_json);
        return Ok(ResolvedRecords {
            records,
            aux_defs,
            source: RecordSource::Override,
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
                });
            }
            Err(e) => {
                eprintln!("  Warning: could not read {}: {e:#}", tj.display());
                eprintln!("  Falling back to legacy docstring scraping.");
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
    })
}
