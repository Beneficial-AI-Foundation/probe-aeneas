use std::collections::{BTreeMap, HashMap};

use probe::types::Atom;
use serde::Serialize;

use crate::aeneas_config::AeneasConfig;
use crate::types::FunctionRecord;

/// Probe-lean atom key prefix.
pub const PROBE_PREFIX: &str = "probe:";

/// Standard Aeneas extraction artifact suffixes (underscore-joined).
pub const ARTIFACT_SUFFIXES: &[&str] =
    &["_body", "_loop", "_loop0", "_loop1", "_loop2", "_loop3"];

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

/// Check if a name should be hidden based on naming patterns and attributes.
///
/// Returns `true` for trait instance wrappers (`.Insts.`), mutual loop defs
/// (`.mutual`), closures (`.closure`), blanket impls (`.Blanket.`),
/// DOC_HIDDEN constants, and names in the config's hidden set.
pub fn is_hidden(name: &str, attrs: &[String], config: &AeneasConfig) -> bool {
    let has_trait_attr = attrs.iter().any(|a| a == "rust_trait_impl");
    let has_insts_pattern = name.contains(".Insts.");
    let is_mutual_loop = name.ends_with(".mutual");
    let has_closure = name.contains(".closure");
    let has_blanket = name.contains(".Blanket.");
    let has_doc_hidden = name.contains("DOC_HIDDEN");
    let in_config = config.hidden.contains(name);
    has_trait_attr
        || has_insts_pattern
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
    name.contains(".Insts.")
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

        let func_is_hidden = is_hidden(&rec.lean_name, &attrs, config)
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

        let spec_file = spec_atom.map(|a| a.code_path.clone()).filter(|p| !p.is_empty());

        let (spec_docstring, spec_statement) = extract_spec_text(spec_atom);

        let dependencies: Vec<String> = deps_raw
            .iter()
            .map(|d| strip_prefix(d).to_string())
            .collect();

        let fully_verified =
            compute_fully_verified(&rec.lean_name, atoms, &mut fv_cache);

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

    let stats = compute_enrichment_stats(&results);
    print_enrichment_stats(&stats);
    results
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
        if let Some(ps) = atom
            .extensions
            .get("primary-spec")
            .and_then(|v| v.as_str())
        {
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
        let is_verified = spec_atom
            .and_then(|a| {
                a.extensions
                    .get("verification-status")
                    .and_then(|v| v.as_str())
            })
            .map(|s| s == "verified")
            .unwrap_or(false);
        if !is_verified {
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
        if code_path.ends_with("Funs.lean") {
            if !compute_fully_verified(dep_name, atoms, cache) {
                return false;
            }
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
        artifacts: functions.iter().filter(|f| f.is_extraction_artifact).count(),
    }
}

fn print_enrichment_stats(stats: &FunctionEnrichStats) {
    println!("\nEnriched {} functions ({} visible)", stats.total, stats.visible);
    println!(
        "  verified={}  specified={}  externally_verified={}  fully_verified={}",
        stats.verified, stats.specified, stats.externally_verified, stats.fully_verified,
    );
    println!(
        "  hidden={}  extraction_artifacts={}",
        stats.hidden, stats.artifacts,
    );
}
