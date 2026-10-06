//! Integration tests that validate probe-aeneas extract output.
//!
//! The real aeneas example uses MergedEnvelope (with `inputs` instead of `source`),
//! so we validate it using serde_json::Value for basic structural checks.

use std::path::Path;

/// Validate the real merged aeneas example has correct top-level structure.
#[test]
fn example_merged_json_has_valid_structure() {
    let content = std::fs::read_to_string("examples/aeneas_curve25519-dalek_4.1.3.json").unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    // Merged envelope fields.
    assert_eq!(json["schema"], "probe-aeneas/extract");
    assert_eq!(json["schema-version"], "3.0");
    assert!(json["tool"]["name"].is_string());
    assert!(
        json["inputs"].is_array(),
        "merged envelope should have 'inputs' array"
    );
    assert!(json["timestamp"].is_string());
    assert!(json["data"].is_object());
}

/// Validate that all atoms in the merged example have required fields.
#[test]
fn example_merged_json_atoms_have_required_fields() {
    let content = std::fs::read_to_string("examples/aeneas_curve25519-dalek_4.1.3.json").unwrap();
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
        assert!(atom["kind"].is_string(), "atom {key} missing kind");
        assert!(atom["language"].is_string(), "atom {key} missing language");
    }
}

/// Validate that Rust atoms in the merged example have translation metadata.
#[test]
fn example_merged_json_rust_atoms_have_translations() {
    let content = std::fs::read_to_string("examples/aeneas_curve25519-dalek_4.1.3.json").unwrap();
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();
    let data = json["data"].as_object().unwrap();

    let rust_atoms: Vec<_> = data
        .iter()
        .filter(|(_, v)| v["language"] == "rust")
        .collect();

    assert!(
        !rust_atoms.is_empty(),
        "expected Rust atoms in merged output"
    );

    // All Rust atoms should have the untracked scope field.
    for (key, atom) in &rust_atoms {
        assert!(
            atom.get("untracked").is_some(),
            "Rust atom {key} missing 'untracked' field"
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

/// Run the merge pipeline via the library API using pre-generated example JSON files.
///
/// No external tools needed — uses pre-computed Rust atoms, Lean atoms, and functions.json.
#[test]
fn library_extract_with_pregenerated_json() {
    let dir = tempfile::tempdir().unwrap();
    let output_path = dir.path().join("merged.json");

    let rust_json = Path::new("examples/rust_curve25519-dalek_4.1.3.json");
    let lean_example = Path::new("examples/lean_Curve25519Dalek_0.1.0.json");
    let functions_json = Path::new("examples/functions.json");

    assert!(rust_json.exists(), "rust example JSON not found");
    assert!(lean_example.exists(), "lean example JSON not found");

    // Stand-in until the examples are re-extracted with probe-lean 0.16.0
    // (#71): the checked-in Lean example comes from probe-lean 0.4.5, which
    // the hub's ADR-006 version gate rejects, so run on a copy restamped as
    // a contract release. The example carries no `kernel-taint` markers, so
    // this exercises the pipeline shape, not the Lean evidence contract.
    let mut lean_envelope: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(lean_example).unwrap()).unwrap();
    lean_envelope["tool"]["version"] = serde_json::json!("0.16.0");
    let lean_path = dir.path().join("lean.json");
    std::fs::write(&lean_path, lean_envelope.to_string()).unwrap();
    let lean_json = lean_path.as_path();
    assert!(functions_json.exists(), "functions.json not found");

    probe_aeneas::extract::run_extract(
        Some(rust_json),
        None,
        Some(lean_json),
        None,
        Some(functions_json),
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
    let json: serde_json::Value = serde_json::from_str(&content).unwrap();

    assert_eq!(json["schema"], "probe-aeneas/extract");
    assert_eq!(json["schema-version"], "3.0");
    assert!(json["data"].is_object());

    let data = json["data"].as_object().unwrap();
    assert!(
        !data.is_empty(),
        "expected non-empty data from library extract"
    );

    // Verify the merge produced atoms from both languages.
    let has_rust = data.values().any(|v| v["language"] == "rust");
    let has_lean = data.values().any(|v| v["language"] == "lean");
    assert!(has_rust, "expected Rust atoms in merged output");
    assert!(has_lean, "expected Lean atoms in merged output");

    // Verify translation metadata on Rust atoms.
    let rust_with_translation = data
        .values()
        .filter(|v| v["language"] == "rust" && v.get("translation-name").is_some())
        .count();
    assert!(
        rust_with_translation > 0,
        "expected some Rust atoms with translation-name"
    );

    // ADR-006: mappings become correspondence records, and every status
    // copied onto a Rust atom is marked as imported evidence.
    let rust_with_record = data
        .values()
        .filter(|v| v["language"] == "rust" && v["maps-to"].is_array())
        .count();
    assert_eq!(rust_with_record, rust_with_translation);
    // Confidence and method come from the generated records, not from a
    // reconstructed default: the example yields non-exact matches too.
    let records: Vec<&serde_json::Value> = data
        .values()
        .filter_map(|v| v["maps-to"].as_array())
        .flatten()
        .collect();
    assert!(records.iter().all(|r| r["method"].is_string()));
    assert!(records
        .iter()
        .any(|r| r["confidence"] == "exact-disambiguated"));
    assert!(data
        .values()
        .any(|v| v["language"] == "lean" && v["mapped-from"].is_array()));
    for (key, atom) in data.iter().filter(|(_, v)| v["language"] == "rust") {
        if atom.get("verification-status").is_some() {
            assert_eq!(atom["status-origin"], "translation", "{key}");
            assert_ne!(
                atom["verification-status"], "transitively-verified",
                "{key}"
            );
        }
    }
}
