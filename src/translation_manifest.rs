//! `translation_manifest` module: parse Aeneas's `translation.json` and use it
//! as the authoritative source of [`FunctionRecord`]s.
//!
//! Aeneas emits `translation.json` when the `emit-json` arg is set (see the
//! project's `aeneas-config.yml`). Every generated Lean item is listed with its
//! originating Rust name and source location. Loop-generated helpers additionally
//! carry a `loop` object and a `parent_lean_name`; a whole family (top-level def
//! plus its loop helpers) shares one `def_id`. The top-level def is precisely the
//! entry with **no** `loop` field.
//!
//! Two consumption modes:
//! - [`records_from_manifest`] builds records directly from the manifest — the
//!   authoritative replacement for the docstring scraper (`gen_functions`).
//! - [`annotate`]/[`apply`] overlay the authoritative loop/primary classification
//!   onto records that came from *another* source (a user-supplied
//!   `functions.json`), joining by exact `lean_name`. probe-aeneas otherwise
//!   infers "this is a loop artifact" from name suffixes (`_loop`, `_body`,
//!   `.body`, …), a heuristic that is fragile in both directions; the overlay
//!   records ground truth on [`FunctionRecord::is_loop_artifact`] /
//!   `parent_lean_name` / `def_id`, which the matcher prefers over the heuristic.
//!
//! ## Error model
//!
//! [`load`] returns [`anyhow::Result`]; failures are IO or JSON parse errors,
//! surfaced with `.context(...)` and propagated by callers via `?`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

use crate::enrich;
use crate::types::FunctionRecord;

/// Aeneas `translation.json`. The `functions`, `globals`, and `trait_impls`
/// arrays are consumed as function-record sources ([`records_from_manifest`]);
/// `functions` additionally drives loop/primary classification (only it carries
/// loop metadata). The `types` and `trait_impls` arrays also name the
/// Aeneas-generated auxiliary defs (type stand-ins and trait-instance wrappers)
/// for [`auxiliary_lean_names`](TranslationManifest::auxiliary_lean_names).
/// `trait_decls` is unused.
#[derive(Debug, Deserialize)]
pub struct TranslationManifest {
    /// charon version that produced this manifest (top-level `charon_version`).
    /// Used to provenance-gate the `def_id` join: probe-rust's `charon-def-id`s
    /// are only comparable to these `def_id`s when they come from the same
    /// charon *version* (best-effort provenance, not proof of an identical run;
    /// see docs/charon-def-id-matching-plan.md).
    #[serde(default)]
    pub charon_version: Option<String>,
    #[serde(default)]
    pub functions: Vec<TranslationFunc>,
    #[serde(default)]
    pub globals: Vec<TranslationFunc>,
    #[serde(default)]
    pub types: Vec<TranslationEntry>,
    #[serde(default)]
    pub trait_impls: Vec<TranslationFunc>,
}

/// A `types` entry from `translation.json`. Only `lean_name` is consumed —
/// enough to identify the generated auxiliary type stand-in by name.
#[derive(Debug, Deserialize)]
pub struct TranslationEntry {
    pub lean_name: String,
}

/// The `source` object on a manifest entry: originating Rust file and span.
#[derive(Debug, Deserialize)]
pub struct Source {
    pub file: String,
    #[serde(default)]
    pub begin_line: u32,
    #[serde(default)]
    pub end_line: u32,
}

impl TranslationManifest {
    /// Lean names of Aeneas-generated auxiliary defs that carry `rust-source`
    /// but are scaffolding, not implementations: type stand-ins (`types`) and
    /// trait-instance wrappers (`trait_impls`). Consumers should exclude these
    /// from implementation counts (probe-aeneas#26). Names are in the
    /// dot-separated Lean form (e.g. `crate.Type.Insts.Trait`), matching an
    /// atom key with the `probe:` prefix stripped.
    pub fn auxiliary_lean_names(&self) -> HashSet<String> {
        self.types
            .iter()
            .map(|e| e.lean_name.clone())
            .chain(self.trait_impls.iter().map(|e| e.lean_name.clone()))
            .collect()
    }
}

