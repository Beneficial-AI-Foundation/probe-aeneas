# Plan: match opaque `External_Template` functions

Analysis and review: [opaque-template-matching.md](opaque-template-matching.md).

## Rule

A **local template record** is a manifest entry (`functions`, `globals`, `trait_impls`) whose `lean_file` ends in `External_Template.lean` and that is local: `is_local == true`, or, when the entry has no `is_local` field, its `rust_name` starts with `<crate>::` (manifest top-level `crate`). Non-local template entries stay excluded as today.

Local template records join translation matching only. They are never written to a `functions.json` and never listed by `listfuns`.

## Changes

1. `src/types.rs` — `FunctionRecord` gains `#[serde(skip)] pub is_local_template: bool` (in-memory, like `def_id`).

2. `src/translation_manifest.rs`
   - `TranslationManifest`: `#[serde(rename = "crate", default)] pub crate_name: Option<String>`.
   - `TranslationFunc`: `#[serde(default)] pub is_local: Option<bool>`; `fn is_local_template(&self, crate_name)` implements the rule.
   - `records_from_manifest`: keep non-template entries plus local template entries (flag set); drop non-local template entries. Doc comment updated.
   - `apply(functions: &mut Vec<FunctionRecord>, …)`: after `annotate`, append the manifest's local template records (all three arrays; globals/trait_impls with `def_id_is_fun_decl = false`) whose `lean_name` is not in `functions`; a record already present is not duplicated but is marked `is_template_name` (repeat runs reuse a `functions.json` that never holds them); the resolver resolves its name and, when it renames it, gives it the template matching rules (Codex review on PR #83: a renamed but unguarded record could bind a same-named impl outside its range). When another record already holds the resolved name, the listed record merges into it (`def_id` transfer, listed record dropped), so the charon join never binds a listed stand-in (Codex review round 2). A renamed record moves after the ordinary records, with the template records, so it never wins a first-wins tie against a regular record (round 3). Template records bind only an atom of their crate: the first segment of the atom's `rust-qualified-name` (else its code-name package) equals the first segment of their `rust_name` (the package alone misses a renamed library such as `md-5`/`md5`), because `normalize_source_path` makes same-path files of different workspace members equal (Copilot review of the rewritten head). The user's records are never re-sorted (record order decides first-wins ties). Canonical placement of template records is the resolver's job (step 4). `ManifestOverlay` gains `crate_name`; its and `apply`'s doc comments say `apply` now also appends records.

3. `src/function_source.rs` — `ResolvedRecords` gains `crate_name: Option<String>`, filled from the manifest (Manifest arm) or the overlay (Override arm); `None` on Lake/Legacy.

