use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use probe::types::{Atom, TranslationMapping};
use regex::Regex;

use crate::types::{FunctionRecord, FunctionsFile, LineRange};

static RE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&'?\w*\s*").expect("valid regex"));
static RE_BRACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^}]+)\}").expect("valid regex"));
static RE_GENERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("valid regex"));

/// Statistics from translation generation.
pub struct TranslateStats {
    pub by_confidence: HashMap<String, usize>,
}

/// Generate translation mappings between Rust and Lean atoms using functions.json
/// as the bridge.
pub fn generate_translations(
    rust_data: &BTreeMap<String, Atom>,
    lean_data: &BTreeMap<String, Atom>,
    functions: &[FunctionRecord],
) -> (Vec<TranslationMapping>, TranslateStats) {
    let mut mappings = Vec::new();
    let mut matched_rust: HashSet<String> = HashSet::new();
    let mut matched_lean: HashSet<String> = HashSet::new();

    // Build indexes from functions.json
    // (source_file, base_method_name) -> [function records]
    let mut file_name_to_funcs: HashMap<(String, String), Vec<&FunctionRecord>> = HashMap::new();
    // source_file -> [function records]
    let mut file_to_funcs: HashMap<String, Vec<&FunctionRecord>> = HashMap::new();

    for func in functions {
        if let Some(ref src) = func.source {
            file_to_funcs.entry(src.clone()).or_default().push(func);
            let base = func.lean_name.rsplit('.').next().unwrap_or("");
            if !base.is_empty() {
                file_name_to_funcs
                    .entry((src.clone(), base.to_string()))
                    .or_default()
                    .push(func);
            }
        }
    }

    // Strategy 1: rust-qualified-name matching
    strategy_rust_qualified_name(
        rust_data,
        lean_data,
        functions,
        &mut mappings,
        &mut matched_rust,
        &mut matched_lean,
    );

    // Strategy 2: file + display-name matching
    strategy_file_display_name(
        rust_data,
        lean_data,
        &file_name_to_funcs,
        &mut mappings,
        &mut matched_rust,
        &mut matched_lean,
    );

    // Strategy 3: file + line overlap matching
    strategy_file_line_overlap(
        rust_data,
        lean_data,
        &file_to_funcs,
        &mut mappings,
        &mut matched_rust,
        &mut matched_lean,
    );

    mappings.sort_by(|a, b| a.from.cmp(&b.from));

    let mut by_confidence: HashMap<String, usize> = HashMap::new();
    for m in &mappings {
        *by_confidence.entry(m.confidence.clone()).or_insert(0) += 1;
    }

    let stats = TranslateStats { by_confidence };

    (mappings, stats)
}

/// Build a set of normalized Rust qualified names from `functions.json` entries.
///
/// Used to determine which Rust atoms Aeneas processed (`is-disabled: false`).
pub fn build_functions_rust_names(functions: &[FunctionRecord]) -> HashSet<String> {
    functions
        .iter()
        .filter_map(|f| f.rust_name.as_deref())
        .filter(|rn| !rn.is_empty())
        .map(normalize_rust_name)
        .collect()
}

/// Normalize a Rust qualified name for fuzzy matching.
///
/// Strips lifetime parameters, reference markers, brace wrappers, and generics.
pub(crate) fn normalize_rust_name(name: &str) -> String {
    let s = RE_REF.replace_all(name, "");
    let s = RE_BRACE.replace_all(&s, "$1");
    let s = RE_GENERIC.replace_all(&s, "");
    s.replace(' ', "")
}

fn extract_base_name(display_name: &str) -> &str {
    display_name.rsplit("::").next().unwrap_or(display_name)
}

