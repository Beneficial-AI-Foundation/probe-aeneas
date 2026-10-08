//! Integration test that runs the extract pipeline on a small synthetic
//! fixture (`tests/fixtures/mini/`) through the library API.
//!
//! The fixture has one Rust function per outcome: one match per name or
//! location strategy, a qualified name shared by three Rust functions, a
//! copied `verified` status, a kernel-tainted spec, a translation without a
//! spec and an untranslated function. Real-data checks run outside the repo
//! on the canonical test projects (docs/testing.md).

use std::path::{Path, PathBuf};

const ADD: &str = "probe:mini/0.1.0/add()";
const SCALE: &str = "probe:mini/0.1.0/point/Point#scale()";
const HELPER: &str = "probe:mini/0.1.0/point/helper()";
const UNTRANSLATED: &str = "probe:mini/0.1.0/untranslated()";
const NEW: &str = "probe:mini/0.1.0/point/Point#new()";
// Share a qualified name with `NEW` and sort before and after it, so taking
// the first or the last candidate picks a wrong atom.
const OTHER_NEW: &str = "probe:mini/0.1.0/other/Point#new()";
const SHADOW_NEW: &str = "probe:mini/0.1.0/shadow/Point#new()";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mini")
        .join(name)
}

/// Run `extract` on the fixture and return the output envelope.
fn extract_mini() -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let output_path = dir.path().join("merged.json");
    probe_aeneas::extract::run_extract(
        Some(&fixture("rust.json")),
        None,
        Some(&fixture("lean.json")),
        None,
        Some(&fixture("functions.json")),
        None,
        Some(&output_path),
        None,
        false,
        None,
        false,
        false,
        None,
    )
    .expect("probe-aeneas extract failed");
    let content = std::fs::read_to_string(&output_path).unwrap();
    serde_json::from_str(&content).unwrap()
}

/// The single `maps-to` record of a Rust atom.
fn record(atom: &serde_json::Value) -> &serde_json::Value {
    let records = atom["maps-to"].as_array().expect("maps-to array");
    assert_eq!(records.len(), 1, "one record per translated function");
    &records[0]
}

#[test]
fn library_extract_on_synthetic_fixture() {
    let json = extract_mini();
    assert_eq!(json["schema"], "probe-aeneas/extract");
    assert_eq!(json["schema-version"], "3.0");
    assert!(json["inputs"].is_array());
    assert_eq!(json["tool"]["name"], "probe-aeneas");
    assert!(!json["tool"]["command"].as_str().unwrap().is_empty());
    assert!(!json["timestamp"].as_str().unwrap().is_empty());
    let data = json["data"].as_object().unwrap();

    // One match per strategy that runs without `translation.json`, with the
    // confidence and method of the generated record.
    let add = &data[ADD];
    assert_eq!(add["translation-name"], "probe:Mini.add");
    assert_eq!(record(add)["confidence"], "exact");
    assert_eq!(record(add)["method"], "rust-qualified-name");
    let scale = &data[SCALE];
    assert_eq!(scale["translation-name"], "probe:Mini.Point.scale");
    assert_eq!(record(scale)["confidence"], "file-and-name");
    assert_eq!(record(scale)["method"], "file+display-name");
    let helper = &data[HELPER];
    assert_eq!(helper["translation-name"], "probe:Mini.helper_inner");
    assert_eq!(record(helper)["confidence"], "file-and-lines");
    assert_eq!(record(helper)["method"], "file+line-overlap");
    assert!(data["probe:Mini.add"]["mapped-from"].is_array());
    assert_eq!(add["translation-path"], "Mini/Funs.lean");
    assert_eq!(add["translation-text"]["lines-start"], 10);
    assert_eq!(add["translation-text"]["lines-end"], 14);

    // Three Rust functions share a qualified name: the `functions.json` source
    // file picks one, and the others stay unmatched.
    let new = &data[NEW];
    assert_eq!(new["translation-name"], "probe:Mini.Point.new");
    assert_eq!(record(new)["confidence"], "exact-disambiguated");
    assert_eq!(record(new)["method"], "rust-qualified-name");
    for key in [OTHER_NEW, SHADOW_NEW] {
        assert!(data[key].get("maps-to").is_none(), "{key}");
        assert!(data[key].get("translation-name").is_none(), "{key}");
    }

    // A verified primary spec gives a copied, marked `verified`.
    assert_eq!(add["verification-status"], "verified");
    assert_eq!(add["status-origin"], "translation");
    // The kernel-taint marker survives on the Lean spec, and the copy of its
    // status is marked as a translation like any other copy.
    let tainted = &data["probe:Mini.Point.scale_correct"];
    assert_eq!(tainted["verification-status"], "verified");
    assert_eq!(tainted["status-origin"], "kernel-taint");
    assert_eq!(scale["verification-status"], "verified");
    assert_eq!(scale["status-origin"], "translation");
    // Enrichment runs: an unmarked Lean spec with clean dependencies is
    // promoted.
    assert_eq!(
        data["probe:Mini.add_spec"]["verification-status"],
        "transitively-verified"
    );

    // A translation without a spec and an untranslated function get no status
    // and stay tracked.
    for key in [HELPER, UNTRANSLATED] {
        let atom = &data[key];
        assert!(atom.get("verification-status").is_none(), "{key}");
        assert!(atom.get("status-origin").is_none(), "{key}");
        assert_eq!(atom["untracked"], false, "{key}");
    }
    assert!(data[UNTRANSLATED].get("translation-name").is_none());
    assert!(data[UNTRANSLATED].get("maps-to").is_none());

    // `is-public-api` from the input passes through.
    assert_eq!(add["is-public-api"], true);
}
