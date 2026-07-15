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

/// Resolve function records in memory, then overlay Aeneas's `translation.json`
/// (authoritative loop/primary classification; a no-op when absent).
///
/// Precedence: explicit `--functions` override, then `lake exe listfuns`, then
/// the legacy docstring scrape. `lean_project` is required unless an override
/// path is given.
pub fn resolve(
    lean_project: Option<&Path>,
    functions_override: Option<&Path>,
    translation_json: Option<&Path>,
    use_lake: bool,
) -> anyhow::Result<ResolvedRecords> {
    let (mut records, source) = if let Some(path) = functions_override {
        (translate::load_functions(path)?, RecordSource::Override)
    } else {
        let lean_project = lean_project.context(
            "function_source::resolve needs a Lean project when no --functions path is given",
        )?;
        if use_lake {
            let path = lean_project.join("functions.json");
            listfuns::run_listfuns(lean_project, &path).map_err(anyhow::Error::new)?;
            (translate::load_functions(&path)?, RecordSource::Lake)
        } else {
            (
                gen_functions::parse_aeneas_project(lean_project).map_err(anyhow::Error::new)?,
                RecordSource::LegacyScrape,
            )
        }
    };

    let aux_defs = translation_manifest::apply(&mut records, translation_json);

    Ok(ResolvedRecords {
        records,
        aux_defs,
        source,
    })
}
