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
///
/// `manifest_charon_version` is the charon version recorded in Aeneas's
/// `translation.json` (`None` when no manifest is available). It provenance-gates
/// the charon-`def_id` join ([`strategy_charon_def_id`]): the integer join is
/// only sound when a Rust atom's `charon-def-id` was produced by the same charon
/// *version* as the manifest's `def_id`s. Version equality is best-effort
/// provenance, not proof of an identical run -- two runs of the same charon
/// version with different cargo flags/sources can still assign different ids
/// (the durable fix is a charon commit hash or LLBC digest; see
/// docs/upstream-issues/charon-expose-commit-and-versioned-releases.md). When the
/// versions disagree (or either is absent), the join is skipped and matching
/// falls back to the name-based strategies. See docs/charon-def-id-matching-plan.md.
pub fn generate_translations(
    rust_data: &BTreeMap<String, Atom>,
    lean_data: &BTreeMap<String, Atom>,
    functions: &[FunctionRecord],
    manifest_charon_version: Option<&str>,
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

    // Strategy 0: charon-def-id integer join (runs first, provenance-gated).
    // Precise for free when probe-rust emits `charon-def-id` from the same
    // charon run that produced the manifest; a no-op otherwise.
    strategy_charon_def_id(
        rust_data,
        lean_data,
        functions,
        manifest_charon_version,
        &mut mappings,
        &mut matched_rust,
        &mut matched_lean,
    );

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
/// The loop/artifact dimension is authoritative when Aeneas's `translation.json`
/// covers the entry (`FunctionRecord::is_loop_artifact` is `Some`): its `loop`
/// field is ground truth, so a real function named `foo_body` is not deferred and
/// a loop helper with an unusual name is. When the manifest is absent, the check
/// falls back to the `functions.json` flags and name-suffix heuristics, so files
/// with incomplete flags (issue #2) are still classified correctly. The hidden
/// dimension (trait boilerplate, closures, …) is always heuristic — the manifest
/// says nothing about it.
///
/// Deferral is never exclusion: a deferred entry (including a false positive
/// from the name heuristic, e.g. a real function named `foo_loop`) still
/// binds in the second pass if its Rust atom is otherwise unmatched.
fn is_deferred_entry(func: &FunctionRecord) -> bool {
    let artifact = match func.is_loop_artifact {
        Some(is_loop) => is_loop,
        None => func.is_extraction_artifact || enrich::is_extraction_artifact(&func.lean_name),
    };
    artifact || func.is_hidden || enrich::is_hidden_by_name(&func.lean_name)
}

/// Build `def_id → primary lean_name` from the function records.
///
/// The primary is the top-level def of a `def_id` family: the entry that is
/// *not* a loop artifact (`is_loop_artifact != Some(true)`; both `Some(false)`
/// and `None` count as primary). Loop helpers share their parent's `def_id`, so
/// they are excluded — the join must land on the spec-carrying primary, not a
/// helper. Entries without a `def_id` (no manifest coverage) contribute nothing.
///
/// Only records whose `def_id` is a charon `FunDeclId` (`def_id_is_fun_decl`)
/// are considered. charon numbers `GlobalDeclId`/`TraitImplId` in separate id
/// spaces, so a global's or trait-impl's `def_id` can share an integer with an
/// unrelated `FunDeclId`; including them would let probe-rust's `charon-def-id`
/// (always a `FunDeclId`) bind to the wrong Lean def. The id-join therefore
/// covers only the manifest's `functions` array.
///
/// Normally exactly one primary exists per `def_id`. The lexicographically
/// smallest `lean_name` is the deterministic tie-break, so the map is
/// independent of `functions` order (P14) even if a family ever exposed more
/// than one non-loop entry.
fn build_def_id_to_primary_lean(functions: &[FunctionRecord]) -> HashMap<u64, String> {
    let mut map: HashMap<u64, String> = HashMap::new();
    for func in functions {
        if !func.def_id_is_fun_decl {
            continue;
        }
        let Some(def_id) = func.def_id else { continue };
        if func.is_loop_artifact == Some(true) || func.lean_name.is_empty() {
            continue;
        }
        map.entry(def_id)
            .and_modify(|existing| {
                if func.lean_name < *existing {
                    *existing = func.lean_name.clone();
                }
            })
            .or_insert_with(|| func.lean_name.clone());
    }
    map
}

/// Strategy 0: join Rust atoms to Lean translations by charon `FunDeclId`
/// integer equality — precise, with no name normalization.
///
/// Aeneas's `translation.json` `def_id` **is** the charon `FunDeclId`; probe-rust
/// resolves each atom's `FunDecl` by source span and emits it as the
/// `charon-def-id` extension. Equal ids therefore denote the same function, so
/// binding a Rust atom to `primary(def_id)` is exact.
///
/// **Provenance gate**: ids from different charon runs point at different
/// functions, silently corrupting the mapping. The join runs only when
/// `manifest_charon_version` is present and each atom's own `charon-version`
/// matches it; otherwise the atom is skipped and the name strategies handle it.
///
/// Forward-compatible no-op until probe-rust emits `charon-def-id`: with no such
/// atoms the loop binds nothing and output is unchanged. Respects
/// `matched_rust`/`matched_lean` (P11) so later strategies never re-bind.
#[allow(clippy::too_many_arguments)]
fn strategy_charon_def_id(
    rust_data: &BTreeMap<String, Atom>,
    lean_data: &BTreeMap<String, Atom>,
    functions: &[FunctionRecord],
    manifest_charon_version: Option<&str>,
    mappings: &mut Vec<Mapping>,
    matched_rust: &mut HashSet<String>,
    matched_lean: &mut HashSet<String>,
) {
    // Without a manifest charon version there is nothing to compare a
    // `charon-def-id` against, so the integer join is unsafe: bail out. An empty
    // version string is treated as absent so two empty strings never satisfy the
    // gate by accident.
    let Some(manifest_version) = manifest_charon_version.filter(|v| !v.is_empty()) else {
        return;
    };

    let def_id_to_lean = build_def_id_to_primary_lean(functions);
    if def_id_to_lean.is_empty() {
        return;
    }

    // Tracks which `charon-def-id` each already-bound Rust atom carried, to
    // detect the invariant violation of two atoms sharing one `FunDeclId`.
    let mut def_id_owner: HashMap<u64, String> = HashMap::new();

    for (code_name, atom) in rust_data {
        // Strategy 0 runs first, so `matched_rust` is empty here today; the guard
        // is future-safety in case a strategy is ever inserted before this one.
        if matched_rust.contains(code_name) {
            continue;
        }
        // Per-atom provenance gate: trust the id only when it came from the same
        // charon version as the manifest.
        let atom_version = atom
            .extensions
            .get("charon-version")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty());
        if atom_version != Some(manifest_version) {
            continue;
        }
        let Some(def_id) = atom
            .extensions
            .get("charon-def-id")
            .and_then(|v| v.as_u64())
        else {
            continue;
        };
        // A `FunDeclId` identifies exactly one Rust function, so two atoms
        // claiming the same id means an upstream bug (span collision, macro
        // expansion). First-iterated wins (deterministic: `rust_data` is a
        // `BTreeMap`); warn so the anomaly is not silent, and let the loser fall
        // through to the name strategies.
        if let Some(first) = def_id_owner.get(&def_id) {
            eprintln!(
                "  ⚠ charon-def-id {def_id} claimed by two Rust atoms ({first} and \
                 {code_name}); keeping the first and deferring the second to name matching."
            );
            continue;
        }
        def_id_owner.insert(def_id, code_name.clone());
        let Some(lean_name) = def_id_to_lean.get(&def_id) else {
            continue;
        };
        let lean_code_name = format!("probe:{lean_name}");
        if lean_data.contains_key(&lean_code_name) && !matched_lean.contains(&lean_code_name) {
            mappings.push(Mapping {
                from: code_name.clone(),
                to: lean_code_name.clone(),
                confidence: "exact".to_string(),
                method: Some("charon-def-id".to_string()),
            });
            matched_rust.insert(code_name.clone());
            matched_lean.insert(lean_code_name);
        }
    }
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
            ..Default::default()
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

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);
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

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);
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

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);
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

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);
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

        let (mappings, _stats) = generate_translations(&rust_atoms, &lean, &funcs, None);

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
        // Overlay flags mirror translation.json: helpers Some(true), primary
        // Some(false). The primary must win regardless of emission order.
        let mut body = make_func(
            "spqr.encoding.gf.parallel_mult_loop.body",
            Some("spqr::encoding::gf::parallel_mult"),
            "src/encoding/gf.rs",
            "L205-L210",
        );
        body.is_loop_artifact = Some(true);
        let mut loopfn = make_func(
            "spqr.encoding.gf.parallel_mult_loop",
            Some("spqr::encoding::gf::parallel_mult"),
            "src/encoding/gf.rs",
            "L205-L210",
        );
        loopfn.is_loop_artifact = Some(true);
        let mut primary = make_func(
            "spqr.encoding.gf.parallel_mult",
            Some("spqr::encoding::gf::parallel_mult"),
            "src/encoding/gf.rs",
            "L201-L214",
        );
        primary.is_loop_artifact = Some(false);
        let funcs = vec![body, loopfn, primary];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);

        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].to, "probe:spqr.encoding.gf.parallel_mult",
            "primary def must win over loop artifacts regardless of emission order"
        );
        assert_eq!(mappings[0].confidence, "exact");
    }

    /// translation.json overlay: a real crate function literally named
    /// `..._body` (which the name-suffix heuristic would wrongly defer) is kept
    /// primary because Aeneas's manifest says it is NOT a loop artifact
    /// (`is_loop_artifact = Some(false)`).
    #[test]
    fn manifest_overlay_rescues_suffix_false_positive() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("parse_body", "src/http.rs", 10, 20);
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("crate::http::parse_body"),
        );
        rust_atoms.insert("probe:crate/1.0/parse_body()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:crate.http.parse_body".to_string(),
            make_lean_atom("parse_body", "Funs.lean"),
        );

        // Name ends in `_body` -> is_extraction_artifact heuristic would defer it,
        // but the manifest overlay says it is the authoritative primary.
        let mut func = make_func(
            "crate.http.parse_body",
            Some("crate::http::parse_body"),
            "src/http.rs",
            "L10-L20",
        );
        func.is_loop_artifact = Some(false);

        // Sanity: without the overlay the heuristic WOULD flag this as an artifact.
        assert!(enrich::is_extraction_artifact("crate.http.parse_body"));
        assert!(!is_deferred_entry(&func));

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &[func], None);
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].to, "probe:crate.http.parse_body");
        assert_eq!(mappings[0].confidence, "exact");
    }

    /// translation.json overlay: an entry with an ordinary (non-suffix) name that
    /// Aeneas marks as a loop helper (`is_loop_artifact = Some(true)`) is deferred,
    /// even though the name heuristic alone would not catch it.
    #[test]
    fn manifest_overlay_defers_non_suffix_loop_helper() {
        let ordinary = make_func(
            "crate.math.compute",
            Some("crate::math::compute"),
            "src/math.rs",
            "L1-L5",
        );
        assert!(!enrich::is_extraction_artifact("crate.math.compute"));
        assert!(!is_deferred_entry(&ordinary));

        let mut loopy = ordinary.clone();
        loopy.is_loop_artifact = Some(true);
        assert!(is_deferred_entry(&loopy));
    }

    /// With no overlay (`is_loop_artifact = None`) the pre-existing name-heuristic
    /// behavior is preserved: a `_loop`/`.body` name is still deferred.
    #[test]
    fn no_overlay_falls_back_to_name_heuristic() {
        let mut func = make_func(
            "crate.gf.parallel_mult_loop.body",
            Some("crate::gf::parallel_mult"),
            "src/gf.rs",
            "L5-L10",
        );
        func.is_loop_artifact = None;
        assert!(is_deferred_entry(&func));
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

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);

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

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);

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

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);
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

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, None);

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

    // =========================================================================
    // Strategy 0: charon-def-id integer join (WS1)
    // =========================================================================

    /// A Rust atom carrying `charon-def-id` and `charon-version` extensions,
    /// as probe-rust will emit them (WS3).
    fn make_rust_atom_charon(
        display: &str,
        path: &str,
        start: usize,
        end: usize,
        def_id: u64,
        charon_version: &str,
    ) -> Atom {
        let mut atom = make_rust_atom(display, path, start, end);
        atom.extensions
            .insert("charon-def-id".to_string(), serde_json::json!(def_id));
        atom.extensions.insert(
            "charon-version".to_string(),
            serde_json::json!(charon_version),
        );
        atom
    }

    /// A manifest-built record: primary defs carry `is_loop_artifact = Some(false)`
    /// and a `def_id`; loop helpers carry `Some(true)` and share the `def_id`.
    fn make_func_def_id(lean_name: &str, def_id: u64, is_loop: bool) -> FunctionRecord {
        FunctionRecord {
            lean_name: lean_name.to_string(),
            def_id: Some(def_id),
            is_loop_artifact: Some(is_loop),
            // Simulates a `functions`-array record: its `def_id` is a FunDeclId.
            def_id_is_fun_decl: true,
            ..Default::default()
        }
    }

    #[test]
    fn build_def_id_map_excludes_loop_helpers_and_tiebreaks() {
        let funcs = vec![
            make_func_def_id("c.gf.div_impl", 439, false),
            make_func_def_id("c.gf.div_impl_loop", 439, true),
            make_func_def_id("c.gf.div_impl_loop.body", 439, true),
            // A record with no def_id contributes nothing.
            make_func("c.gf.other", Some("c::gf::other"), "src/gf.rs", "L1-L2"),
        ];
        let map = build_def_id_to_primary_lean(&funcs);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&439).map(String::as_str), Some("c.gf.div_impl"));
    }

    #[test]
    fn build_def_id_map_tiebreak_is_order_independent() {
        // Two non-loop entries share a def_id (pathological): the lexicographically
        // smallest lean_name wins regardless of insertion order (P14).
        let forward = build_def_id_to_primary_lean(&[
            make_func_def_id("c.aaa", 7, false),
            make_func_def_id("c.zzz", 7, false),
        ]);
        let reverse = build_def_id_to_primary_lean(&[
            make_func_def_id("c.zzz", 7, false),
            make_func_def_id("c.aaa", 7, false),
        ]);
        assert_eq!(forward.get(&7).map(String::as_str), Some("c.aaa"));
        assert_eq!(reverse.get(&7).map(String::as_str), Some("c.aaa"));
    }

    #[test]
    fn charon_def_id_join_binds_when_version_matches() {
        let mut rust_atoms = BTreeMap::new();
        rust_atoms.insert(
            "probe:spqr/1.5.0/div_impl()".to_string(),
            make_rust_atom_charon(
                "GF16::div_impl",
                "src/encoding/gf.rs",
                549,
                559,
                439,
                "0.1.217",
            ),
        );

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:spqr.encoding.gf.GF16.div_impl".to_string(),
            make_lean_atom("div_impl", "Funs.lean"),
        );

        let funcs = vec![make_func_def_id(
            "spqr.encoding.gf.GF16.div_impl",
            439,
            false,
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, Some("0.1.217"));

        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].method.as_deref(), Some("charon-def-id"));
        assert_eq!(mappings[0].confidence, "exact");
        assert_eq!(mappings[0].from, "probe:spqr/1.5.0/div_impl()");
        assert_eq!(mappings[0].to, "probe:spqr.encoding.gf.GF16.div_impl");
    }

    #[test]
    fn charon_def_id_join_binds_primary_not_loop_helper() {
        let mut rust_atoms = BTreeMap::new();
        rust_atoms.insert(
            "probe:spqr/1.5.0/div_impl()".to_string(),
            make_rust_atom_charon(
                "GF16::div_impl",
                "src/encoding/gf.rs",
                549,
                559,
                439,
                "0.1.217",
            ),
        );

        let mut lean = BTreeMap::new();
        for name in [
            "spqr.encoding.gf.GF16.div_impl",
            "spqr.encoding.gf.GF16.div_impl_loop",
            "spqr.encoding.gf.GF16.div_impl_loop.body",
        ] {
            lean.insert(
                format!("probe:{name}"),
                make_lean_atom(name.rsplit('.').next().unwrap(), "Funs.lean"),
            );
        }

        // Family shares def_id 439; only the primary is a valid target.
        let funcs = vec![
            make_func_def_id("spqr.encoding.gf.GF16.div_impl_loop.body", 439, true),
            make_func_def_id("spqr.encoding.gf.GF16.div_impl_loop", 439, true),
            make_func_def_id("spqr.encoding.gf.GF16.div_impl", 439, false),
        ];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, Some("0.1.217"));

        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].to, "probe:spqr.encoding.gf.GF16.div_impl");
        assert_eq!(mappings[0].method.as_deref(), Some("charon-def-id"));
    }

    #[test]
    fn charon_def_id_join_gated_off_on_version_mismatch() {
        // The atom's charon-def-id comes from a different charon run than the
        // manifest, so the id-join must NOT fire. A name fallback still can, but
        // never via the charon-def-id method.
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom_charon(
            "GF16::div_impl",
            "src/encoding/gf.rs",
            549,
            559,
            439,
            "0.1.174",
        );
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("spqr::encoding::gf::GF16::div_impl"),
        );
        rust_atoms.insert("probe:spqr/1.5.0/div_impl()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:spqr.encoding.gf.GF16.div_impl".to_string(),
            make_lean_atom("div_impl", "Funs.lean"),
        );

        // def_id 439 in this (newer) manifest points at a DIFFERENT function;
        // binding on it would be a silent wrong mapping.
        let mut func = make_func(
            "spqr.encoding.gf.GF16.div_impl",
            Some("spqr::encoding::gf::GF16::div_impl"),
            "src/encoding/gf.rs",
            "L549-L559",
        );
        func.def_id = Some(439);
        func.is_loop_artifact = Some(false);
        func.def_id_is_fun_decl = true;

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &[func], Some("0.1.217"));

        // The name strategy still binds it (correctly, by RQN), but the gate
        // prevented the charon-def-id method from firing.
        assert!(
            mappings
                .iter()
                .all(|m| m.method.as_deref() != Some("charon-def-id")),
            "version mismatch must disable the id-join"
        );
    }

    #[test]
    fn charon_def_id_join_gated_off_without_manifest_version() {
        // No manifest charon version -> nothing to compare against -> join off.
        // The atom also carries a `rust-qualified-name` and the record a matching
        // `rust_name`, so a NAME strategy *would* bind if the gate misfired. This
        // makes the assertion non-vacuous: we require exactly one mapping, bound
        // by the name strategy, never via charon-def-id.
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom_charon(
            "GF16::div_impl",
            "src/encoding/gf.rs",
            549,
            559,
            439,
            "0.1.217",
        );
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("spqr::encoding::gf::GF16::div_impl"),
        );
        rust_atoms.insert("probe:spqr/1.5.0/div_impl()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:spqr.encoding.gf.GF16.div_impl".to_string(),
            make_lean_atom("div_impl", "Funs.lean"),
        );

        let mut func = make_func(
            "spqr.encoding.gf.GF16.div_impl",
            Some("spqr::encoding::gf::GF16::div_impl"),
            "src/encoding/gf.rs",
            "L549-L559",
        );
        func.def_id = Some(439);
        func.is_loop_artifact = Some(false);
        func.def_id_is_fun_decl = true;

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &[func], None);

        assert_eq!(
            mappings.len(),
            1,
            "the name strategy must still bind the atom when the id-join is off"
        );
        assert_eq!(
            mappings[0].method.as_deref(),
            Some("rust-qualified-name"),
            "missing manifest charon version must disable the id-join, not name matching"
        );
    }

    #[test]
    fn charon_def_id_join_noop_when_atoms_lack_field() {
        // Forward-compatibility: with a matching manifest version but no atom
        // carrying `charon-def-id`, the join binds nothing (byte-identical output).
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("div_impl", "src/encoding/gf.rs", 549, 559);
        atom.extensions.insert(
            "rust-qualified-name".to_string(),
            serde_json::json!("spqr::encoding::gf::GF16::div_impl"),
        );
        rust_atoms.insert("probe:spqr/1.5.0/div_impl()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:spqr.encoding.gf.GF16.div_impl".to_string(),
            make_lean_atom("div_impl", "Funs.lean"),
        );

        let mut func = make_func(
            "spqr.encoding.gf.GF16.div_impl",
            Some("spqr::encoding::gf::GF16::div_impl"),
            "src/encoding/gf.rs",
            "L549-L559",
        );
        func.def_id = Some(439);
        func.is_loop_artifact = Some(false);
        func.def_id_is_fun_decl = true;

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &[func], Some("0.1.217"));

        // Falls back to the name matcher; no charon-def-id method used.
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].method.as_deref(), Some("rust-qualified-name"));
    }

    /// The coupling invariant (SCHEMA.md) says `charon-def-id` and
    /// `charon-version` are emitted together. If a producer bug ever emits an id
    /// without a version, the per-atom gate must skip it rather than bind on an
    /// id it cannot vouch for.
    #[test]
    fn charon_def_id_join_skipped_when_atom_lacks_version() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("div_impl", "src/encoding/gf.rs", 549, 559);
        atom.extensions
            .insert("charon-def-id".to_string(), serde_json::json!(439));
        rust_atoms.insert("probe:spqr/1.5.0/div_impl()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:spqr.encoding.gf.GF16.div_impl".to_string(),
            make_lean_atom("div_impl", "Funs.lean"),
        );

        let funcs = vec![make_func_def_id(
            "spqr.encoding.gf.GF16.div_impl",
            439,
            false,
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, Some("0.1.217"));

        assert!(
            mappings.is_empty(),
            "an id without a charon-version must not bind via the id-join"
        );
    }

    /// A `charon-version` with no `charon-def-id` has nothing to key on: the
    /// id-join must skip it (the name strategies still apply if names exist).
    #[test]
    fn charon_def_id_join_skipped_when_atom_lacks_def_id() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("div_impl", "src/encoding/gf.rs", 549, 559);
        atom.extensions
            .insert("charon-version".to_string(), serde_json::json!("0.1.217"));
        rust_atoms.insert("probe:spqr/1.5.0/div_impl()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:spqr.encoding.gf.GF16.div_impl".to_string(),
            make_lean_atom("div_impl", "Funs.lean"),
        );

        let funcs = vec![make_func_def_id(
            "spqr.encoding.gf.GF16.div_impl",
            439,
            false,
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, Some("0.1.217"));

        assert!(
            mappings
                .iter()
                .all(|m| m.method.as_deref() != Some("charon-def-id")),
            "a version without an id cannot drive the id-join"
        );
    }

    /// `charon-def-id` must be a JSON integer. A string (or any non-`u64`) value
    /// yields `None` from `as_u64`, so the atom is skipped rather than mis-keyed.
    #[test]
    fn charon_def_id_join_skipped_when_def_id_is_string() {
        let mut rust_atoms = BTreeMap::new();
        let mut atom = make_rust_atom("div_impl", "src/encoding/gf.rs", 549, 559);
        atom.extensions
            .insert("charon-version".to_string(), serde_json::json!("0.1.217"));
        atom.extensions
            .insert("charon-def-id".to_string(), serde_json::json!("439"));
        rust_atoms.insert("probe:spqr/1.5.0/div_impl()".to_string(), atom);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:spqr.encoding.gf.GF16.div_impl".to_string(),
            make_lean_atom("div_impl", "Funs.lean"),
        );

        let funcs = vec![make_func_def_id(
            "spqr.encoding.gf.GF16.div_impl",
            439,
            false,
        )];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, Some("0.1.217"));

        assert!(
            mappings.is_empty(),
            "a non-integer charon-def-id must not bind via the id-join"
        );
    }

    /// Globals and trait-impls carry `def_id`s from charon's separate
    /// `GlobalDeclId`/`TraitImplId` id spaces (`def_id_is_fun_decl == false`), so
    /// even when the integer equals a Rust atom's `charon-def-id` (a `FunDeclId`)
    /// the join must NOT bind them — the ids denote different declarations.
    #[test]
    fn charon_def_id_join_excludes_non_fundecl_records() {
        let mut rust_atoms = BTreeMap::new();
        rust_atoms.insert(
            "probe:spqr/1.5.0/div_impl()".to_string(),
            make_rust_atom_charon(
                "GF16::div_impl",
                "src/encoding/gf.rs",
                549,
                559,
                439,
                "0.1.217",
            ),
        );

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:spqr.encoding.gf.ALPHA".to_string(),
            make_lean_atom("ALPHA", "Funs.lean"),
        );

        // A `globals`-array record whose GlobalDeclId happens to be 439.
        let mut global = make_func_def_id("spqr.encoding.gf.ALPHA", 439, false);
        global.def_id_is_fun_decl = false;

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &[global], Some("0.1.217"));

        assert!(
            mappings.is_empty(),
            "a FunDeclId must not bind to a global/trait-impl sharing its integer"
        );
    }

    /// Two Rust atoms carrying the same `charon-def-id` violate the one-FunDecl =
    /// one-atom invariant. The first (by `BTreeMap` order) binds; the second is
    /// deferred to name matching (here it has no name, so it stays unbound).
    #[test]
    fn charon_def_id_join_first_wins_on_duplicate_def_id() {
        let mut rust_atoms = BTreeMap::new();
        rust_atoms.insert(
            "probe:x/1/a()".to_string(),
            make_rust_atom_charon("a", "src/a.rs", 1, 2, 439, "0.1.217"),
        );
        rust_atoms.insert(
            "probe:x/1/b()".to_string(),
            make_rust_atom_charon("b", "src/b.rs", 3, 4, 439, "0.1.217"),
        );

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:x.div_impl".to_string(),
            make_lean_atom("div_impl", "Funs.lean"),
        );

        let funcs = vec![make_func_def_id("x.div_impl", 439, false)];

        let (mappings, _) = generate_translations(&rust_atoms, &lean, &funcs, Some("0.1.217"));

        let id_binds: Vec<&Mapping> = mappings
            .iter()
            .filter(|m| m.method.as_deref() == Some("charon-def-id"))
            .collect();
        assert_eq!(
            id_binds.len(),
            1,
            "only one atom may claim the shared def_id"
        );
        assert_eq!(
            id_binds[0].from, "probe:x/1/a()",
            "first by BTreeMap order wins"
        );
    }

    // =========================================================================
    // RQN-pair fixture oracle (tests/fixtures/rqn_pairs.json)
    //
    // The fixture holds real spqr+dalek pairs. `matched` = the same charon
    // function rendered two ways (probe-rust RQN vs translation.json rust_name),
    // which the name matcher must reconcile via `normalize_rust_name`. `distinct`
    // = genuinely-different impls, some of which the normalizer *collapses*
    // (owned vs borrowed, `Shared0` vs `SharedA`) — the exact collisions the
    // charon-def-id join eliminates. These tests pin the naming noise (so a
    // future normalizer change is deliberate) and exercise the join's *mechanics*
    // on the must-split shapes using synthetic ids. They do NOT prove that real
    // probe-rust `charon-def-id`s equal the manifest's `def_id`s — that
    // end-to-end equality needs a real manifest (WS3), not this fixture.
    // =========================================================================

    #[derive(serde::Deserialize)]
    struct RqnFixture {
        matched: Vec<[String; 2]>,
        distinct: Vec<[String; 2]>,
    }

    fn load_rqn_fixture() -> RqnFixture {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/rqn_pairs.json");
        let content = std::fs::read_to_string(path).expect("read rqn_pairs.json fixture");
        serde_json::from_str(&content).expect("parse rqn_pairs.json fixture")
    }

    /// A borrow/owned distinct pair the normalizer collapses (its `&` is stripped
    /// by `RE_REF`), used to anchor the join-split test. `None` if the fixture
    /// ever loses such a case.
    fn first_normalizer_collision(fixture: &RqnFixture) -> Option<&[String; 2]> {
        fixture.distinct.iter().find(|[a, b]| {
            (a.contains('&') || b.contains('&')) && normalize_rust_name(a) == normalize_rust_name(b)
        })
    }

    /// `matched` pairs are the same charon function rendered two ways. The name
    /// matcher reconciles them only via `normalize_rust_name`, and it does not
    /// bridge every rendering: a residual set (e.g. probe-rust's short
    /// `GF16::const_div` vs the manifest's brace-qualified `{...GF16}::const_div`)
    /// stays unreconciled, so strategy-1 name matching misses those functions.
    ///
    /// This is precisely the noise the charon-def-id join removes — the join is
    /// name-agnostic, so it binds all matched pairs regardless of rendering. The
    /// bound is a regression pin: the normalizer must reconcile at least as many
    /// as it does today (the count can only improve).
    #[test]
    fn rqn_fixture_matched_pairs_mostly_normalize_equal() {
        // Renderings the current normalizer does NOT bridge. The id-join makes
        // this count irrelevant; lower is better, so this is an upper bound.
        const KNOWN_UNRECONCILED: usize = 29;

        let fixture = load_rqn_fixture();
        assert!(
            !fixture.matched.is_empty(),
            "fixture should have matched pairs"
        );
        let unreconciled = fixture
            .matched
            .iter()
            .filter(|[a, b]| normalize_rust_name(a) != normalize_rust_name(b))
            .count();
        assert!(
            unreconciled <= KNOWN_UNRECONCILED,
            "normalizer regressed: {unreconciled} matched pairs now fail to \
             normalize-equal (was {KNOWN_UNRECONCILED}). The id-join binds these \
             regardless, but strategy-1 name matching relies on this reconciliation."
        );
    }

    /// Oracle: the normalizer *does* collapse some genuinely-distinct impls
    /// (owned vs borrowed). Pinning this documents why the id-join exists — a
    /// future `normalize_rust_name` change that silently split or merged these is
    /// caught here.
    #[test]
    fn rqn_fixture_documents_normalizer_collisions() {
        let fixture = load_rqn_fixture();
        assert!(
            first_normalizer_collision(&fixture).is_some(),
            "expected a borrow/owned pair that the normalizer collapses — the \
             id-join's motivation; did normalize_rust_name change?"
        );
    }

    /// Join-correctness: take a real distinct pair the normalizer collapses, give
    /// the two atoms distinct `charon-def-id`s, and confirm the id-join maps each
    /// to its own Lean def — the split the name matcher cannot make.
    #[test]
    fn charon_def_id_join_splits_a_normalizer_collision() {
        let fixture = load_rqn_fixture();
        let [rqn_a, rqn_b] = first_normalizer_collision(&fixture)
            .expect("a borrow/owned normalizer collision in the fixture")
            .clone();
        // Precondition: the name matcher cannot tell these apart.
        assert_eq!(normalize_rust_name(&rqn_a), normalize_rust_name(&rqn_b));

        let mut rust = BTreeMap::new();
        let mut a = make_rust_atom_charon("add_assign", "src/encoding/gf.rs", 10, 12, 100, "v1");
        a.extensions
            .insert("rust-qualified-name".to_string(), serde_json::json!(rqn_a));
        let mut b = make_rust_atom_charon("add_assign", "src/encoding/gf.rs", 20, 22, 101, "v1");
        b.extensions
            .insert("rust-qualified-name".to_string(), serde_json::json!(rqn_b));
        rust.insert("probe:x/1/a()".to_string(), a);
        rust.insert("probe:x/1/b()".to_string(), b);

        let mut lean = BTreeMap::new();
        lean.insert(
            "probe:x.add_assign_owned".to_string(),
            make_lean_atom("add_assign_owned", "Funs.lean"),
        );
        lean.insert(
            "probe:x.add_assign_shared".to_string(),
            make_lean_atom("add_assign_shared", "Funs.lean"),
        );

        // Distinct def_ids: what the same-run LLBC would assign to the two impls.
        let funcs = vec![
            make_func_def_id("x.add_assign_owned", 100, false),
            make_func_def_id("x.add_assign_shared", 101, false),
        ];

        let (mappings, _) = generate_translations(&rust, &lean, &funcs, Some("v1"));

        assert_eq!(mappings.len(), 2, "both colliding-name atoms must map");
        assert!(
            mappings
                .iter()
                .all(|m| m.method.as_deref() == Some("charon-def-id")),
            "both must bind via the id-join, not names"
        );
        let targets: std::collections::BTreeSet<&str> =
            mappings.iter().map(|m| m.to.as_str()).collect();
        assert_eq!(
            targets.len(),
            2,
            "the id-join must send the two atoms to distinct Lean defs"
        );
    }
}