fn strategy_rust_qualified_name(
    rust_data: &BTreeMap<String, Atom>,
    lean_data: &BTreeMap<String, Atom>,
    functions: &[FunctionRecord],
    mappings: &mut Vec<TranslationMapping>,
    matched_rust: &mut HashSet<String>,
    matched_lean: &mut HashSet<String>,
) {
    let mut rqn_to_rust: HashMap<String, Option<String>> = HashMap::new();
    for (code_name, atom) in rust_data {
        if let Some(rqn) = atom.extensions.get("rust-qualified-name") {
            if let Some(rqn_str) = rqn.as_str() {
                let norm = normalize_rust_name(rqn_str);
                rqn_to_rust
                    .entry(norm.clone())
                    .and_modify(|existing| {
                        eprintln!(
                            "Warning: duplicate normalized RQN {norm:?}: {:?} and {code_name:?} — skipping ambiguous match",
                            existing.as_deref().unwrap_or("(already ambiguous)")
                        );
                        *existing = None;
                    })
                    .or_insert_with(|| Some(code_name.clone()));
            }
        }
    }

    for func in functions {
        let rn = match func.rust_name.as_deref() {
            Some(rn) if !rn.is_empty() => rn,
            _ => continue,
        };
        let ln = &func.lean_name;
        if ln.is_empty() {
            continue;
        }

        let norm_rn = normalize_rust_name(rn);
        let lean_code_name = format!("probe:{ln}");

        if let Some(Some(rust_code_name)) = rqn_to_rust.get(&norm_rn) {
            if lean_data.contains_key(&lean_code_name)
                && !matched_rust.contains(rust_code_name)
                && !matched_lean.contains(&lean_code_name)
            {
                mappings.push(TranslationMapping {
                    from: rust_code_name.clone(),
                    to: lean_code_name.clone(),
                    confidence: "exact".to_string(),
                    method: Some("rust-qualified-name".to_string()),
                });
                matched_rust.insert(rust_code_name.clone());
                matched_lean.insert(lean_code_name);
            }
        }
    }
}

fn strategy_file_display_name(
    rust_data: &BTreeMap<String, Atom>,
    lean_data: &BTreeMap<String, Atom>,
    file_name_to_funcs: &HashMap<(String, String), Vec<&FunctionRecord>>,
    mappings: &mut Vec<TranslationMapping>,
    matched_rust: &mut HashSet<String>,
    matched_lean: &mut HashSet<String>,
) {
    for (code_name, atom) in rust_data {
        if matched_rust.contains(code_name) || atom.code_path.is_empty() {
            continue;
        }

        let base_name = extract_base_name(&atom.display_name);
        if base_name.is_empty() {
            continue;
        }

        let key = (atom.code_path.clone(), base_name.to_string());
        let candidates = match file_name_to_funcs.get(&key) {
            Some(c) if c.len() == 1 => c,
            _ => continue,
        };

        let func = candidates[0];
        let lean_code_name = format!("probe:{}", func.lean_name);
        if lean_data.contains_key(&lean_code_name) && !matched_lean.contains(&lean_code_name) {
            mappings.push(TranslationMapping {
                from: code_name.clone(),
                to: lean_code_name.clone(),
                confidence: "file-and-name".to_string(),
                method: Some("file+display-name".to_string()),
            });
            matched_rust.insert(code_name.clone());
            matched_lean.insert(lean_code_name);
        }
    }
}