/// Build [`FunctionRecord`]s directly from the manifest — the authoritative
/// replacement for the docstring scraper. Draws from `functions`, `globals`,
/// and `trait_impls` (single-method trait wrappers are real translation
/// targets, so the wrappers must be records), excluding entries in
/// `*External_Template.lean` — external stand-ins with no local Lean atom, the
/// same files the legacy scraper skips (they lack the Aeneas marker).
///
/// Records are sorted by `lean_name` so a possibly-unstable `translation.json`
/// array order never leaks into probe-aeneas's output (P14). The
/// `is_hidden`/`is_extraction_artifact` name-heuristic flags are computed as the
/// scraper did, for output parity; the authoritative loop/primary dimension
/// rides on `is_loop_artifact`/`parent_lean_name`/`def_id`.
pub fn records_from_manifest(manifest: &TranslationManifest) -> Vec<FunctionRecord> {
    // Only `functions` entries carry a charon `FunDeclId` in `def_id`; `globals`
    // and `trait_impls` are numbered in charon's separate `GlobalDeclId`/
    // `TraitImplId` id spaces, so their `def_id`s must not feed the integer join
    // (an int collision across kinds would be a silent wrong mapping).
    let funcs = manifest
        .functions
        .iter()
        .filter(|f| !f.is_external_template())
        .map(|f| f.to_record(true));
    let others = manifest
        .globals
        .iter()
        .chain(manifest.trait_impls.iter())
        .filter(|f| !f.is_external_template())
        .map(|f| f.to_record(false));
    let mut records: Vec<FunctionRecord> = funcs.chain(others).collect();
    records.sort_by(|a, b| a.lean_name.cmp(&b.lean_name));
    records
}

/// A single `functions`, `globals`, or `trait_impls` entry from
/// `translation.json`. (Extra `trait_impls`-only fields like `impl_trait_*` are
/// present in the JSON but not deserialized.)
#[derive(Debug, Deserialize)]
pub struct TranslationFunc {
    pub def_id: u64,
    pub lean_name: String,
    /// Charon-derived qualified Rust name.
    #[serde(default)]
    pub rust_name: Option<String>,
    /// Originating Rust file + span.
    #[serde(default)]
    pub source: Option<Source>,
    /// Which generated Lean file the def lives in (e.g. `SrcTranslated/Funs.lean`).
    #[serde(default)]
    pub lean_file: Option<String>,
    /// Present only on loop-generated helpers. `loop` is a Rust keyword, hence
    /// the rename.
    #[serde(rename = "loop", default)]
    pub loop_info: Option<LoopInfo>,
    /// `lean_name` of the enclosing top-level def (loop helpers only).
    #[serde(default)]
    pub parent_lean_name: Option<String>,
}

impl TranslationFunc {
    /// Whether this entry lives in an `*External_Template.lean` file — an
    /// external stand-in with no local Lean atom, excluded from records.
    fn is_external_template(&self) -> bool {
        self.lean_file
            .as_deref()
            .is_some_and(|f| f.ends_with("External_Template.lean"))
    }

    /// Convert to a [`FunctionRecord`], carrying authoritative loop/primary
    /// classification directly (no post-hoc overlay needed).
    ///
    /// `is_fun_decl` is `true` only for entries from the manifest's `functions`
    /// array, whose `def_id` is a charon `FunDeclId` (the one comparable to
    /// probe-rust's `charon-def-id`). Globals and trait-impls pass `false`.
    fn to_record(&self, is_fun_decl: bool) -> FunctionRecord {
        FunctionRecord {
            lean_name: self.lean_name.clone(),
            rust_name: self.rust_name.clone(),
            source: self.source.as_ref().map(|s| s.file.clone()),
            // Line numbers are 1-based; a 0 span means the manifest omitted
            // `begin_line`/`end_line`. Omit `lines` rather than emit a sentinel
            // `L0-L0` range.
            lines: self.source.as_ref().and_then(|s| {
                (s.begin_line != 0 || s.end_line != 0)
                    .then(|| format!("L{}-L{}", s.begin_line, s.end_line))
            }),
            is_hidden: enrich::is_hidden_by_name(&self.lean_name),
            is_extraction_artifact: enrich::is_extraction_artifact(&self.lean_name),
            is_loop_artifact: Some(self.loop_info.is_some()),
            parent_lean_name: self.parent_lean_name.clone(),
            def_id: Some(self.def_id),
            def_id_is_fun_decl: is_fun_decl,
        }
    }
}

