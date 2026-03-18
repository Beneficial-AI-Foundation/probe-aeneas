//! Integration tests that validate probe-aeneas extract output using probe-extract-check.
//!
//! The real aeneas example uses MergedEnvelope (with `inputs` instead of `source`),
//! so we validate it using serde_json::Value for basic structural checks.
//! For full AtomEnvelope-based checks, we use the probe-extract-check micro fixtures.

use std::path::Path;

/// Validate the real merged aeneas example has correct top-level structure.
#[test]
fn example_merged_json_has_valid_structure() {
    let content =
        std::fs::read_to_string("examples/aeneas_curve25519-dalek_4.1.3.json").unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    // Merged envelope fields.
    assert_eq!(json["schema"], "probe-aeneas/extract");
    assert_eq!(json["schema-version"], "2.0");
    assert!(json["tool"]["name"].is_string());
    assert!(json["inputs"].is_array(), "merged envelope should have 'inputs' array");
    assert!(json["timestamp"].is_string());
    assert!(json["data"].is_object());
}

/// Validate that all atoms in the merged example have required fields.
#[test]
fn example_merged_json_atoms_have_required_fields() {
    let content =
        std::fs::read_to_string("examples/aeneas_curve25519-dalek_4.1.3.json").unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    let data = json["data"].as_object().unwrap();

    assert!(
        !data.is_empty(),
        "expected non-empty data in merged example"
    );

    for (key, atom) in data {
        assert!(
            key.starts_with("probe:"),
            "atom key {key} missing 'probe:' prefix"
        );
        assert!(
            atom["display-name"].is_string(),
            "atom {key} missing display-name"
        );
        assert!(
            atom["kind"].is_string(),
            "atom {key} missing kind"
        );
        assert!(
            atom["language"].is_string(),
            "atom {key} missing language"
        );
    }
}

/// Validate that Rust atoms in the merged example have translation metadata.
#[test]
fn example_merged_json_rust_atoms_have_translations() {
    let content =
        std::fs::read_to_string("examples/aeneas_curve25519-dalek_4.1.3.json").unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    let data = json["data"].as_object().unwrap();

    let rust_atoms: Vec<_> = data
        .iter()
        .filter(|(_, v)| v["language"] == "rust")
        .collect();

    assert!(!rust_atoms.is_empty(), "expected Rust atoms in merged output");

    // All Rust atoms should have is-disabled field.
    for (key, atom) in &rust_atoms {
        assert!(
            atom.get("is-disabled").is_some(),
            "Rust atom {key} missing 'is-disabled' field"
        );
    }

    // At least some Rust atoms should have translation-name.
    let with_translation: Vec<_> = rust_atoms
        .iter()
        .filter(|(_, v)| v.get("translation-name").is_some())
        .collect();
    assert!(
        !with_translation.is_empty(),
        "expected at least some Rust atoms with translation-name"
    );
}

/// Validate the aeneas_micro fixture from probe-extract-check parses as AtomEnvelope.
#[test]
fn micro_fixture_structural_check() {
    let fixture_json =
        Path::new("../probe/probe-extract-check/tests/fixtures/aeneas_micro/expected.json");
    if !fixture_json.exists() {
        // Skip if fixture not available (different checkout).
        eprintln!("skipping: aeneas_micro fixture not found");
        return;
    }

    let envelope = probe_extract_check::load_extract_json(fixture_json).unwrap();
    let report = probe_extract_check::check_all(&envelope, None);

    for d in report.errors() {
        eprintln!("{d}");
    }
    assert!(
        report.is_ok(),
        "structural check found {} error(s)",
        report.error_count()
    );
}

/// Run `probe-aeneas extract` on test projects and validate the output.
///
/// Requires `probe-aeneas`, `probe-rust`, and `probe-lean` to be installed on PATH.
#[test]
#[ignore]
fn live_extract_structural_check() {
    use std::process::Command;

    let dir = tempfile::tempdir().unwrap();
    let output_path = dir.path().join("merged.json");

    let rust_fixture =
        Path::new("../probe/probe-extract-check/tests/fixtures/aeneas_micro/rust_src");
    let lean_fixture =
        Path::new("../probe/probe-extract-check/tests/fixtures/aeneas_micro/lean_src");

    if !rust_fixture.exists() || !lean_fixture.exists() {
        panic!("aeneas_micro fixture not found");
    }

    let status = Command::new("probe-aeneas")
        .args(["extract", "--rust-project"])
        .arg(rust_fixture)
        .arg("--lean-project")
        .arg(lean_fixture)
        .arg("--output")
        .arg(&output_path)
        .status()
        .expect("failed to run probe-aeneas");
    assert!(status.success(), "probe-aeneas extract failed");

    // The merged output uses MergedEnvelope, so do basic JSON validation.
    let content = std::fs::read_to_string(&output_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(json["data"].is_object());
    assert!(
        !json["data"].as_object().unwrap().is_empty(),
        "expected non-empty data from live extract"
    );
}