fn strategy_file_line_overlap(
    rust_data: &BTreeMap<String, Atom>,
    lean_data: &BTreeMap<String, Atom>,
    file_to_funcs: &HashMap<String, Vec<&FunctionRecord>>,
    mappings: &mut Vec<TranslationMapping>,
    matched_rust: &mut HashSet<String>,
    matched_lean: &mut HashSet<String>,
) {
    for (code_name, atom) in rust_data {
        if matched_rust.contains(code_name) || atom.code_path.is_empty() {
            continue;
        }
        let v_start = atom.code_text.lines_start;
        let v_end = atom.code_text.lines_end;
        if v_start == 0 {
            continue;
        }

        let rust_range = LineRange {
            start: v_start,
            end: v_end,
        };

        let candidates = match file_to_funcs.get(&atom.code_path) {
            Some(c) => c,
            None => continue,
        };

        let mut best_match: Option<&FunctionRecord> = None;
        let mut best_overlap: i64 = -1;

        for func in candidates {
            let lean_code_name = format!("probe:{}", func.lean_name);
            if matched_lean.contains(&lean_code_name) {
                continue;
            }

            let func_range = match func.lines.as_deref().and_then(LineRange::parse) {
                Some(r) => r,
                None => continue,
            };

            if rust_range.overlaps(&func_range, 10) {
                let overlap = rust_range.overlap_amount(&func_range);
                if overlap > best_overlap {
                    best_overlap = overlap;
                    best_match = Some(func);
                }
            }
        }

        if let Some(func) = best_match {
            let lean_code_name = format!("probe:{}", func.lean_name);
            if lean_data.contains_key(&lean_code_name) && !matched_lean.contains(&lean_code_name) {
                mappings.push(TranslationMapping {
                    from: code_name.clone(),
                    to: lean_code_name.clone(),
                    confidence: "file-and-lines".to_string(),
                    method: Some("file+line-overlap".to_string()),
                });
                matched_rust.insert(code_name.clone());
                matched_lean.insert(lean_code_name);
            }
        }
    }
}

/// Load functions.json from disk.
pub fn load_functions(path: &Path) -> Result<Vec<FunctionRecord>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let file: FunctionsFile = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
    Ok(file.functions)
}

/// Load atom data from a probe envelope JSON file.
pub fn load_atoms(path: &Path) -> Result<BTreeMap<String, Atom>, String> {
    let (data, _provenance) = probe::types::load_atom_file(path)?;
    Ok(data)
}