/// The `loop` object on a loop-helper entry. Only presence (loop vs. not) drives
/// classification today; the fields are kept for future per-loop views.
#[derive(Debug, Deserialize)]
pub struct LoopInfo {
    pub id: u64,
    #[serde(default)]
    pub pos: Vec<u32>,
    pub is_body: bool,
}

/// Minimal view of `aeneas-config.yml` — just the emit-json output directory,
/// so [`resolve_path`] can find `translation.json` without depending on the
/// full config parser in `extract`.
#[derive(Deserialize)]
struct DestConfig {
    aeneas_args: Option<AeneasArgs>,
}

#[derive(Deserialize)]
struct AeneasArgs {
    dest: Option<String>,
}

/// Best-effort path to `translation.json` for a project root. Aeneas writes it
/// to `aeneas_args.dest` (default: the project root), read from
/// `aeneas-config.yml` when present. Returns `Some` only if the file exists, so
/// callers get a uniform "manifest available?" answer regardless of entry point.
pub fn resolve_path(project_root: &Path) -> Option<PathBuf> {
    let dest = std::fs::read_to_string(project_root.join("aeneas-config.yml"))
        .ok()
        .and_then(|c| serde_yaml::from_str::<DestConfig>(&c).ok())
        .and_then(|cfg| cfg.aeneas_args.and_then(|a| a.dest));
    let dir = dest.map_or_else(|| project_root.to_path_buf(), |d| project_root.join(d));
    let path = dir.join("translation.json");
    path.exists().then_some(path)
}

/// What overlaying a `translation.json` yields for downstream consumers:
/// the auxiliary-def Lean names to flag as artifacts, plus the manifest's
/// charon version for provenance-gating the `def_id` join.
#[derive(Debug, Default)]
pub struct ManifestOverlay {
    /// Lean names of auxiliary defs (type stand-ins + trait-instance wrappers).
    pub aux_defs: HashSet<String>,
    /// charon version from the manifest, or `None` when unavailable.
    pub charon_version: Option<String>,
}

/// Load Aeneas's `translation.json` from `path` (if any), overlay its
/// authoritative loop/primary classification onto `functions`, and return the
/// auxiliary-def Lean names (type stand-ins + trait-instance wrappers) plus the
/// manifest's charon version. A `None` path is a silent no-op; a load error is
/// reported and swallowed. Both return an empty [`ManifestOverlay`] and fall
/// back to the name heuristics, never aborting the pipeline. Shared by the
/// `extract` and `listfuns` flows.
pub fn apply(functions: &mut [FunctionRecord], path: Option<&Path>) -> ManifestOverlay {
    let Some(path) = path else {
        return ManifestOverlay::default();
    };
    match load(path) {
        Ok(manifest) => {
            let n = annotate(functions, &manifest);
            println!(
                "  Applied translation.json overlay: {n}/{} entries classified authoritatively",
                functions.len()
            );
            ManifestOverlay {
                aux_defs: manifest.auxiliary_lean_names(),
                charon_version: manifest.charon_version,
            }
        }
        Err(e) => {
            eprintln!("  Warning: could not read {}: {e:#}", path.display());
            eprintln!("  Falling back to name-heuristic classification.");
            ManifestOverlay::default()
        }
    }
}

/// Load and parse `translation.json` from disk.
pub fn load(path: &Path) -> anyhow::Result<TranslationManifest> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let manifest: TranslationManifest =
        serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
    Ok(manifest)
}

