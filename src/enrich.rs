use std::collections::{BTreeMap, HashMap};

use probe::types::Atom;
use serde::Serialize;

use crate::aeneas_config::AeneasConfig;
use crate::types::FunctionRecord;

/// Probe-lean atom key prefix.
pub const PROBE_PREFIX: &str = "probe:";

/// Standard Aeneas extraction artifact suffixes (underscore-joined).
pub const ARTIFACT_SUFFIXES: &[&str] = &["_body", "_loop", "_loop0", "_loop1", "_loop2", "_loop3"];

/// Aeneas artifact suffixes matching dot-separated last component.
pub const ARTIFACT_DOT_SUFFIXES: &[&str] = &[".body"];

// ---------------------------------------------------------------------------
// Pure heuristic functions (shared by `extract` and `listfuns`)
// ---------------------------------------------------------------------------

/// Check if a name is an Aeneas extraction artifact by suffix.
pub fn is_extraction_artifact(name: &str) -> bool {
    ARTIFACT_SUFFIXES.iter().any(|sfx| name.ends_with(sfx))
        || ARTIFACT_DOT_SUFFIXES.iter().any(|sfx| name.ends_with(sfx))
}

/// Trait implementation fragments that represent boilerplate (auto-derived,
/// marker, or trivial). Only `.Insts.` functions matching one of these are
/// hidden by default; meaningful trait impls (arithmetic, crypto, conversion)
/// stay visible.
const BOILERPLATE_INSTS_FRAGMENTS: &[&str] = &[
    "CoreCloneClone",
    "CoreMarkerCopy",
    "CoreMarkerStructuralPartialEq",
    "CoreDefaultDefault",
    "ZeroizeZeroize",
    "ZeroizeDefaultIsZeroes",
];

/// Check if an `.Insts.` function is a boilerplate trait implementation.
///
/// Extracts the trait name (the segment immediately after `.Insts.`) and
/// checks it against [`BOILERPLATE_INSTS_FRAGMENTS`]. Returns `false` for
/// names without `.Insts.` or for meaningful trait impls like arithmetic
/// operators, crypto traits, or conversions.
pub fn is_boilerplate_insts(name: &str) -> bool {
    let Some(insts_pos) = name.find(".Insts.") else {
        return false;
    };
    let after_insts = &name[insts_pos + ".Insts.".len()..];
    let trait_part = after_insts.split('.').next().unwrap_or("");
    BOILERPLATE_INSTS_FRAGMENTS.contains(&trait_part)
}

/// Check if a name is an Aeneas borrow-pattern delegator variant.
///
/// In Aeneas, each operator impl generates multiple borrow variants:
/// - `Shared0<Type>` — primary (canonical reference-taking implementation)
/// - `SharedA<Type>` — delegator (alternative borrow of receiver)
/// - `SharedB<Type>` — delegator (alternative borrow in trait args, owned receiver)
///
/// Only the `Shared0` primary is meaningful; `SharedA`/`SharedB` variants
/// are thin wrappers that delegate to it.
pub fn is_borrow_delegator(name: &str) -> bool {
    if let Some(insts_pos) = name.find(".Insts.") {
        let receiver_part = &name[..insts_pos];
        let receiver_type = receiver_part.rsplit('.').next().unwrap_or("");
        if receiver_type.starts_with("SharedA") || receiver_type.starts_with("SharedB") {
            return true;
        }

        let after_insts = &name[insts_pos + ".Insts.".len()..];
        let trait_part = after_insts.split('.').next().unwrap_or("");
        if trait_part.contains("SharedB") {
            return true;
        }
    }
    false
}

/// Check if a name should be hidden based on naming patterns and attributes.
///
/// Returns `true` for boilerplate trait instance wrappers (Clone, Copy,
/// Default, Zeroize via `.Insts.`), mutual loop defs (`.mutual`), closures
/// (`.closure`), blanket impls (`.Blanket.`), DOC_HIDDEN constants,
/// `rust_trait_impl` attribute, borrow-pattern delegator variants
/// (`SharedA`/`SharedB`), and names in the config's hidden set.
pub fn is_hidden(name: &str, attrs: &[String], config: &AeneasConfig) -> bool {
    let has_trait_attr = attrs.iter().any(|a| a == "rust_trait_impl");
    let is_mutual_loop = name.ends_with(".mutual");
    let has_closure = name.contains(".closure");
    let has_blanket = name.contains(".Blanket.");
    let has_doc_hidden = name.contains("DOC_HIDDEN");
    let in_config = config.hidden.contains(name);
    has_trait_attr
        || is_boilerplate_insts(name)
        || is_borrow_delegator(name)
        || is_mutual_loop
        || has_closure
        || has_blanket
        || has_doc_hidden
        || in_config
}

/// Check if a name should be hidden using only name patterns (no atoms needed).
///
/// Used by `gen_functions` during initial parsing before atoms are available.
pub fn is_hidden_by_name(name: &str) -> bool {
    is_boilerplate_insts(name)
        || is_borrow_delegator(name)
        || name.ends_with(".mutual")
        || name.contains(".closure")
        || name.contains(".Blanket.")
        || name.contains("DOC_HIDDEN")
}

