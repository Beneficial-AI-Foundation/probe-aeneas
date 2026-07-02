//! `translate` module: generate translation mappings between Rust and Lean atoms.
//!
//! The matching logic ([`generate_translations`] and the `strategy_*` helpers)
//! is pure and infallible. Only the two I/O loaders ([`load_atoms`],
//! [`load_functions`]) are fallible.
//!
//! ## Error model
//!
//! Fallible functions return [`anyhow::Result<T>`] directly. All failures are
//! IO or JSON parse errors, so no typed variants are needed. Callers in
//! `extract` and `listfuns` use `?` to propagate these into their own
//! `Other(anyhow::Error)` catch-all variants.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Context as _;
use probe::types::{Atom, Mapping};
use regex::Regex;

use crate::enrich;
use crate::types::{FunctionRecord, FunctionsFile, LineRange};

/// Normalize a source path for matching: strip leading package-name component
/// so `"curve25519-dalek/src/foo.rs"` and `"src/foo.rs"` both become `"src/foo.rs"`.
fn normalize_source_path(p: &str) -> &str {
    if let Some(idx) = p.find("/src/") {
        &p[idx + 1..]
    } else {
        p
    }
}

static RE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&'?\w*\s*").expect("valid regex"));
/// Unwrap `{…}` segments in a Rust qualified name, normalizing trait-impl
/// notation. Charon's `{impl Trait}` shorthand reduces to `Trait`. The
/// expanded `{Trait<Params> for Type}` form preserves the implementing type
/// only when it is fully qualified (contains `::`), producing
/// `Trait<Params>::Type`. Bare names (generic parameters like `T`/`T0`,
/// primitives like `u8`) are stripped to avoid mismatches from Charon's
/// type-parameter numbering.
fn unwrap_braces(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut content = String::new();
            let mut depth = 1usize;
            for c in chars.by_ref() {
                match c {
                    '{' => {
                        depth += 1;
                        content.push(c);
                    }
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        content.push(c);
                    }
                    _ => content.push(c),
                }
            }
            let inner = content.strip_prefix("impl ").unwrap_or(&content);
            match inner.find(" for ") {
                Some(idx) => {
                    let trait_part = &inner[..idx];
                    let type_part = inner[idx + 5..].trim();
                    result.push_str(trait_part);
                    if !type_part.is_empty() && type_part.contains("::") {
                        result.push_str("::");
                        result.push_str(type_part);
                    }
                }
                None => result.push_str(inner),
            }
        } else {
            result.push(ch);
        }
    }
    result
}
/// Strip all generic parameters, handling arbitrary nesting depth.
///
/// `From<SpecificServiceId<KIND>>` → `From`
/// `TryFrom<u8, TryFromReprError<u8>>` → `TryFrom`
fn strip_generics(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut depth = 0usize;
    for ch in s.chars() {
        match ch {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => result.push(ch),
            _ => {}
        }
    }
    result
}

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
) -> (Vec<Mapping>, TranslateStats) {
    let mut mappings = Vec::new();
    let mut matched_rust: HashSet<String> = HashSet::new();
    let mut matched_lean: HashSet<String> = HashSet::new();

    // Build indexes from functions.json
    // (source_file, base_method_name) -> [function records]
    let mut file_name_to_funcs: HashMap<(String, String), Vec<&FunctionRecord>> = HashMap::new();
    // source_file -> [function records]
    let mut file_to_funcs: HashMap<String, Vec<&FunctionRecord>> = HashMap::new();

    for func in functions {
        if func.is_hidden || func.is_extraction_artifact {
            continue;
        }
        if let Some(ref src) = func.source {
            let norm_src = normalize_source_path(src).to_string();
            file_to_funcs
                .entry(norm_src.clone())
                .or_default()
                .push(func);
            let base = func.lean_name.rsplit('.').next().unwrap_or("");
            if !base.is_empty() {
                file_name_to_funcs
                    .entry((norm_src, base.to_string()))
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
/// Strips lifetime parameters, reference markers, brace wrappers (including
/// `impl` prefixes and `for Type` suffixes), and generic parameters at any
/// nesting depth.
pub(crate) fn normalize_rust_name(name: &str) -> String {
    let s = RE_REF.replace_all(name, "");
    let s = unwrap_braces(&s);
    let s = strip_generics(&s);
    s.replace(' ', "")
}

fn extract_base_name(display_name: &str) -> &str {
    display_name.rsplit("::").next().unwrap_or(display_name)
}

/// Classify a `functions.json` entry as deferred for RQN matching.
///
/// Hidden entries (macro-generated trait-impl variants, `.mutual`, closures)
/// and extraction artifacts (`_loop`, `.body` defs) share their `rust_name`
/// with the primary definition, so matching them first shadows the real def
/// and binds the Rust atom to a spec-less Lean atom (issue #16, and the SPQR
/// `parallel_mult` → `parallel_mult_loop.body` case).
///
/// Both the `functions.json` flags and the name heuristics are consulted, so
/// files with incomplete flags (issue #2) are still classified correctly.
///
/// Deferral is never exclusion: a deferred entry (including a false positive
/// from the name heuristic, e.g. a real function named `foo_loop`) still
/// binds in the second pass if its Rust atom is otherwise unmatched.
fn is_deferred_entry(func: &FunctionRecord) -> bool {
    func.is_hidden
        || func.is_extraction_artifact
        || enrich::is_extraction_artifact(&func.lean_name)
        || enrich::is_hidden_by_name(&func.lean_name)
}

/// Match Rust atoms to Lean translations via normalized `rust-qualified-name`.
///
/// `normalize_rust_name` preserves the implementing type in `{Trait for Type}`
/// segments only when the type is fully qualified (contains `::`), e.g.
/// `{From for my_crate::MyType}` → `From::my_crate::MyType`. Bare types
/// (generic parameters like `T`/`T0`, primitives like `u8`) are stripped to
/// avoid mismatches from Charon's type-parameter numbering. This means
/// `{impl From}` and `{From for u8}` normalize identically (both → `From`),
/// while `{From for crate::LookupTable}` and `{From for crate::NafTable}`
/// remain distinct.
///
/// Runs in two passes: visible, non-artifact entries first, then deferred
/// entries ([`is_deferred_entry`]) as a fallback for Rust atoms nothing else
/// claimed. This makes the outcome independent of the order Aeneas emits
/// defs in `Funs.lean` when several entries share a normalized `rust_name`.
fn strategy_rust_qualified_name(
    rust_data: &BTreeMap<String, Atom>,
    lean_data: &BTreeMap<String, Atom>,
    functions: &[FunctionRecord],
    mappings: &mut Vec<Mapping>,
    matched_rust: &mut HashSet<String>,
    matched_lean: &mut HashSet<String>,
) {
    let mut rqn_to_rust: HashMap<String, Vec<String>> = HashMap::new();
    for (code_name, atom) in rust_data {
        if let Some(rqn) = atom.extensions.get("rust-qualified-name") {
            if let Some(rqn_str) = rqn.as_str() {
                let norm = normalize_rust_name(rqn_str);
                rqn_to_rust.entry(norm).or_default().push(code_name.clone());
            }
        }
    }

    // Pass 1: visible, non-artifact entries. Pass 2: deferred entries, which
    // only bind Rust atoms still unmatched after pass 1 (`matched_rust`).
    let (preferred, deferred): (Vec<&FunctionRecord>, Vec<&FunctionRecord>) =
        functions.iter().partition(|f| !is_deferred_entry(f));

    for func in preferred.into_iter().chain(deferred) {
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

        let candidates = match rqn_to_rust.get(&norm_rn) {
            Some(c) if !c.is_empty() => c,
            _ => continue,
        };

        let rust_code_name = if candidates.len() == 1 {
            &candidates[0]
        } else {
            match disambiguate_by_file_and_lines(candidates, func, rust_data) {
                Some(name) => name,
                None => continue,
            }
        };

        if lean_data.contains_key(&lean_code_name)
            && !matched_rust.contains(rust_code_name)
            && !matched_lean.contains(&lean_code_name)
        {
            let confidence = if candidates.len() == 1 {
                "exact"
            } else {
                "exact-disambiguated"
            };
            mappings.push(Mapping {
                from: rust_code_name.clone(),
                to: lean_code_name.clone(),
                confidence: confidence.to_string(),
                method: Some("rust-qualified-name".to_string()),
            });
            matched_rust.insert(rust_code_name.clone());
            matched_lean.insert(lean_code_name);
        }
    }
}

/// When multiple Rust atoms share the same normalized RQN, use the
/// functions.json entry's `source` file and `lines` to pick the right one.
fn disambiguate_by_file_and_lines<'a>(
    candidates: &'a [String],
    func: &FunctionRecord,
    rust_data: &'a BTreeMap<String, Atom>,
) -> Option<&'a String> {
    let func_source = func.source.as_deref()?;
    let norm_func_source = normalize_source_path(func_source);
    let func_range = func.lines.as_deref().and_then(|s| LineRange::parse(s).ok());

    let mut file_matches: Vec<&String> = candidates
        .iter()
        .filter(|code_name| {
            rust_data.get(*code_name).is_some_and(|atom| {
                let norm_atom = normalize_source_path(&atom.code_path);
                norm_atom == norm_func_source
            })
        })
        .collect();

    if file_matches.len() == 1 {
        return Some(file_matches[0]);
    }

    if let Some(fr) = func_range {
        if file_matches.is_empty() {
            file_matches = candidates.iter().collect();
        }
        let mut best: Option<&String> = None;
        let mut best_overlap: i64 = i64::MIN;
        for code_name in &file_matches {
            if let Some(atom) = rust_data.get(*code_name) {
                let atom_range = LineRange {
                    start: atom.code_text.lines_start,
                    end: atom.code_text.lines_end,
                };
                if atom_range.start == 0 {
                    continue;
                }
                let overlap = atom_range.overlap_amount(&fr);
                if overlap > best_overlap {
                    best_overlap = overlap;
                    best = Some(code_name);
                }
            }
        }
        if best_overlap > 0 {
            return best;
        }
    }

    None
}

fn strategy_file_display_name(
    rust_data: &BTreeMap<String, Atom>,
    lean_data: &BTreeMap<String, Atom>,
    file_name_to_funcs: &HashMap<(String, String), Vec<&FunctionRecord>>,
    mappings: &mut Vec<Mapping>,
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

        let norm_path = normalize_source_path(&atom.code_path).to_string();
        let key = (norm_path, base_name.to_string());
        let candidates = match file_name_to_funcs.get(&key) {
            Some(c) if c.len() == 1 => c,
            _ => continue,
        };

        let func = candidates[0];
        let lean_code_name = format!("probe:{}", func.lean_name);
        if lean_data.contains_key(&lean_code_name) && !matched_lean.contains(&lean_code_name) {
            mappings.push(Mapping {
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
    mappings: &mut Vec<Mapping>,
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

        let norm_path = normalize_source_path(&atom.code_path);
        let candidates = match file_to_funcs.get(norm_path) {
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

            let func_range = match func.lines.as_deref().and_then(|s| LineRange::parse(s).ok()) {
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
                mappings.push(Mapping {
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
pub fn load_functions(path: &Path) -> anyhow::Result<Vec<FunctionRecord>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let file: FunctionsFile =
        serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?;
    Ok(file.functions)
}

/// Load atom data from a probe envelope JSON file.
pub fn load_atoms(path: &Path) -> anyhow::Result<BTreeMap<String, Atom>> {
    let (data, _provenance) = probe::types::load_atom_file(path).map_err(anyhow::Error::msg)?;
    Ok(data)
}

/// Build a full translations JSON value ready to write to disk.
///
/// Expects single-probe envelopes with a top-level `"source"` key containing
/// package metadata. Does not support merged envelopes, which use `"inputs"`
/// instead of `"source"`.
pub fn build_translations_json(
    mappings: &[Mapping],
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
        "schema": "probe/mappings",
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
            is_hidden: false,
            is_extraction_artifact: false,
        }
    }

    fn make_func_flagged(
        lean_name: &str,
        rust_name: Option<&str>,
        source: &str,
        lines: &str,
        is_hidden: bool,
        is_extraction_artifact: bool,
    ) -> FunctionRecord {
        FunctionRecord {
            is_hidden,
            is_extraction_artifact,
            ..make_func(lean_name, rust_name, source, lines)
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

    // -- Regression-safety tests: pin output for inputs that already work --

    #[test]
    fn test_normalize_simple_brace_unwrap() {
        assert_eq!(
            normalize_rust_name("path::{Type}::method"),
            "path::Type::method"
        );
    }

    #[test]
    fn test_normalize_no_braces_passthrough() {
        assert_eq!(
            normalize_rust_name("my_crate::module::func"),
            "my_crate::module::func"
        );
    }

    #[test]
    fn test_normalize_single_level_generics() {
        assert_eq!(normalize_rust_name("Vec<u8>"), "Vec");
        assert_eq!(normalize_rust_name("HashMap<String, Vec<u8>>"), "HashMap");
    }

    // -- New-behavior tests: the three root causes from issue #8 --

    #[test]
    fn test_normalize_nested_generics() {
        assert_eq!(normalize_rust_name("From<SpecificServiceId<KIND>>"), "From");
        assert_eq!(
            normalize_rust_name("TryFrom<u8, TryFromReprError<u8>>"),
            "TryFrom"
        );
    }

    #[test]
    fn test_normalize_impl_and_bare_for_type_match() {
        // {impl Trait} and {Trait for BareType} both strip the type (bare = no `::`)
        // so they normalize identically.
        let atom = normalize_rust_name("libsignal_core::address::{impl core::convert::From}::from");
        let fj = normalize_rust_name(
            "libsignal_core::address::{core::convert::From<libsignal_core::address::ServiceIdKind> for u8}::from",
        );
        assert_eq!(atom, fj);
    }

    #[test]
    fn test_normalize_impl_stripped() {
        assert_eq!(
            normalize_rust_name("path::{impl core::convert::From}::method"),
            "path::core::convert::From::method"
        );
    }

    #[test]
    fn test_normalize_for_bare_type_stripped() {
        // Bare types (no `::`) are stripped to avoid generic-parameter mismatches.
        assert_eq!(
            normalize_rust_name("path::{core::convert::From<T> for u8}::method"),
            "path::core::convert::From::method"
        );
    }

    #[test]
    fn test_normalize_for_qualified_type_preserved() {
        // Fully qualified types (with `::`) are preserved to distinguish impls.
        assert_eq!(
            normalize_rust_name("path::{core::convert::From<T> for my::module::MyType}::method"),
            "path::core::convert::From::my::module::MyType::method"
        );
    }

    // -- Edge-case tests for unwrap_braces --

    #[test]
    fn test_unwrap_braces_multiple_segments() {
        assert_eq!(unwrap_braces("{impl A}::{B for C}::m"), "A::B::m");
    }

    #[test]
    fn test_unwrap_braces_no_braces() {
        assert_eq!(unwrap_braces("plain::path"), "plain::path");
    }

    #[test]
    fn test_unwrap_braces_impl_without_for() {
        assert_eq!(unwrap_braces("{impl Trait}"), "Trait");
    }

    #[test]
    fn test_unwrap_braces_for_without_impl() {
        assert_eq!(unwrap_braces("{Trait for Type}"), "Trait");
    }

    #[test]
    fn test_unwrap_braces_for_qualified_type_kept() {
        assert_eq!(
            unwrap_braces("{Trait for my::module::Type}"),
            "Trait::my::module::Type"
        );
    }

    #[test]
    fn test_unwrap_braces_for_empty_type() {
        assert_eq!(unwrap_braces("{Trait for }"), "Trait");
    }

    // -- Regression test for issue #9: different From impls must not collide --

    #[test]
    fn test_normalize_different_from_impls_are_distinct() {
        let lookup = normalize_rust_name(
            "curve25519_dalek::window::{core::convert::From<&'a \
             (curve25519_dalek::edwards::EdwardsPoint)> for \
             curve25519_dalek::window::LookupTable<curve25519_dalek::backend::\
             serial::u64::field::FieldElement51>}::from",
        );
        let naf = normalize_rust_name(
            "curve25519_dalek::window::{core::convert::From<&'0 \
             (curve25519_dalek::edwards::EdwardsPoint)> for \
             curve25519_dalek::window::NafLookupTable5}::from",
        );
        assert_ne!(lookup, naf, "different From impls must not collide");
        assert!(lookup.contains("LookupTable"));
        assert!(naf.contains("NafLookupTable5"));
    }

    #[test]
    fn test_line_range_parse() {
        let r = LineRange::parse("L292-L325").unwrap();
        assert_eq!(r.start, 292);
        assert_eq!(r.end, 325);
    }

    #[test]
    fn test_line_range_parse_invalid() {
        assert!(LineRange::parse("292-325").is_err());
        assert!(LineRange::parse("").is_err());
        assert!(LineRange::parse("L325-L292").is_err()); // start > end
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

    /// SPQR `parallel_mult` shape: Aeneas emits the loop-body and loop defs
    /// BEFORE the primary def, all sharing the same `rust_name`. The artifact
    /// entries must not shadow the primary Lean def.
    #[test]
    fn artifact_entries_do_not_shadow_primary() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("parallel_mult", "src/encoding/gf.rs", 201, 214);
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("spqr::encoding::gf::parallel_mult"),
        );
        rust_atoms.insert("probe:spqr/1.5.0/parallel_mult()".to_string(), atom);

        let mut lean = BTreeMap::new();
        for name in [
            "spqr.encoding.gf.parallel_mult_loop.body",
            "spqr.encoding.gf.parallel_mult_loop",
            "spqr.encoding.gf.parallel_mult",
        ] {
            lean.insert(
                format!("probe:{name}"),
                make_lean_atom(name.rsplit('.').next().unwrap(), "Funs.lean"),
            );
        }

        // Emission order as in real Aeneas output: body, loop, then primary.
        let funcs = vec![
            make_func_flagged(
                "spqr.encoding.gf.parallel_mult_loop.body",
                Some("spqr::encoding::gf::parallel_mult"),
                "src/encoding/gf.rs",
                "L205-L210",
                false,
                true,
            ),
            make_func_flagged(
                "spqr.encoding.gf.parallel_mult_loop",
                Some("spqr::encoding::gf::parallel_mult"),
                "src/encoding/gf.rs",
                "L205-L210",
                false,
                true,
            ),
            make_func(
                "spqr.encoding.gf.parallel_mult",
                Some("spqr::encoding::gf::parallel_mult"),
                "src/encoding/gf.rs",
                "L201-L214",
            ),
        ];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);

        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].to, "probe:spqr.encoding.gf.parallel_mult",
            "primary def must win over loop artifacts regardless of emission order"
        );
        assert_eq!(mappings[0].confidence, "exact");
    }

    /// Issue #16 shape: a hidden macro-generated owned-variant entry precedes
    /// the visible `&T` entry; both collapse to the same normalized rust_name.
    /// The visible entry must win.
    #[test]
    fn hidden_entry_does_not_shadow_visible() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom(
            "MontgomeryPoint::mul_assign",
            "curve25519-dalek/src/montgomery.rs",
            455,
            457,
        );
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!(
                "curve25519_dalek::montgomery::{core::ops::arith::MulAssign<&'0 (curve25519_dalek::scalar::Scalar)> for curve25519_dalek::montgomery::MontgomeryPoint}::mul_assign"
            ),
        );
        rust_atoms.insert("probe:dalek/4.2.0/mul_assign()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:curve25519_dalek.montgomery.MontgomeryPoint.mul_assign_owned".to_string(),
            make_lean_atom("mul_assign_owned", "Funs.lean"),
        );
        lean.insert(
            "probe:curve25519_dalek.montgomery.MontgomeryPoint.mul_assign_shared".to_string(),
            make_lean_atom("mul_assign_shared", "Funs.lean"),
        );

        // Hidden owned variant first (macros.rs), visible &Scalar variant second.
        // Both rust_names normalize to the same key because RE_REF strips `&'0`.
        let funcs = vec![
            make_func_flagged(
                "curve25519_dalek.montgomery.MontgomeryPoint.mul_assign_owned",
                Some("curve25519_dalek::montgomery::{core::ops::arith::MulAssign<curve25519_dalek::scalar::Scalar> for curve25519_dalek::montgomery::MontgomeryPoint}::mul_assign"),
                "curve25519-dalek/src/macros.rs",
                "L118-L120",
                true,
                false,
            ),
            make_func(
                "curve25519_dalek.montgomery.MontgomeryPoint.mul_assign_shared",
                Some("curve25519_dalek::montgomery::{core::ops::arith::MulAssign<&'0 (curve25519_dalek::scalar::Scalar)> for curve25519_dalek::montgomery::MontgomeryPoint}::mul_assign"),
                "curve25519-dalek/src/montgomery.rs",
                "L455-L457",
            ),
        ];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);

        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].to, "probe:curve25519_dalek.montgomery.MontgomeryPoint.mul_assign_shared",
            "visible &T entry must win over the hidden macro-generated owned variant"
        );
    }

    /// Deferral is never exclusion: a Rust atom whose ONLY functions.json
    /// entry is hidden must still get a mapping via the pass-2 fallback.
    #[test]
    fn hidden_only_entry_still_binds() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("clone", "crate/src/lib.rs", 10, 12);
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("my_crate::lib::clone"),
        );
        rust_atoms.insert("probe:crate/1.0/clone()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:my_crate.lib.clone".to_string(),
            make_lean_atom("clone", "Funs.lean"),
        );

        let funcs = vec![make_func_flagged(
            "my_crate.lib.clone",
            Some("my_crate::lib::clone"),
            "crate/src/lib.rs",
            "L10-L12",
            true,
            false,
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs);

        assert_eq!(
            mappings.len(),
            1,
            "hidden-only entry must still bind in pass 2 (no regression vs pre-fix behavior)"
        );
        assert_eq!(mappings[0].to, "probe:my_crate.lib.clone");
        assert_eq!(mappings[0].from, "probe:crate/1.0/clone()");
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
