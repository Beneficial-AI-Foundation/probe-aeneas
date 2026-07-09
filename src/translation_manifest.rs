//! `translation_manifest` module: parse Aeneas's `translation.json` and overlay
//! its authoritative loop/primary classification onto [`FunctionRecord`]s.
//!
//! Aeneas emits `translation.json` when the `emit-json` arg is set (see the
//! project's `aeneas-config.yml`). Every generated Lean item is listed with its
//! originating Rust name and source location. Loop-generated helpers additionally
//! carry a `loop` object and a `parent_lean_name`; a whole family (top-level def
//! plus its loop helpers) shares one `def_id`. The top-level def is precisely the
//! entry with **no** `loop` field.
//!
//! probe-aeneas otherwise infers "this is a loop artifact" from name suffixes
//! (`_loop`, `_body`, `.body`, …). That heuristic is fragile in both directions.
//! When `translation.json` is available, [`annotate`] joins it to the loaded
//! `functions.json` records by exact `lean_name` and records the ground truth on
//! [`FunctionRecord::is_loop_artifact`] / `parent_lean_name` / `def_id`, which the
//! matcher then prefers over the heuristic.
//!
//! ## Error model
//!
//! [`load`] returns [`anyhow::Result`]; failures are IO or JSON parse errors,
//! surfaced with `.context(...)` and propagated by callers via `?`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

use crate::types::FunctionRecord;

/// Aeneas `translation.json`. Only the `functions` array is consumed; `types`,
/// `globals`, `trait_decls`, and `trait_impls` never carry loop metadata.
#[derive(Debug, Deserialize)]
pub struct TranslationManifest {
    #[serde(default)]
    pub functions: Vec<TranslationFunc>,
}

/// A single `functions` entry from `translation.json`.
#[derive(Debug, Deserialize)]
pub struct TranslationFunc {
    pub def_id: u64,
    pub lean_name: String,
    /// Present only on loop-generated helpers. `loop` is a Rust keyword, hence
    /// the rename.
    #[serde(rename = "loop", default)]
    pub loop_info: Option<LoopInfo>,
    /// `lean_name` of the enclosing top-level def (loop helpers only).
    #[serde(default)]
    pub parent_lean_name: Option<String>,
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

/// Load Aeneas's `translation.json` from `path` (if any) and overlay its
/// authoritative loop/primary classification onto `functions`. A `None` path or
/// a load error is a no-op (heuristic fallback) and is reported — a bad manifest
/// must never abort the pipeline. Shared by the `extract` and `listfuns` flows.
pub fn apply(functions: &mut [FunctionRecord], path: Option<&Path>) {
    let Some(path) = path else {
        return;
    };
    match load(path) {
        Ok(manifest) => {
            let n = annotate(functions, &manifest);
            println!(
                "  Applied translation.json overlay: {n}/{} entries classified authoritatively",
                functions.len()
            );
        }
        Err(e) => {
            eprintln!("  Warning: could not read {}: {e:#}", path.display());
            eprintln!("  Falling back to name-heuristic classification.");
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
            annotated += 1;
        }
    }
    annotated
}

#[cfg(test)]
mod tests {
    use super::*;

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
        "types": [],
        "globals": []
    }"#;

    fn rec(lean_name: &str) -> FunctionRecord {
        FunctionRecord {
            lean_name: lean_name.to_string(),
            ..Default::default()
        }
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