/// Determine if a function is relevant (from the target crate, not stdlib/deps).
pub fn is_relevant(rust_source: &str, rust_crate_name: &str) -> bool {
    if rust_source.is_empty() || rust_source == "null" {
        return true;
    }
    if rust_crate_name.is_empty() {
        return true;
    }
    rust_source.contains(rust_crate_name)
        && !rust_source.starts_with('/')
        && !rust_source.contains("/cargo/registry/")
}

/// Check if attributes include `externally_verified`.
pub fn is_externally_verified(attrs: &[String]) -> bool {
    attrs.iter().any(|a| a == "externally_verified")
}

/// Check if a Lean atom represents a structure definition.
pub fn is_structure(kind: &str) -> bool {
    kind == "structure"
}

/// Check if a Lean atom is a type alias (reducible def).
pub fn is_type_alias(kind: &str, attrs: &[String]) -> bool {
    kind == "def" && attrs.iter().any(|a| a == "reducible")
}

// ---------------------------------------------------------------------------
// Atom helpers
// ---------------------------------------------------------------------------

/// Extract the list of attribute strings from an atom's extensions.
pub fn atom_attrs(atom: &Atom) -> Vec<String> {
    atom.extensions
        .get("attributes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Get the `rust-source` extension from an atom.
pub fn atom_rust_source(atom: &Atom) -> &str {
    atom.extensions
        .get("rust-source")
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Get the `kind` extension from an atom (e.g. "def", "theorem", "structure").
pub fn atom_kind(atom: &Atom) -> &str {
    atom.extensions
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Get atom dependencies as a list of key strings.
pub fn atom_dependencies(atom: &Atom) -> Vec<String> {
    atom.extensions
        .get("dependencies")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Strip the `probe:` prefix from an atom key.
pub fn strip_prefix(key: &str) -> &str {
    key.strip_prefix(PROBE_PREFIX).unwrap_or(key)
}

// ---------------------------------------------------------------------------
// Enriched output types (for `listfuns` enriched JSON)
// ---------------------------------------------------------------------------

/// Fully enriched function record for JSON output.
#[derive(Debug, Clone, Serialize)]
pub struct EnrichedFunctionOutput {
    pub lean_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<String>,
    pub dependencies: Vec<String>,
    pub nested_children: Vec<String>,
    pub is_relevant: bool,
    pub is_extraction_artifact: bool,
    pub is_hidden: bool,
    pub is_ignored: bool,
    pub specified: bool,
    pub verified: bool,
    pub fully_verified: bool,
    pub externally_verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_docstring: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_statement: Option<String>,
}

/// Top-level JSON envelope for enriched functions output.
#[derive(Serialize)]
pub struct EnrichedFunctionsFile {
    pub functions: Vec<EnrichedFunctionOutput>,
}

// ---------------------------------------------------------------------------
// Enrichment pipeline (takes basic FunctionRecords + atoms → enriched output)
// ---------------------------------------------------------------------------

/// Enrich a list of basic function records using probe-lean atom data.
///
/// For each function, looks up the corresponding atom by `probe:<lean_name>`,
/// finds its primary spec theorem, and extracts verification status,
/// dependencies, and other metadata.
pub fn enrich_function_records(
    records: &[FunctionRecord],
    atoms: &BTreeMap<String, Atom>,
    rust_crate_name: &str,
    config: &AeneasConfig,
) -> Vec<EnrichedFunctionOutput> {
    let mut fv_cache: HashMap<String, bool> = HashMap::new();
    let mut results = Vec::with_capacity(records.len());

    for rec in records {
        let key = format!("{PROBE_PREFIX}{}", rec.lean_name);
        let atom = atoms.get(&key);

        let (attrs, deps_raw, rust_source, kind) = match atom {
            Some(a) => (
                atom_attrs(a),
                atom_dependencies(a),
                atom_rust_source(a).to_string(),
                atom_kind(a).to_string(),
            ),
            None => (vec![], vec![], String::new(), String::new()),
        };

        let mut func_is_hidden = is_hidden(&rec.lean_name, &attrs, config)
            || is_structure(&kind)
            || is_type_alias(&kind, &attrs)
            || (!rust_source.is_empty() && !is_relevant(&rust_source, rust_crate_name));

        let func_is_artifact = is_extraction_artifact(&rec.lean_name);
        let func_is_relevant = if rust_source.is_empty() {
            atom.map(|a| {
                a.extensions
                    .get("is-in-package")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
            })
            .unwrap_or(false)
        } else {
            is_relevant(&rust_source, rust_crate_name)
        };

        let (primary_spec_key, spec_atom) = find_primary_spec(&rec.lean_name, atoms);
        let specified = primary_spec_key.is_some();

        // Layer B: never hide a function that has a primary spec.
        if func_is_hidden && specified {
            func_is_hidden = false;
        }
        let verified = spec_atom
            .and_then(|a| {
                a.extensions
                    .get("verification-status")
                    .and_then(|v| v.as_str())
            })
            .map(|s| s == "verified")
            .unwrap_or(false);
        let ext_verified = spec_atom
            .map(|a| is_externally_verified(&atom_attrs(a)))
            .unwrap_or(false);

        let spec_file = spec_atom
            .map(|a| a.code_path.clone())
            .filter(|p| !p.is_empty());

        let (spec_docstring, spec_statement) = extract_spec_text(spec_atom);

        let dependencies: Vec<String> = deps_raw
            .iter()
            .map(|d| strip_prefix(d).to_string())
            .collect();

        let fully_verified = compute_fully_verified(&rec.lean_name, atoms, &mut fv_cache);

        results.push(EnrichedFunctionOutput {
            lean_name: rec.lean_name.clone(),
            rust_name: rec.rust_name.clone(),
            source: rec.source.clone(),
            lines: rec.lines.clone(),
            dependencies,
            nested_children: vec![],
            is_relevant: func_is_relevant,
            is_extraction_artifact: func_is_artifact,
            is_hidden: func_is_hidden,
            is_ignored: false,
            specified,
            verified,
            fully_verified,
            externally_verified: ext_verified,
            spec_file,
            spec_docstring,
            spec_statement,
        });
    }

    hide_single_child_parents(&mut results);

    let stats = compute_enrichment_stats(&results);
    print_enrichment_stats(&stats);
    results
}

/// Hide `.Insts.` parent structs that have exactly one nested child method.
///
/// Aeneas generates a parent entry (e.g. `Type.Insts.TraitName`) for each
/// trait implementation alongside the leaf method (e.g. `Type.Insts.TraitName.method`).
/// When there is exactly one child, the parent is a structural container with
/// no independent value — hide it and populate `nested_children` on the parent.
///
/// Parents with a primary spec override (Layer B) are not re-hidden.
fn hide_single_child_parents(results: &mut [EnrichedFunctionOutput]) {
    let names: Vec<String> = results.iter().map(|r| r.lean_name.clone()).collect();
    let name_set: std::collections::HashSet<&str> = names.iter().map(|s| s.as_str()).collect();

    for result in results.iter_mut() {
        if !result.lean_name.contains(".Insts.") {
            continue;
        }

        let prefix = format!("{}.", result.lean_name);
        let children: Vec<&str> = name_set
            .iter()
            .filter(|n| n.starts_with(prefix.as_str()) && !n[prefix.len()..].contains('.'))
            .copied()
            .collect();

        if children.len() == 1 {
            result.nested_children = children.iter().map(|s| s.to_string()).collect();
            if !result.specified {
                result.is_hidden = true;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Spec theorem lookup
// ---------------------------------------------------------------------------

/// Find the primary spec atom for a function.
///
/// Checks the `primary-spec` extension first, then falls back to the
/// `<name>_spec` naming convention.
fn find_primary_spec<'a>(
    lean_name: &str,
    atoms: &'a BTreeMap<String, Atom>,
) -> (Option<String>, Option<&'a Atom>) {
    let key = format!("{PROBE_PREFIX}{lean_name}");

    if let Some(atom) = atoms.get(&key) {
        if let Some(ps) = atom.extensions.get("primary-spec").and_then(|v| v.as_str()) {
            let ps_key = if ps.starts_with(PROBE_PREFIX) {
                ps.to_string()
            } else {
                format!("{PROBE_PREFIX}{ps}")
            };
            if let Some(spec_atom) = atoms.get(&ps_key) {
                return (Some(ps_key), Some(spec_atom));
            }
        }
    }

    let spec_key = format!("{key}_spec");
    if let Some(spec_atom) = atoms.get(&spec_key) {
        return (Some(spec_key.clone()), Some(spec_atom));
    }

    (None, None)
}

/// Extract spec docstring and statement from a spec atom's extensions.
///
/// Currently probe-lean atoms include only line ranges (`code-text.lines-start/end`),
/// not the actual source text. These fields are populated as `None` for now;
/// a follow-up can read from source files using the line range metadata.
fn extract_spec_text(_spec_atom: Option<&Atom>) -> (Option<String>, Option<String>) {
    (None, None)
}

// ---------------------------------------------------------------------------
// fully_verified transitive walk
// ---------------------------------------------------------------------------

/// Compute whether a function is fully verified: itself verified AND all
/// transitive dependencies (within Funs.lean) are also verified.
///
/// Ported from `enrich_functions.py:compute_fully_verified`.
pub fn compute_fully_verified(
    lean_name: &str,
    atoms: &BTreeMap<String, Atom>,
    cache: &mut HashMap<String, bool>,
) -> bool {
    if let Some(&cached) = cache.get(lean_name) {
        return cached;
    }

    cache.insert(lean_name.to_string(), false);

    let key = format!("{PROBE_PREFIX}{lean_name}");
    let Some(atom) = atoms.get(&key) else {
        return false;
    };

    let (primary_key, _) = find_primary_spec(lean_name, atoms);
    if let Some(pk) = &primary_key {
        let spec_atom = atoms.get(pk);
        let proof_verified = spec_atom
            .and_then(|a| {
                a.extensions
                    .get("verification-status")
                    .and_then(|v| v.as_str())
            })
            .map(|s| s == "verified")
            .unwrap_or(false);
        let ev = spec_atom
            .map(|a| is_externally_verified(&atom_attrs(a)))
            .unwrap_or(false);
        if !proof_verified && !ev {
            return false;
        }
    } else {
        return false;
    }

    let deps = atom_dependencies(atom);
    for dep_key in &deps {
        let dep_name = strip_prefix(dep_key);
        let Some(dep_atom) = atoms.get(dep_key) else {
            continue;
        };
        let code_path = &dep_atom.code_path;
        if code_path.ends_with("Funs.lean") && !compute_fully_verified(dep_name, atoms, cache) {
            return false;
        }
    }

    cache.insert(lean_name.to_string(), true);
    true
}

// ---------------------------------------------------------------------------
// Enrichment for atoms (used by `extract` command)
// ---------------------------------------------------------------------------

/// Statistics from the Aeneas enrichment pass.
#[derive(Default)]
pub struct AeneasEnrichStats {
    pub artifacts: usize,
    pub hidden: usize,
    pub ignored: usize,
    pub relevant: usize,
    pub externally_verified: usize,
}

/// Aeneas-specific enrichment pass for Lean atoms in a merged atom map.
///
/// Computes `is-hidden`, `is-extraction-artifact`, `is-relevant` (refined),
/// `is-externally-verified`, and applies optional config overrides for
/// `is-hidden` (project tail) and `is-ignored` (always manual).
pub fn enrich_lean_atom_flags(
    merged: &mut BTreeMap<String, Atom>,
    rust_crate_name: &str,
    config: &AeneasConfig,
) {
    let mut stats = AeneasEnrichStats::default();

    for (key, atom) in merged.iter_mut() {
        if atom.language != "lean" {
            continue;
        }

        let display_name = &atom.display_name;
        let name_no_prefix = strip_prefix(key);
        let attrs = atom_attrs(atom);
        let rust_source = atom_rust_source(atom).to_string();

        let artifact = is_extraction_artifact(display_name);
        if artifact {
            stats.artifacts += 1;
        }
        atom.extensions.insert(
            "is-extraction-artifact".to_string(),
            serde_json::json!(artifact),
        );

        let hidden = is_hidden(name_no_prefix, &attrs, config);
        if hidden {
            stats.hidden += 1;
        }
        atom.extensions
            .insert("is-hidden".to_string(), serde_json::json!(hidden));

        let ignored = config.ignored.contains(name_no_prefix);
        if ignored {
            stats.ignored += 1;
        }
        atom.extensions
            .insert("is-ignored".to_string(), serde_json::json!(ignored));

        let relevant = if rust_source.is_empty() || rust_source == "null" {
            atom.extensions
                .get("is-in-package")
                .and_then(|v| v.as_bool())
                .unwrap_or(true)
        } else {
            is_relevant(&rust_source, rust_crate_name)
        };
        if relevant {
            stats.relevant += 1;
        }
        atom.extensions
            .insert("is-relevant".to_string(), serde_json::json!(relevant));

        let ext_verified = is_externally_verified(&attrs);
        if ext_verified {
            stats.externally_verified += 1;
            atom.extensions.insert(
                "is-externally-verified".to_string(),
                serde_json::json!(true),
            );
        }
    }

    println!("\nAeneas enrichment (Lean atoms):");
    println!("  Extraction artifacts: {}", stats.artifacts);
    println!("  Hidden:              {}", stats.hidden);
    println!("  Ignored:             {}", stats.ignored);
    println!("  Relevant:            {}", stats.relevant);
    if stats.externally_verified > 0 {
        println!("  Externally verified: {}", stats.externally_verified);
    }
}

// ---------------------------------------------------------------------------
// Stats helpers
// ---------------------------------------------------------------------------

struct FunctionEnrichStats {
    total: usize,
    visible: usize,
    specified: usize,
    verified: usize,
    fully_verified: usize,
    externally_verified: usize,
    hidden: usize,
    artifacts: usize,
}

fn compute_enrichment_stats(functions: &[EnrichedFunctionOutput]) -> FunctionEnrichStats {
    let visible: Vec<_> = functions
        .iter()
        .filter(|f| !f.is_hidden && !f.is_extraction_artifact)
        .collect();
    FunctionEnrichStats {
        total: functions.len(),
        visible: visible.len(),
        specified: visible.iter().filter(|f| f.specified).count(),
        verified: visible.iter().filter(|f| f.verified).count(),
        fully_verified: visible.iter().filter(|f| f.fully_verified).count(),
        externally_verified: visible.iter().filter(|f| f.externally_verified).count(),
        hidden: functions.iter().filter(|f| f.is_hidden).count(),
        artifacts: functions
            .iter()
            .filter(|f| f.is_extraction_artifact)
            .count(),
    }
}

fn print_enrichment_stats(stats: &FunctionEnrichStats) {
    println!(
        "\nEnriched {} functions ({} visible)",
        stats.total, stats.visible
    );
    println!(
        "  verified={}  specified={}  externally_verified={}  fully_verified={}",
        stats.verified, stats.specified, stats.externally_verified, stats.fully_verified,
    );
    println!(
        "  hidden={}  extraction_artifacts={}",
        stats.hidden, stats.artifacts,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use probe::types::CodeText;

    use super::*;

    fn test_atom() -> Atom {
        Atom {
            display_name: String::new(),
            dependencies: BTreeSet::new(),
            code_module: String::new(),
            code_path: String::new(),
            code_text: CodeText::default(),
            kind: String::new(),
            language: String::new(),
            extensions: BTreeMap::new(),
        }
    }

    #[test]
    fn artifact_suffixes_detected() {
        assert!(is_extraction_artifact("foo_body"));
        assert!(is_extraction_artifact("bar_loop"));
        assert!(is_extraction_artifact("baz_loop0"));
        assert!(is_extraction_artifact("baz_loop3"));
        assert!(is_extraction_artifact("qux_loop.body"));
        assert!(!is_extraction_artifact("foo"));
        assert!(!is_extraction_artifact("loop_helper"));
        assert!(!is_extraction_artifact("body_parser"));
    }

    #[test]
    fn boilerplate_insts_detected() {
        assert!(is_boilerplate_insts("Scalar.Insts.CoreCloneClone"));
        assert!(is_boilerplate_insts("Scalar.Insts.CoreCloneClone.clone"));
        assert!(is_boilerplate_insts("Foo.Insts.CoreMarkerCopy"));
        assert!(is_boilerplate_insts("Foo.Insts.CoreDefaultDefault"));
        assert!(is_boilerplate_insts("Foo.Insts.CoreDefaultDefault.default"));
        assert!(is_boilerplate_insts("Foo.Insts.ZeroizeZeroize"));
        assert!(is_boilerplate_insts("Foo.Insts.ZeroizeZeroize.zeroize"));
        assert!(is_boilerplate_insts("Foo.Insts.ZeroizeDefaultIsZeroes"));
        assert!(is_boilerplate_insts(
            "Foo.Insts.CoreMarkerStructuralPartialEq"
        ));
    }

    #[test]
    fn meaningful_insts_not_boilerplate() {
        assert!(!is_boilerplate_insts(
            "Scalar.Insts.CoreOpsArithAddScalarScalar"
        ));
        assert!(!is_boilerplate_insts(
            "Scalar.Insts.CoreOpsArithMulScalarScalar.mul"
        ));
        assert!(!is_boilerplate_insts(
            "EdwardsPoint.Insts.SubtleConditionallySelectable"
        ));
        assert!(!is_boilerplate_insts(
            "Scalar.Insts.SubtleConstantTimeEq.ct_eq"
        ));
        assert!(!is_boilerplate_insts(
            "Scalar.Insts.CoreConvertFromU64.from"
        ));
        assert!(!is_boilerplate_insts(
            "EdwardsPoint.Insts.CoreCmpPartialEqEdwardsPoint.eq"
        ));
        assert!(!is_boilerplate_insts("EdwardsPoint.Insts.CoreCmpEq"));
        assert!(!is_boilerplate_insts(
            "RistrettoPoint.Insts.Curve25519_dalekTraitsIdentity.identity"
        ));
        assert!(!is_boilerplate_insts("no.Insts.segment"));
    }

    #[test]
    fn boilerplate_insts_no_match_without_insts() {
        assert!(!is_boilerplate_insts("CoreCloneClone"));
        assert!(!is_boilerplate_insts("foo.bar.CoreDefaultDefault"));
        assert!(!is_boilerplate_insts(""));
    }

    #[test]
    fn hidden_by_name_patterns() {
        // Boilerplate .Insts. → hidden
        assert!(is_hidden_by_name("Foo.Insts.CoreCloneClone"));
        assert!(is_hidden_by_name("Foo.Insts.CoreDefaultDefault.default"));
        // Meaningful .Insts. → NOT hidden by name alone
        assert!(!is_hidden_by_name("Foo.Insts.CoreOpsArithAddXY"));
        assert!(!is_hidden_by_name(
            "Foo.Insts.SubtleConditionallySelectable"
        ));
        // Other patterns still hidden
        assert!(is_hidden_by_name("foo.mutual"));
        assert!(is_hidden_by_name("foo.closure.anon"));
        assert!(is_hidden_by_name("Foo.Blanket.Bar"));
        assert!(is_hidden_by_name("DOC_HIDDEN_CONST"));
        // Normal names not hidden
        assert!(!is_hidden_by_name("foo.bar.baz"));
        assert!(!is_hidden_by_name("Scalar.reduce"));
    }

    #[test]
    fn hidden_with_attrs_and_config() {
        let config = AeneasConfig::default();

        let trait_attrs = vec!["rust_trait_impl".to_string()];
        assert!(is_hidden("normal.name", &trait_attrs, &config));

        assert!(!is_hidden("normal.name", &[], &config));

        let mut config_with_hidden = AeneasConfig::default();
        config_with_hidden
            .hidden
            .insert("manually_hidden".to_string());
        assert!(is_hidden("manually_hidden", &[], &config_with_hidden));
    }

    #[test]
    fn hidden_meaningful_insts_not_hidden() {
        let config = AeneasConfig::default();
        assert!(!is_hidden(
            "Scalar.Insts.CoreOpsArithAddScalarScalar",
            &[],
            &config
        ));
        assert!(!is_hidden(
            "EdwardsPoint.Insts.SubtleConditionallySelectable.conditional_select",
            &[],
            &config
        ));
        assert!(is_hidden("Scalar.Insts.CoreCloneClone.clone", &[], &config));
    }

    #[test]
    fn is_relevant_logic() {
        assert!(is_relevant("", "mycrate"));
        assert!(is_relevant("null", "mycrate"));
        assert!(is_relevant("anything", ""));
        assert!(is_relevant("mycrate/src/lib.rs", "mycrate"));
        assert!(!is_relevant("/absolute/path/mycrate/src/lib.rs", "mycrate"));
        assert!(!is_relevant(
            "registry/cargo/registry/mycrate/src/lib.rs",
            "mycrate"
        ));
        assert!(!is_relevant("othercrate/src/lib.rs", "mycrate"));
    }

    #[test]
    fn externally_verified_check() {
        let attrs = vec!["externally_verified".to_string()];
        assert!(is_externally_verified(&attrs));
        assert!(!is_externally_verified(&[]));
        assert!(!is_externally_verified(&["other".to_string()]));
    }

    #[test]
    fn structure_and_type_alias_checks() {
        assert!(is_structure("structure"));
        assert!(!is_structure("def"));

        assert!(is_type_alias("def", &["reducible".to_string()]));
        assert!(!is_type_alias("def", &[]));
        assert!(!is_type_alias("theorem", &["reducible".to_string()]));
    }

    #[test]
    fn strip_prefix_removes_probe_prefix() {
        assert_eq!(strip_prefix("probe:Foo.bar"), "Foo.bar");
        assert_eq!(strip_prefix("Foo.bar"), "Foo.bar");
        assert_eq!(strip_prefix("probe:"), "");
    }

    #[test]
    fn atom_attrs_extracts_strings() {
        let mut atom = test_atom();
        atom.extensions.insert(
            "attributes".to_string(),
            serde_json::json!(["rust_trait_impl", "reducible"]),
        );
        let attrs = atom_attrs(&atom);
        assert_eq!(attrs, vec!["rust_trait_impl", "reducible"]);
    }

    #[test]
    fn atom_attrs_empty_when_missing() {
        let atom = test_atom();
        assert!(atom_attrs(&atom).is_empty());
    }

    #[test]
    fn find_primary_spec_by_extension() {
        let mut atoms = BTreeMap::new();

        let mut func_atom = test_atom();
        func_atom
            .extensions
            .insert("primary-spec".to_string(), serde_json::json!("MyFunc_spec"));
        atoms.insert("probe:MyFunc".to_string(), func_atom);

        let mut spec_atom = test_atom();
        spec_atom.code_path = "Specs.lean".to_string();
        atoms.insert("probe:MyFunc_spec".to_string(), spec_atom);

        let (key, atom) = find_primary_spec("MyFunc", &atoms);
        assert_eq!(key, Some("probe:MyFunc_spec".to_string()));
        assert!(atom.is_some());
    }

    #[test]
    fn find_primary_spec_by_name_convention() {
        let mut atoms = BTreeMap::new();

        let func_atom = test_atom();
        atoms.insert("probe:MyFunc".to_string(), func_atom);

        let mut spec_atom = test_atom();
        spec_atom.code_path = "Specs.lean".to_string();
        atoms.insert("probe:MyFunc_spec".to_string(), spec_atom);

        let (key, atom) = find_primary_spec("MyFunc", &atoms);
        assert_eq!(key, Some("probe:MyFunc_spec".to_string()));
        assert!(atom.is_some());
    }

    #[test]
    fn find_primary_spec_none_when_missing() {
        let atoms = BTreeMap::new();
        let (key, atom) = find_primary_spec("NoSuchFunc", &atoms);
        assert!(key.is_none());
        assert!(atom.is_none());
    }

    #[test]
    fn compute_fully_verified_simple_case() {
        let mut atoms = BTreeMap::new();

        let mut func_atom = test_atom();
        func_atom
            .extensions
            .insert("dependencies".to_string(), serde_json::json!([]));
        func_atom
            .extensions
            .insert("primary-spec".to_string(), serde_json::json!("Foo_spec"));
        atoms.insert("probe:Foo".to_string(), func_atom);

        let mut spec_atom = test_atom();
        spec_atom.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("verified"),
        );
        atoms.insert("probe:Foo_spec".to_string(), spec_atom);

        let mut cache = HashMap::new();
        assert!(compute_fully_verified("Foo", &atoms, &mut cache));
    }

    #[test]
    fn compute_fully_verified_unverified_returns_false() {
        let mut atoms = BTreeMap::new();

        let mut func_atom = test_atom();
        func_atom
            .extensions
            .insert("dependencies".to_string(), serde_json::json!([]));
        func_atom
            .extensions
            .insert("primary-spec".to_string(), serde_json::json!("Foo_spec"));
        atoms.insert("probe:Foo".to_string(), func_atom);

        let mut spec_atom = test_atom();
        spec_atom.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("unverified"),
        );
        atoms.insert("probe:Foo_spec".to_string(), spec_atom);

        let mut cache = HashMap::new();
        assert!(!compute_fully_verified("Foo", &atoms, &mut cache));
    }

    #[test]
    fn compute_fully_verified_no_spec_returns_false() {
        let mut atoms = BTreeMap::new();
        let func_atom = test_atom();
        atoms.insert("probe:Foo".to_string(), func_atom);

        let mut cache = HashMap::new();
        assert!(!compute_fully_verified("Foo", &atoms, &mut cache));
    }

    #[test]
    fn compute_fully_verified_transitive_dep_in_funs() {
        let mut atoms = BTreeMap::new();

        let mut parent = test_atom();
        parent.extensions.insert(
            "dependencies".to_string(),
            serde_json::json!(["probe:Child"]),
        );
        parent
            .extensions
            .insert("primary-spec".to_string(), serde_json::json!("Parent_spec"));
        atoms.insert("probe:Parent".to_string(), parent);

        let mut parent_spec = test_atom();
        parent_spec.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("verified"),
        );
        atoms.insert("probe:Parent_spec".to_string(), parent_spec);

        let mut child = test_atom();
        child.code_path = "Project/Funs.lean".to_string();
        child
            .extensions
            .insert("dependencies".to_string(), serde_json::json!([]));
        child
            .extensions
            .insert("primary-spec".to_string(), serde_json::json!("Child_spec"));
        atoms.insert("probe:Child".to_string(), child);

        let mut child_spec = test_atom();
        child_spec.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("verified"),
        );
        atoms.insert("probe:Child_spec".to_string(), child_spec);

        let mut cache = HashMap::new();
        assert!(compute_fully_verified("Parent", &atoms, &mut cache));
    }

    #[test]
    fn compute_fully_verified_transitive_dep_unverified() {
        let mut atoms = BTreeMap::new();

        let mut parent = test_atom();
        parent.extensions.insert(
            "dependencies".to_string(),
            serde_json::json!(["probe:Child"]),
        );
        parent
            .extensions
            .insert("primary-spec".to_string(), serde_json::json!("Parent_spec"));
        atoms.insert("probe:Parent".to_string(), parent);

        let mut parent_spec = test_atom();
        parent_spec.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("verified"),
        );
        atoms.insert("probe:Parent_spec".to_string(), parent_spec);

        let mut child = test_atom();
        child.code_path = "Project/Funs.lean".to_string();
        child
            .extensions
            .insert("dependencies".to_string(), serde_json::json!([]));
        child
            .extensions
            .insert("primary-spec".to_string(), serde_json::json!("Child_spec"));
        atoms.insert("probe:Child".to_string(), child);

        let mut child_spec = test_atom();
        child_spec.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("unverified"),
        );
        atoms.insert("probe:Child_spec".to_string(), child_spec);

        let mut cache = HashMap::new();
        assert!(!compute_fully_verified("Parent", &atoms, &mut cache));
    }

    #[test]
    fn spec_override_unhides_specified_function() {
        let mut atoms = BTreeMap::new();

        let mut func_atom = test_atom();
        func_atom.extensions.insert(
            "attributes".to_string(),
            serde_json::json!(["rust_trait_impl"]),
        );
        func_atom
            .extensions
            .insert("dependencies".to_string(), serde_json::json!([]));
        func_atom.extensions.insert(
            "primary-spec".to_string(),
            serde_json::json!("TraitFunc_spec"),
        );
        func_atom
            .extensions
            .insert("is-in-package".to_string(), serde_json::json!(true));
        atoms.insert("probe:TraitFunc".to_string(), func_atom);

        let mut spec_atom = test_atom();
        spec_atom.code_path = "Specs/TraitFunc.lean".to_string();
        spec_atom.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("verified"),
        );
        atoms.insert("probe:TraitFunc_spec".to_string(), spec_atom);

        let records = vec![FunctionRecord {
            lean_name: "TraitFunc".to_string(),
            rust_name: None,
            source: None,
            lines: None,
            is_hidden: false,
            is_extraction_artifact: false,
        }];
        let config = AeneasConfig::default();
        let results = enrich_function_records(&records, &atoms, "mycrate", &config);

        assert_eq!(results.len(), 1);
        // rust_trait_impl would hide it, but spec override keeps it visible
        assert!(!results[0].is_hidden);
        assert!(results[0].specified);
        assert!(results[0].verified);
    }

    #[test]
    fn boilerplate_insts_hidden_when_no_spec() {
        let mut atoms = BTreeMap::new();

        let mut func_atom = test_atom();
        func_atom
            .extensions
            .insert("dependencies".to_string(), serde_json::json!([]));
        func_atom
            .extensions
            .insert("is-in-package".to_string(), serde_json::json!(true));
        atoms.insert(
            "probe:Foo.Insts.CoreCloneClone.clone".to_string(),
            func_atom,
        );

        let records = vec![FunctionRecord {
            lean_name: "Foo.Insts.CoreCloneClone.clone".to_string(),
            rust_name: None,
            source: None,
            lines: None,
            is_hidden: false,
            is_extraction_artifact: false,
        }];
        let config = AeneasConfig::default();
        let results = enrich_function_records(&records, &atoms, "mycrate", &config);

        assert_eq!(results.len(), 1);
        assert!(results[0].is_hidden);
        assert!(!results[0].specified);
    }

    #[test]
    fn borrow_delegator_shared_a_receiver() {
        assert!(is_borrow_delegator(
            "crate.SharedAScalar.Insts.CoreOpsArithMulEdwardsPointEdwardsPoint.mul"
        ));
        assert!(is_borrow_delegator(
            "crate.SharedAEdwardsPoint.Insts.CoreOpsArithAddEdwardsPointEdwardsPoint.add"
        ));
    }

    #[test]
    fn borrow_delegator_shared_b_in_trait_args() {
        assert!(is_borrow_delegator(
            "crate.edwards.EdwardsPoint.Insts.CoreOpsArithAddSharedBEdwardsPointEdwardsPoint.add"
        ));
        assert!(is_borrow_delegator(
            "crate.scalar.Scalar.Insts.CoreOpsArithAddSharedBScalarScalar.add"
        ));
    }

    #[test]
    fn borrow_delegator_shared0_not_hidden() {
        assert!(!is_borrow_delegator(
            "crate.Shared0EdwardsPoint.Insts.CoreOpsArithAddSharedAProjectiveNielsPointCompletedPoint.add"
        ));
        assert!(!is_borrow_delegator(
            "crate.Shared0Scalar.Insts.CoreOpsArithAddSharedAScalarScalar.add"
        ));
    }

    #[test]
    fn borrow_delegator_no_insts_not_matched() {
        assert!(!is_borrow_delegator("SharedAScalar.mul"));
        assert!(!is_borrow_delegator("normal.function"));
        assert!(!is_borrow_delegator(""));
    }

    #[test]
    fn borrow_delegator_hidden_by_name() {
        assert!(is_hidden_by_name(
            "crate.SharedAScalar.Insts.CoreOpsArithMulXY.mul"
        ));
        assert!(is_hidden_by_name(
            "crate.X.Insts.CoreOpsArithAddSharedBYZ.add"
        ));
        assert!(!is_hidden_by_name(
            "crate.Shared0X.Insts.CoreOpsArithAddSharedAYZ.add"
        ));
    }

    #[test]
    fn externally_verified_exclusive_from_verified() {
        let mut atoms = BTreeMap::new();

        let mut func_atom = test_atom();
        func_atom
            .extensions
            .insert("dependencies".to_string(), serde_json::json!([]));
        func_atom
            .extensions
            .insert("is-in-package".to_string(), serde_json::json!(true));
        func_atom
            .extensions
            .insert("primary-spec".to_string(), serde_json::json!("Foo_spec"));
        atoms.insert("probe:Foo".to_string(), func_atom);

        let mut spec_atom = test_atom();
        spec_atom.extensions.insert(
            "verification-status".to_string(),
            serde_json::json!("unverified"),
        );
        spec_atom.extensions.insert(
            "attributes".to_string(),
            serde_json::json!(["externally_verified"]),
        );
        atoms.insert("probe:Foo_spec".to_string(), spec_atom);

        let records = vec![FunctionRecord {
            lean_name: "Foo".to_string(),
            rust_name: None,
            source: None,
            lines: None,
            is_hidden: false,
            is_extraction_artifact: false,
        }];
        let config = AeneasConfig::default();
        let results = enrich_function_records(&records, &atoms, "", &config);

        assert_eq!(results.len(), 1);
        assert!(results[0].externally_verified);
        assert!(
            !results[0].verified,
            "externally_verified should be exclusive from verified"
        );
        assert!(results[0].specified);
    }

    #[test]
    fn single_child_parent_hidden() {
        let mut atoms = BTreeMap::new();

        for name in &["Foo.Insts.TraitName", "Foo.Insts.TraitName.method"] {
            let mut a = test_atom();
            a.extensions
                .insert("is-in-package".to_string(), serde_json::json!(true));
            a.extensions
                .insert("dependencies".to_string(), serde_json::json!([]));
            atoms.insert(format!("probe:{name}"), a);
        }

        let records = vec![
            FunctionRecord {
                lean_name: "Foo.Insts.TraitName".to_string(),
                rust_name: None,
                source: None,
                lines: None,
                is_hidden: false,
                is_extraction_artifact: false,
            },
            FunctionRecord {
                lean_name: "Foo.Insts.TraitName.method".to_string(),
                rust_name: None,
                source: None,
                lines: None,
                is_hidden: false,
                is_extraction_artifact: false,
            },
        ];
        let config = AeneasConfig::default();
        let results = enrich_function_records(&records, &atoms, "", &config);

        assert_eq!(results.len(), 2);
        assert!(
            results[0].is_hidden,
            "parent with one child should be hidden"
        );
        assert_eq!(
            results[0].nested_children,
            vec!["Foo.Insts.TraitName.method"]
        );
        assert!(!results[1].is_hidden, "child method should remain visible");
    }
}