4. `src/translate.rs`
   - New `resolve_template_lean_names(functions, lean_data, crate_name) -> Vec<FunctionRecord>` (clone of `functions`): for each `is_local_template` record, look up both the exact key `probe:{lean_name}` and the alternative `probe:{alt}`, where `alt` is `<crate>.{lean_name}` if `lean_name` does not start with `<crate>.`, else `lean_name` with exactly the `<crate>.` prefix removed. Exact exists → keep the name (warn if the alternative also exists), except that when both exist and only the exact atom's `code-path` ends in `External_Template.lean` (the uncompleted stand-in), the alternative wins. Else alternative exists → rewrite `lean_name` to `alt`. Never strip any other segment. `is_hidden`/`is_extraction_artifact` are unaffected by a leading `<crate>.` segment (and are computed before resolution).
     - **Drop** a local template record whose exact and alternative keys both miss: it can never map, and left in the file indexes it could make a `file+display-name` key ambiguous or win a `file+line-overlap` against a real record (strategy 3 checks the atom only on the winner).
     - **Dedupe templates**: if two local template records resolve to one `lean_name`, keep the one whose exact name hit, then one with `def_id_is_fun_decl`, then the smallest `def_id` (order-independent); drop the rest.
     - **Drop** a local template record whose resolved `lean_name` equals a non-template record's `lean_name` (e.g. a user/lake `functions.json` already lists `spqr.kdf.hkdf_to_slice`); if that record has no `def_id`, copy the template's `def_id`/`def_id_is_fun_decl` onto it. Avoids duplicate records that would make strategy 2 ambiguous.
     - Without `crate_name`, only the exact name is tried (atom-less local templates are still dropped).
     - Output order: non-template records in input order, then the kept local template records sorted by resolved `lean_name`. Same order on the first run (Manifest arm) and a repeat run (Override arm), and templates never come before a regular record in first-wins contention.
     - Crate spelling: `crate` is normalized `-` → `_` before the `<crate>::` and `<crate>.` checks.
     - Known risk (#61 direction): stripping `<crate>.` accepts any existing root-level atom of that name. Bounded by the locality rule (only local opaque entries) and exact-name-first; Aeneas emits regular defs inside `namespace <crate>`, so a root-level def of the same path is the hand-completed external.
   - `strategy_rust_qualified_name`: local template records form a third pass, after the preferred and deferred non-template passes, so a template whose own Rust atom is missing can never take a colliding normalized name from a regular record. A template record also binds in strategy 1 only when the Rust atom's file equals the record's `source` (checked even with one candidate).
   - `file_to_funcs` (strategy 3 index) excludes local template records without a `rust_name` (`record_may_name_atom` would not gate them).
   - `file_name_to_funcs` (strategy 2 index) excludes local template records: a kept template with an atom must not make a unique `(file, base)` key ambiguous. Templates are reached by `def_id`, qualified name, or `rust_name`-gated line overlap.
   - `generate_translations(rust, lean, functions, manifest_charon_version, crate_name)`: calls the resolver first; all four strategies, the `def_id` map, the file indexes **and `impls_with_method_records`** then use the resolved records (after the drops). An atom-less local template method record (SymCRust's `Key::box_default`, an opaque axiom) therefore never blocks the instance-of-own-impl rule in `record_may_name_atom`. When that axiom does have a Lean atom, the method maps to the axiom (its own def, by `charon-def-id` or `rust-qualified-name`) instead of the instance: decided as correct, it is the function's own translation; callers' derived statuses may change. Every `Mapping.to` is the real atom key, so downstream (`translation-*`, status copy, `mapped-from`, primary-spec lookup) needs no change.
   - Test call sites pass `None`.

5. `src/extract.rs` — `run_translate` and the `translate` subcommand pass `resolved.crate_name`.

6. `src/gen_functions.rs` — `write_functions_json` skips `is_local_template` records (covers `extract`'s write and basic `listfuns`).

7. `src/listfuns.rs` — enriched listfuns drops `is_local_template` records before enrichment, so its output (like `functions.json`) never lists them. (`detect_crate_name` is unaffected either way.)

8. `USAGE.md`: `translate` needs `--translation` to match template functions (a `functions.json` never lists them).
9. Docs: `CLAUDE.md` (Translation Matching: template records + resolver), `docs/architecture.md`, the `translation_manifest.rs` module/struct/`records_from_manifest` doc comments (which say template entries have "no local Lean atom"), `docs/SCHEMA.md` and `docs/USAGE.md` where they describe the template exclusion / name lookup. Patch bump in `Cargo.toml`.

Older manifests: if a manifest has neither `is_local` nor `crate`, no template entry is kept and #61 stays unfixed for it (conservative). #61-era manifests could not be checked here.

## Tests

Fixtures copy the SPQR entries: `def_id` 43, `lean_name: "kdf.hkdf_to_slice"`, `lean_file: "SrcTranslated/FunsExternal_Template.lean"`, `rust_name: "spqr::kdf::hkdf_to_slice"`, `is_local: true`, `is_opaque: true`, source `src/kdf.rs` L14-L18, `crate: "spqr"`, charon `0.1.304`; neighbour `def_id` 42 `spqr.kdf.hkdf_to_vec` in `Funs.lean`.

translation_manifest:
- T1 `crate` and `is_local` parse; absent fields give `None`.
- T2 `records_from_manifest` keeps the local template entry (flagged), drops a non-local `core.*` template entry, keeps regular entries unflagged; the `is_local`-absent fallback keeps `rust_name` `spqr::…` and drops `core::…`.
- T3a Manifest arm → `write_functions_json` → Override arm on that file with the same manifest yields (compared explicitly on these only) the same `functions` and template records (names, `def_id`, flags, order) and identical `generate_translations` output. Globals/trait_impls of non-template entries already differ between the arms today (`annotate` joins only `functions`); out of scope.
- T3 `apply` on a `functions.json` without template records appends the local template record (flag, `def_id`, `def_id_is_fun_decl`) once; a file that already has it is not duplicated, and the existing record gets `is_template_name`, not `is_local_template`.

translate:
- T4 resolver: bare manifest name → prefixed atom (SPQR direction); prefixed manifest name → root atom (#61 direction); exact atom wins when both exist; no atom under either name → unchanged; a non-crate prefix (`core.x` with crate `spqr`, atom `x`) is never stripped; a non-template record is never rewritten; `crate_name: None` → unchanged.
- T4b an atom-less local template record in the same file as a regular record (same base name, larger overlap) does not block the regular record's mapping (strategies 2 and 3).
- T4b' a resolved local template record whose Rust atom is missing does not displace a correct mapping of another same-file atom.
- T4d two templates resolving to one name: the exact-hit one is kept, regardless of input order; a function vs global template tie keeps the `def_id_is_fun_decl` one.
- T4g strategy 1: a template record (sorting first, own Rust atom missing) and a regular record with colliding normalized `rust_name`, one Rust atom, no `def_id` gate: the regular record keeps its mapping.
- T4h strategy 2: a template with an atom in the same file and base name as a regular record does not make the regular match ambiguous.
- T4i an unsorted Override `functions.json` plus a manifest with a local template gives the same non-template mappings as without the template.
- T4j hyphenated `crate` (`my-crate`) resolves `my_crate.` names in both directions.
- T4k when both names have atoms and the exact one is the `External_Template.lean` stand-in, the alternative wins.
- T4l strategy 1: a template whose Rust atom is missing does not bind a colliding regular Rust atom in another file, even when that atom is unmatched.
- T4m a global template record never joins strategy 0.
- T4e SymCRust `box_default`: the instance fixture plus an atom-less local template record for `…::{impl … for …Key}::box_default` still binds the instance; with an atom for the axiom, the method maps to the axiom.
- T4f P14: reversed manifest arrays give identical records and mappings.
- T4c a local template resolving to the `lean_name` of an existing record is dropped; the existing record gains its `def_id` when it had none.
- T5 `generate_translations`: SPQR fixture maps `hkdf_to_slice` by `charon-def-id` to `probe:spqr.kdf.hkdf_to_slice` and `hkdf_to_vec` still maps; #61 direction maps; a template record with no atom gives no mapping; a non-local `core.*` template record no longer reaches the strategies (`records_from_manifest` drops it) — regression for review point 2.

extract:
- T5b `run_translate_only` with a `functions.json` that lacks the template entry plus the `translation.json` maps `hkdf_to_slice` (Override arm + resolver).

extract merge (mappings from `generate_translations`, then `run_extract_with_translations`):
- T6 Rust atom gets `translation-name`, `verification-status: trusted`, `status-origin: translation`, `translation-path` = the Lean atom's `code-path` (`SrcTranslated/FunsExternal.lean`); Lean atom gets `mapped-from`.
- T7 same with a clean `…_spec.mvcgen_spec` primary spec present: status stays `trusted`.

functions.json / listfuns:
- T8 `write_functions_json` omits local template records.
- T9b enriched listfuns (with `atoms_path`) omits it too.
- T9 basic listfuns on a project with a `translation.json` containing a local template entry writes a `functions.json` without it.

## Expected effect on SPQR

Planned: on SPQR only `spqr::kdf::hkdf_to_slice` gains `maps-to`/`translation-*`/`trusted`; derived statuses of its callers may change (a copied `trusted` is a blocker seed). Measured: on the May extracts only `hkdf_to_slice` changed; on the October extracts a second local template, `potentially_fix_state_incorrectly_encoded_by_libcrux_issue_1275`, also maps (see the checklist below). SymCRust has no `translation.json`, so nothing changes there. curve25519-dalek has no local template entries.

## Progress

- [x] plan reviewed: round 4 gave no high/medium findings (8 low, all accepted). (round 3: 2 medium, 5 low; accepted except the minor-version suggestion: the task asks for a patch bump for this bug fix. round 1: 3 medium, 5 low, all accepted; round 2: 1 medium, 5 low, accepted except L1's broader `annotate` change, out of scope; T3a narrowed instead)
- [x] implementation reviewed by a fresh subagent: missing T4b'/T4f/T4i tests, a weak strategy-3 test, and four low code findings fixed; added a strategy-3 containment guard for template records (a template whose Rust atom is missing binds only an atom its range contains).
- [x] implementation (planned tests added; mutation checks on the strategy-1 file guard, the display-name index exclusion and the atom-less drop each fail tests)
- [x] docs, version (0.22.2)
- [x] Copilot review on the PR: (1) strategy 1 also requires the template's range to contain the atom (`normalize_rust_name` collapses owned/borrowed impls in one file); (2) a record an Override file already lists under a template's manifest name gets name resolution (`is_template_name`). Both with tests. Later rounds (Codex 1-5, Copilot): a renamed listed record gets the template rules and goes after the regular records; a listed record whose resolved name is held merges into the holder; template records bind only an atom of their own crate.
- [x] issue Beneficial-AI-Foundation/probe-aeneas#82, draft PR Beneficial-AI-Foundation/probe-aeneas#83
- [x] Ralph Loop audit (ambiguity, code quality, test quality), two rounds, on the final code; findings in this repo fixed. Deferred to the probe repo: the hub glossary entries on `functions.json` and strategy order, and ADR-003 repeating the strategy-3 rules.
- [x] A/B on SPQR's October 2026 extracts (probe-rust 0.12.0 with `charon-def-id`, probe-lean 0.19.0): `hkdf_to_slice` maps by `charon-def-id` to the prefixed `spqr.kdf.hkdf_to_slice`, and `potentially_fix_state_incorrectly_encoded_by_libcrux_issue_1275` maps to its root-level def; no other atom changes.