/// Overlay the manifest's authoritative loop/primary classification onto
/// `functions`, joined by exact `lean_name`. Returns the number of records
/// annotated. Records with no matching manifest entry are left untouched
/// (their overlay fields stay `None`, so callers fall back to name heuristics).
pub fn annotate(functions: &mut [FunctionRecord], manifest: &TranslationManifest) -> usize {
    let by_lean_name: HashMap<&str, &TranslationFunc> = manifest
        .functions
        .iter()
        .map(|tf| (tf.lean_name.as_str(), tf))
        .collect();

    let mut annotated = 0;
    for rec in functions.iter_mut() {
        if let Some(tf) = by_lean_name.get(rec.lean_name.as_str()) {
            rec.is_loop_artifact = Some(tf.loop_info.is_some());
            rec.parent_lean_name = tf.parent_lean_name.clone();
            rec.def_id = Some(tf.def_id);
            // `by_lean_name` is built only from `manifest.functions`, so every
            // annotated `def_id` here is a charon `FunDeclId`.
            rec.def_id_is_fun_decl = true;
            annotated += 1;
        }
    }
    annotated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_from_manifest_builds_sorts_and_excludes_templates() {
        let json = r#"{
            "functions": [
                {"def_id":1,"lean_name":"c.zeta","rust_name":"c::zeta","lean_file":"SrcTranslated/Funs.lean",
                 "source":{"file":"src/a.rs","begin_line":10,"end_line":20}},
                {"def_id":2,"lean_name":"c.zeta_loop","rust_name":"c::zeta","lean_file":"SrcTranslated/Funs.lean",
                 "source":{"file":"src/a.rs","begin_line":12,"end_line":18},
                 "loop":{"id":0,"pos":[0],"is_body":false},"parent_lean_name":"c.zeta"},
                {"def_id":3,"lean_name":"c.ext_fn","rust_name":"core::ext",
                 "lean_file":"SrcTranslated/FunsExternal_Template.lean",
                 "source":{"file":"/rustc/x.rs","begin_line":1,"end_line":1}}
            ],
            "globals":[
                {"def_id":4,"lean_name":"c.alpha","rust_name":"c::ALPHA","lean_file":"SrcTranslated/Funs.lean",
                 "source":{"file":"src/a.rs","begin_line":1,"end_line":1}}
            ],
            "trait_impls":[
                {"def_id":5,"lean_name":"c.T.Insts.Tr","rust_name":"c::{impl Tr for T}",
                 "lean_file":"SrcTranslated/Funs.lean","source":{"file":"src/a.rs","begin_line":30,"end_line":40}}
            ]
        }"#;
        let m: TranslationManifest = serde_json::from_str(json).unwrap();
        let recs = records_from_manifest(&m);

        // Sorted by lean_name (P14); the External_Template entry is excluded;
        // functions + globals + trait_impls are all included.
        let names: Vec<&str> = recs.iter().map(|r| r.lean_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["c.T.Insts.Tr", "c.alpha", "c.zeta", "c.zeta_loop"]
        );

        // Field mapping + authoritative loop/primary classification.
        let zeta = recs.iter().find(|r| r.lean_name == "c.zeta").unwrap();
        assert_eq!(zeta.rust_name.as_deref(), Some("c::zeta"));
        assert_eq!(zeta.source.as_deref(), Some("src/a.rs"));
        assert_eq!(zeta.lines.as_deref(), Some("L10-L20"));
        assert_eq!(zeta.is_loop_artifact, Some(false));
        assert_eq!(zeta.def_id, Some(1));

        let zloop = recs.iter().find(|r| r.lean_name == "c.zeta_loop").unwrap();
        assert_eq!(zloop.is_loop_artifact, Some(true));
        assert_eq!(zloop.parent_lean_name.as_deref(), Some("c.zeta"));

        // Only `functions`-array entries carry a charon `FunDeclId`; the global
        // (`c.alpha`) and trait-impl (`c.T.Insts.Tr`) records must not, so their
        // `def_id`s never feed the integer join.
        assert!(zeta.def_id_is_fun_decl);
        assert!(zloop.def_id_is_fun_decl);
        let alpha = recs.iter().find(|r| r.lean_name == "c.alpha").unwrap();
        let trait_impl = recs.iter().find(|r| r.lean_name == "c.T.Insts.Tr").unwrap();
        assert!(!alpha.def_id_is_fun_decl, "global def_id is a GlobalDeclId");
        assert!(
            !trait_impl.def_id_is_fun_decl,
            "trait-impl def_id is a TraitImplId"
        );
    }

    #[test]
    fn omits_lines_when_source_span_missing() {
        // `source` present but with no line span -> serde defaults both to 0.
        // A 0 span must yield no `lines`, not a sentinel `L0-L0` range.
        let json = r#"{
            "functions": [
                {"def_id":1,"lean_name":"c.f","rust_name":"c::f","lean_file":"SrcTranslated/Funs.lean",
                 "source":{"file":"src/a.rs"}}
            ]
        }"#;
        let m: TranslationManifest = serde_json::from_str(json).unwrap();
        let recs = records_from_manifest(&m);
        let f = recs.iter().find(|r| r.lean_name == "c.f").unwrap();
        assert_eq!(f.source.as_deref(), Some("src/a.rs"));
        assert_eq!(f.lines, None);
    }

    #[test]
    fn external_template_match_is_suffix_precise() {
        // Only files ending in `External_Template.lean` are excluded; an
        // unrelated path that merely contains the substring is kept.
        let json = r#"{
            "functions": [
                {"def_id":1,"lean_name":"c.kept","rust_name":"c::kept",
                 "lean_file":"External_Template_Notes/Funs.lean",
                 "source":{"file":"src/a.rs","begin_line":1,"end_line":2}},
                {"def_id":2,"lean_name":"c.dropped","rust_name":"c::dropped",
                 "lean_file":"SrcTranslated/FunsExternal_Template.lean",
                 "source":{"file":"/rustc/x.rs","begin_line":1,"end_line":1}}
            ]
        }"#;
        let m: TranslationManifest = serde_json::from_str(json).unwrap();
        let recs = records_from_manifest(&m);
        let names: Vec<&str> = recs.iter().map(|r| r.lean_name.as_str()).collect();
        assert_eq!(names, vec!["c.kept"]);
    }

    const SAMPLE: &str = r#"{
        "aeneas_version": "x",
        "charon_version": "y",
        "crate": "spqr",
        "functions": [
            {
                "def_id": 439,
                "lean_name": "spqr.encoding.gf.GF16.div_impl",
                "rust_name": "spqr::encoding::gf::{spqr::encoding::gf::GF16}::div_impl",
                "source": {"file": "src/encoding/gf.rs", "begin_line": 549, "end_line": 559}
            },
            {
                "def_id": 439,
                "lean_name": "spqr.encoding.gf.GF16.div_impl_loop",
                "rust_name": "spqr::encoding::gf::{spqr::encoding::gf::GF16}::div_impl",
                "source": {"file": "src/encoding/gf.rs", "begin_line": 554, "end_line": 557},
                "loop": {"id": 0, "pos": [0], "is_body": false},
                "parent_lean_name": "spqr.encoding.gf.GF16.div_impl"
            },
            {
                "def_id": 439,
                "lean_name": "spqr.encoding.gf.GF16.div_impl_loop.body",
                "rust_name": "spqr::encoding::gf::{spqr::encoding::gf::GF16}::div_impl",
                "source": {"file": "src/encoding/gf.rs", "begin_line": 554, "end_line": 557},
                "loop": {"id": 0, "pos": [0], "is_body": true},
                "parent_lean_name": "spqr.encoding.gf.GF16.div_impl"
            }
        ],
        "types": [
            {"def_id": 133, "lean_name": "spqr.encoding.gf.GF16"}
        ],
        "trait_impls": [
            {"def_id": 329, "lean_name": "spqr.Array.Insts.CoreFmtDebug"}
        ],
        "globals": []
    }"#;

    fn rec(lean_name: &str) -> FunctionRecord {
        FunctionRecord {
            lean_name: lean_name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn parses_charon_version() {
        let m: TranslationManifest = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(m.charon_version.as_deref(), Some("y"));
        // Absent field deserializes to None (legacy manifests, provenance gate off).
        let bare: TranslationManifest = serde_json::from_str(r#"{"functions": []}"#).unwrap();
        assert_eq!(bare.charon_version, None);
    }

    #[test]
    fn parses_functions_and_loop_field() {
        let m: TranslationManifest = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(m.functions.len(), 3);
        let primary = &m.functions[0];
        assert!(primary.loop_info.is_none());
        assert_eq!(primary.parent_lean_name, None);
        let body = &m.functions[2];
        assert!(body.loop_info.as_ref().unwrap().is_body);
        assert_eq!(
            body.parent_lean_name.as_deref(),
            Some("spqr.encoding.gf.GF16.div_impl")
        );
    }

    #[test]
    fn annotate_sets_authoritative_flags_by_lean_name() {
        let m: TranslationManifest = serde_json::from_str(SAMPLE).unwrap();
        let mut funcs = vec![
            rec("spqr.encoding.gf.GF16.div_impl"),
            rec("spqr.encoding.gf.GF16.div_impl_loop"),
            rec("spqr.encoding.gf.GF16.div_impl_loop.body"),
            rec("spqr.something.not_in_manifest"),
        ];

        let count = annotate(&mut funcs, &m);
        assert_eq!(count, 3, "three of four records join by lean_name");

        assert_eq!(funcs[0].is_loop_artifact, Some(false));
        assert_eq!(funcs[0].def_id, Some(439));
        assert_eq!(funcs[0].parent_lean_name, None);

        assert_eq!(funcs[1].is_loop_artifact, Some(true));
        assert_eq!(funcs[2].is_loop_artifact, Some(true));
        assert_eq!(
            funcs[2].parent_lean_name.as_deref(),
            Some("spqr.encoding.gf.GF16.div_impl")
        );

        // Unmatched record stays on the heuristic-fallback path.
        assert_eq!(funcs[3].is_loop_artifact, None);
        assert_eq!(funcs[3].def_id, None);
    }

    #[test]
    fn auxiliary_lean_names_unions_types_and_trait_impls() {
        let m: TranslationManifest = serde_json::from_str(SAMPLE).unwrap();
        let aux = m.auxiliary_lean_names();
        assert_eq!(aux.len(), 2);
        assert!(aux.contains("spqr.encoding.gf.GF16"));
        assert!(aux.contains("spqr.Array.Insts.CoreFmtDebug"));
        // Function entries are NOT auxiliary defs.
        assert!(!aux.contains("spqr.encoding.gf.GF16.div_impl"));
    }

    #[test]
    fn auxiliary_lean_names_empty_when_arrays_absent() {
        let m: TranslationManifest = serde_json::from_str(r#"{"functions": []}"#).unwrap();
        assert!(m.auxiliary_lean_names().is_empty());
    }

    #[test]
    fn resolve_path_honors_dest_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(
            root.join("aeneas-config.yml"),
            "aeneas_args:\n  dest: \"out-dir\"\n",
        )
        .unwrap();
        // File at root must NOT be picked when dest points elsewhere.
        std::fs::write(root.join("translation.json"), "{}").unwrap();
        // No file at dest yet -> None.
        assert_eq!(resolve_path(root), None);

        let dest = root.join("out-dir");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("translation.json"), "{}").unwrap();
        assert_eq!(resolve_path(root), Some(dest.join("translation.json")));
    }

    #[test]
    fn resolve_path_defaults_to_root_without_config() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(resolve_path(root), None);
        std::fs::write(root.join("translation.json"), "{}").unwrap();
        assert_eq!(resolve_path(root), Some(root.join("translation.json")));
    }
}