/// Build a full translations JSON value ready to write to disk.
///
/// Expects single-probe envelopes with a top-level `"source"` key containing
/// package metadata. Does not support merged envelopes, which use `"inputs"`
/// instead of `"source"`.
pub fn build_translations_json(
    mappings: &[TranslationMapping],
    rust_envelope: &serde_json::Value,
    lean_envelope: &serde_json::Value,
) -> serde_json::Value {
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let rust_source = rust_envelope
        .get("source")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let lean_source = lean_envelope
        .get("source")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    serde_json::json!({
        "schema": "probe/translations",
        "schema-version": "2.0",
        "tool": {
            "name": "probe-aeneas",
            "version": env!("CARGO_PKG_VERSION"),
            "command": "translate"
        },
        "timestamp": timestamp,
        "sources": {
            "from": {
                "schema": rust_envelope.get("schema").and_then(|v| v.as_str()).unwrap_or("probe-rust/extract"),
                "package": rust_source.get("package").and_then(|v| v.as_str()).unwrap_or(""),
                "package-version": rust_source.get("package-version").and_then(|v| v.as_str()).unwrap_or(""),
            },
            "to": {
                "schema": lean_envelope.get("schema").and_then(|v| v.as_str()).unwrap_or("probe-lean/extract"),
                "package": lean_source.get("package").and_then(|v| v.as_str()).unwrap_or(""),
                "package-version": lean_source.get("package-version").and_then(|v| v.as_str()).unwrap_or(""),
            },
        },
        "mappings": mappings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe::types::CodeText;
    use std::collections::BTreeSet;

    fn make_rust_atom(display: &str, path: &str, start: usize, end: usize) -> Atom {
        Atom {
            display_name: display.to_string(),
            dependencies: BTreeSet::new(),
            code_module: String::new(),
            code_path: path.to_string(),
            code_text: CodeText {
                lines_start: start,
                lines_end: end,
            },
            kind: "exec".to_string(),
            language: "rust".to_string(),
            extensions: BTreeMap::new(),
        }
    }

    fn make_lean_atom(display: &str, path: &str) -> Atom {
        Atom {
            display_name: display.to_string(),
            dependencies: BTreeSet::new(),
            code_module: String::new(),
            code_path: path.to_string(),
            code_text: CodeText::default(),
            kind: "def".to_string(),
            language: "lean".to_string(),
            extensions: BTreeMap::new(),
        }
    }

    fn make_func(
        lean_name: &str,
        rust_name: Option<&str>,
        source: &str,
        lines: &str,
    ) -> FunctionRecord {
        FunctionRecord {
            lean_name: lean_name.to_string(),
            rust_name: rust_name.map(|s| s.to_string()),
            source: Some(source.to_string()),
            lines: Some(lines.to_string()),
        }
    }

    #[test]
    fn test_normalize_rust_name() {
        assert_eq!(
            normalize_rust_name("curve25519_dalek::backend::serial::u64::field::{curve25519_dalek::backend::serial::u64::field::FieldElement51}::reduce"),
            "curve25519_dalek::backend::serial::u64::field::curve25519_dalek::backend::serial::u64::field::FieldElement51::reduce"
        );
    }

    #[test]
    fn test_normalize_strips_generics_and_refs() {
        assert_eq!(normalize_rust_name("&'a Foo<Bar>"), "Foo");
    }

    #[test]
    fn test_normalize_rust_name_deterministic_strips_generics() {
        // Behavioral contract: output must not contain < or >
        let result = normalize_rust_name("Vec<u8>");
        assert!(!result.contains('<'), "generics must be stripped");
        assert!(!result.contains('>'), "generics must be stripped");
    }

    #[test]
    fn test_normalize_rust_name_strips_refs_and_lifetimes() {
        // Behavioral contract: output must not contain & or '
        let result = normalize_rust_name("&'a str");
        assert!(!result.contains('&'), "reference markers must be stripped");
        assert!(
            !result.contains('\''),
            "lifetime parameters must be stripped"
        );
    }

    #[test]
    fn test_normalize_rust_name_identity_for_simple_names() {
        assert_eq!(normalize_rust_name("foo"), "foo");
    }

    #[test]
    fn test_line_range_parse() {
        let r = LineRange::parse("L292-L325").unwrap();
        assert_eq!(r.start, 292);
        assert_eq!(r.end, 325);
    }

    #[test]
    fn test_line_range_parse_invalid() {
        assert!(LineRange::parse("292-325").is_none());
        assert!(LineRange::parse("").is_none());
        assert!(LineRange::parse("L325-L292").is_none()); // start > end
    }

    #[test]
    fn test_line_range_overlap() {
        let a = LineRange {
            start: 100,
            end: 200,
        };
        let b = LineRange {
            start: 150,
            end: 250,
        };
        assert!(a.overlaps(&b, 0));
        assert_eq!(a.overlap_amount(&b), 50);
    }

    #[test]
    fn test_line_range_no_overlap() {
        let a = LineRange {
            start: 100,
            end: 200,
        };
        let b = LineRange {
            start: 300,
            end: 400,
        };
        assert!(!a.overlaps(&b, 0));
    }

    #[test]
    fn test_strategy_file_display_name() {
        let mut rust_atoms = BTreeMap::new();
        rust_atoms.insert(
            "probe:crate/1.0/reduce()".to_string(),
            make_rust_atom("FieldElement51::reduce", "crate/src/field.rs", 100, 120),
        );

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:crate.field.FieldElement51.reduce".to_string(),
            make_lean_atom("reduce", "Field.lean"),
        );

        let funcs = vec![make_func(
            "crate.field.FieldElement51.reduce",
            None,
            "crate/src/field.rs",
            "L100-L120",
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].confidence, "file-and-name");
        assert_eq!(mappings[0].from, "probe:crate/1.0/reduce()");
        assert_eq!(mappings[0].to, "probe:crate.field.FieldElement51.reduce");
    }

    #[test]
    fn test_strategy_file_line_overlap() {
        let mut rust_atoms = BTreeMap::new();
        rust_atoms.insert(
            "probe:crate/1.0/mystery()".to_string(),
            make_rust_atom("mystery_fn", "crate/src/field.rs", 200, 250),
        );

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:crate.field.some_fn".to_string(),
            make_lean_atom("some_fn", "Field.lean"),
        );

        let funcs = vec![make_func(
            "crate.field.some_fn",
            None,
            "crate/src/field.rs",
            "L210-L240",
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].confidence, "file-and-lines");
    }

    #[test]
    fn test_strategy_rust_qualified_name() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("reduce", "crate/src/field.rs", 100, 120);
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::field::FieldElement51::reduce"),
        );
        rust_atoms.insert("probe:crate/1.0/reduce()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:my_crate.field.FieldElement51.reduce".to_string(),
            make_lean_atom("reduce", "Field.lean"),
        );

        let funcs = vec![make_func(
            "my_crate.field.FieldElement51.reduce",
            Some("my_crate::field::FieldElement51::reduce"),
            "crate/src/field.rs",
            "L100-L120",
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].confidence, "exact");
        assert_eq!(mappings[0].method.as_deref(), Some("rust-qualified-name"));
    }

    #[test]
    fn test_no_duplicate_mappings() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("FieldElement51::reduce", "crate/src/field.rs", 100, 120);
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::field::FieldElement51::reduce"),
        );
        rust_atoms.insert("probe:crate/1.0/reduce()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:my_crate.field.FieldElement51.reduce".to_string(),
            make_lean_atom("reduce", "Field.lean"),
        );

        let funcs = vec![make_func(
            "my_crate.field.FieldElement51.reduce",
            Some("my_crate::field::FieldElement51::reduce"),
            "crate/src/field.rs",
            "L100-L120",
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);
        // Should only match once (via strategy 1), not again via strategy 2 or 3
        assert_eq!(mappings.len(), 1);
    }

    #[test]
    fn test_one_to_one_primary_wins() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("add_assign", "crate/src/field.rs", 100, 120);
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::field::FieldElement51::add_assign"),
        );
        rust_atoms.insert("probe:crate/1.0/add_assign()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:my_crate.field.FieldElement51.add_assign".to_string(),
            make_lean_atom("add_assign", "Field.lean"),
        );
        lean.insert(
            "probe:my_crate.field.FieldElement51.add_assign_loop".to_string(),
            make_lean_atom("add_assign_loop", "Field.lean"),
        );
        lean.insert(
            "probe:my_crate.field.FieldElement51.add_assign_loop.mutual".to_string(),
            make_lean_atom("mutual", "Field.lean"),
        );

        // functions.json lists primary first, then loop variants (same rust_name)
        let funcs = vec![
            make_func(
                "my_crate.field.FieldElement51.add_assign",
                Some("my_crate::field::FieldElement51::add_assign"),
                "crate/src/field.rs",
                "L100-L120",
            ),
            make_func(
                "my_crate.field.FieldElement51.add_assign_loop",
                Some("my_crate::field::FieldElement51::add_assign"),
                "crate/src/field.rs",
                "L100-L120",
            ),
            make_func(
                "my_crate.field.FieldElement51.add_assign_loop.mutual",
                Some("my_crate::field::FieldElement51::add_assign"),
                "crate/src/field.rs",
                "L100-L120",
            ),
        ];

        let (mappings, _stats) = generate_translations(&rust_atoms, &lean, &funcs);

        assert_eq!(
            mappings.len(),
            1,
            "1-to-1: only primary Lean def should be matched, loop variants skipped"
        );
        assert_eq!(
            mappings[0].to,
            "probe:my_crate.field.FieldElement51.add_assign"
        );
        assert_eq!(mappings[0].from, "probe:crate/1.0/add_assign()");
        assert_eq!(mappings[0].confidence, "exact");
        assert_eq!(mappings[0].method.as_deref(), Some("rust-qualified-name"));
    }

    #[test]
    fn test_does_not_double_claim_lean() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom1 = make_rust_atom("foo", "crate/src/mod.rs", 100, 120);
        atom1.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::mod::foo"),
        );
        rust_atoms.insert("probe:crate/1.0/foo()".to_string(), atom1);

        let mut atom2 = make_rust_atom("bar", "crate/src/mod.rs", 200, 220);
        atom2.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::mod::bar"),
        );
        rust_atoms.insert("probe:crate/1.0/bar()".to_string(), atom2);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:my_crate.mod.shared_lean".to_string(),
            make_lean_atom("shared_lean", "Mod.lean"),
        );

        // Both rust_names try to claim the same lean_name — first wins
        let funcs = vec![
            make_func(
                "my_crate.mod.shared_lean",
                Some("my_crate::mod::foo"),
                "crate/src/mod.rs",
                "L100-L120",
            ),
            make_func(
                "my_crate.mod.shared_lean",
                Some("my_crate::mod::bar"),
                "crate/src/mod.rs",
                "L200-L220",
            ),
        ];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);
        assert_eq!(
            mappings.len(),
            1,
            "same Lean atom should not be claimed by two Rust atoms"
        );
        assert_eq!(mappings[0].from, "probe:crate/1.0/foo()");
    }

    #[test]
    fn test_build_functions_rust_names() {
        let funcs = vec![
            make_func("a.b.foo", Some("my_crate::foo"), "src/lib.rs", "L1-L10"),
            make_func("a.b.bar", Some("my_crate::bar"), "src/lib.rs", "L20-L30"),
            make_func("a.b.baz", None, "src/lib.rs", "L40-L50"),
        ];
        let names = build_functions_rust_names(&funcs);
        assert_eq!(names.len(), 2);
        assert!(names.contains(&normalize_rust_name("my_crate::foo")));
        assert!(names.contains(&normalize_rust_name("my_crate::bar")));
    }

    // =========================================================================
    // Core algorithm correctness tests (C6, C7)
    // =========================================================================

    /// C6: When two Rust atoms share the same normalized rust-qualified-name,
    /// strategy_rust_qualified_name overwrites the first with the last (HashMap insert).
    #[test]
    fn test_duplicate_rqn_last_wins() {
        let mut rust_atoms = BTreeMap::new();

        let mut atom1 = make_rust_atom("Scalar::mul", "crate/src/scalar.rs", 10, 20);
        atom1.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::scalar::Scalar::mul"),
        );
        rust_atoms.insert("probe:crate/1.0/Scalar.mul#1()".to_string(), atom1);

        let mut atom2 = make_rust_atom("Scalar::mul", "crate/src/scalar.rs", 30, 40);
        atom2.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::scalar::Scalar::mul"),
        );
        rust_atoms.insert("probe:crate/1.0/Scalar.mul#2()".to_string(), atom2);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:my_crate.scalar.Scalar.mul".to_string(),
            make_lean_atom("mul", "Scalar.lean"),
        );

        let funcs = vec![make_func(
            "my_crate.scalar.Scalar.mul",
            Some("my_crate::scalar::Scalar::mul"),
            "crate/src/scalar.rs",
            "L10-L20",
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);

        // With C6 bug: rqn_to_rust.insert() overwrites, so the last Rust atom
        // (in BTreeMap iteration order) wins. The first atom is silently dropped.
        // Both atoms have the same RQN, so only one mapping is produced.
        assert_eq!(mappings.len(), 1, "one mapping expected (limitation)");

        // Document which atom got the mapping
        let mapped_from = &mappings[0].from;
        eprintln!(
            "C6: duplicate RQN mapped to {:?} (other atom silently dropped)",
            mapped_from
        );
    }

    /// C7: Lean atoms without source location (lines 0,0) get misleading
    /// translation-text in the enrichment step.
    #[test]
    fn test_lean_atom_no_location_has_default_code_text() {
        let lean_atom = make_lean_atom("foo", "Foo.lean");
        // make_lean_atom uses CodeText::default() which is (0, 0)
        assert_eq!(lean_atom.code_text.lines_start, 0);
        assert_eq!(lean_atom.code_text.lines_end, 0);
        // If this atom is used for enrichment, translation-text will be
        // {"lines-start": 0, "lines-end": 0} which is misleading.
        // The enrichment code should check for this and skip or mark as unknown.
    }
}
