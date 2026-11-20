# Opaque `External_Template` functions are never matched

Status: fixed in 0.22.2 (PR #83). This is the analysis before the fix. The rules that shipped differ from the fix proposed below; see the "Opaque template functions" rules in [USAGE.md](USAGE.md#translation-strategies).

Problem: `spqr::kdf::hkdf_to_slice` (SPQR, probe-aeneas 0.22.1) gets no `maps-to`, `translation-name` or `verification-status`. Its Rust atom has `charon-def-id: 43`. Its Lean atom `probe:spqr.kdf.hkdf_to_slice` is `kind: opaque`, `code-path: SrcTranslated/FunsExternal.lean`, `verification-status: trusted`, `trusted-reason: external`, `primary-spec: probe:spqr.kdf.hkdf_to_slice_spec.mvcgen_spec`. The neighbour `hkdf_to_vec` (def_id 42, in `Funs.lean`) maps correctly by `charon-def-id`.

Manifest entry (Aeneas nightly-2026.11.03-ab34bd2, charon 0.1.304, top-level `"crate": "spqr"`):
`{ "def_id": 43, "lean_name": "kdf.hkdf_to_slice", "lean_file": "SrcTranslated/FunsExternal_Template.lean", "rust_name": "spqr::kdf::hkdf_to_slice", "is_local": true, "is_opaque": true, "source": {"file": "src/kdf.rs", "begin_line": 14, "end_line": 18} }`
Regular entry for comparison: `{ "def_id": 42, "lean_name": "spqr.kdf.hkdf_to_vec", "lean_file": "SrcTranslated/Funs.lean" }`.

Two causes, both needed for the miss:

Cause 1. `records_from_manifest` in `src/translation_manifest.rs` drops every entry whose `lean_file` ends in `External_Template.lean` (`is_external_template`). Its assumption was that these are library stand-ins with no Lean atom. That is false here: the project copies the template to `FunsExternal.lean`, completes it, and probe-lean extracts the atom.

Cause 2. Even with the record kept, the name does not match. Aeneas writes template-entry names WITHOUT the crate prefix (`kdf.hkdf_to_slice`) but regular entries WITH it (`spqr.kdf.hkdf_to_vec`). The project wraps `hkdf_to_slice` in `namespace spqr.kdf` in `FunsExternal.lean`, so its atom key is `probe:spqr.kdf.hkdf_to_slice`. All four strategies in `src/translate.rs` look up `format!("probe:{lean_name}")` exactly, so the lookup misses. Of SPQR's 59 template entries, 56 resolve under the bare name, `kdf.hkdf_to_slice` only with the `spqr.` prefix, and 2 (`core.slice.Slice.chunks`, `core.slice.iter.Chunks.Insts.CoreIterTraitsIteratorIteratorSharedASlice.next`) under neither. Issue #61 is the opposite direction (older Aeneas put the prefix on template names while the file declared them at root, issue AeneasVerif/aeneas#1248), so the resolver must handle both directions.

Facts already checked in the code: strategies 0-2 skip a record whose Lean atom is missing (`lean_data.contains_key`). Strategy 3 (`file+line-overlap`) checks only the winning record, so an unresolvable record that wins the overlap hides a second-best record that has an atom (a lost match, not a wrong one). This does NOT make keeping template records safe: most template entries (the `core.*` stand-ins) do resolve to a Lean atom, and see the review below. `resolve_verification_status` in `src/extract.rs` returns a `trusted`/`failed` Lean status before it looks for a spec, so the clean `mvcgen_spec` theorem cannot override `trusted`. `translation-path` is copied from the Lean atom's `code-path`, so it will be `FunsExternal.lean`, not the template. Only 3 SPQR template entries are `is_local` (`kdf.hkdf_to_slice`, `encode_state_pb`, `decode_state_pb`); the last two have no Rust atom in the current extract, so on SPQR only `hkdf_to_slice` is expected to change.

Side effect to handle: `listfuns` (`src/listfuns.rs`) also builds its output from `records_from_manifest` via `function_source::resolve`. Removing the filter unconditionally adds the ~59 template entries (mostly `core.*` library stand-ins) to generated `functions.json`. Decide whether listfuns keeps excluding them or marks them, and test that decision. Also consider whether extra template records can make a previously unambiguous `file+display-name` match ambiguous.

Proposed fix: (a) keep template records for the extract join; (b) add one Lean-key resolver used by all four strategies: try `probe:{lean_name}`; on a miss, try adding the manifest `crate` prefix, or removing it when `lean_name` starts with `<crate>.`; accept only a key that exists in the Lean atoms; never strip an arbitrary first segment; if both forms exist, prefer the exact name and warn. This needs the manifest `crate` field plumbed through (`TranslationManifest` and `function_source::ResolvedRecords`, which already carry `charon_version`).

Required tests: an opaque template entry with an atom under the prefixed name maps by `charon-def-id`, and the Rust atom gets `verification-status: trusted`, `status-origin: translation`, `translation-path` = the atom's `code-path`, and the Lean atom gets `mapped-from`; same with a clean primary spec, status stays `trusted`; the #61 direction (prefixed manifest name, root atom) also maps; a template entry with no Lean atom gives no mapping; a non-crate prefix is never stripped; listfuns output behaves as decided.

Out of scope (mention as follow-ups in the issue, do not implement): copying `trusted-reason` and the axiom name to the Rust atom; distinguishing an opaque function with an axiom spec from one with no spec; why `is-relevant` differs between the Rust and Lean atoms.

## Review outcome (adversarial review, round 1)

Codex was not available in this environment (no `codex` binary, no `OPENROUTER_API_KEY`); a fresh subagent reviewed the doc instead. Accepted corrections:

1. **Repeat runs use the Override arm.** The positional `extract` writes `<project>/functions.json` (`extract.rs` Manifest arm) and the next run reuses it as `--functions` (`resolve_project`). `annotate` only overlays existing records, so a template record missing from that file never comes back. The fix must add the manifest's template records on the Override arm too.
2. **Keeping all template records is unsafe.** `normalize_source_path` cuts everything before the first `/src/`, so `/rustc/<hash>/library/core/src/slice/mod.rs` becomes `src/slice/mod.rs`. A `core.*` stand-in that resolves to a Lean atom can then make a `file+display-name` key ambiguous or win a `file+line-overlap` and bind a project atom to a `core.*` def. Keep only **local** template entries: `is_local == true`, or, when the manifest has no `is_local` (older Aeneas, #61), `rust_name` starts with `<crate>::`.
3. **Strategy 3 checks the atom only on the winner** (see above). Resolution must happen before any strategy sees the record.
4. **Plumbing is wider.** `crate` is not parsed today (needs `#[serde(rename = "crate")]`). The Override arm builds `ResolvedRecords` from `ManifestOverlay`, so the crate and the template records also go through `apply`. Without a manifest the crate is unknown and nothing changes.
5. **The resolver must be limited to template records.** Applied to every record, it could bind a regular `spqr.X` with a missing atom to an unrelated root-level `X`. Records need an in-memory `is_external_template` flag, restored by `annotate` on the Override arm. Simplest implementation: rewrite the template record's `lean_name` to the resolved atom name once at the start of `generate_translations`, so the four strategies, the `def_id` map and the indexes all see one name.
6. **`functions.json` is written by `extract` as well as `listfuns`**, and enriched listfuns looks up `probe:{lean_name}` exactly. Decision: template records stay out of every written `functions.json` (no schema has a field to mark them, and enriched listfuns would list them with no atom); they exist only in memory for the extract/translate join, re-added from the manifest on every run (point 1).
8. **Template `globals`/`trait_impls`** follow the same locality rule; they never join by `def_id` (`def_id_is_fun_decl` false).
9. **Callers' statuses can change**: a copied `trusted` is a blocker seed, so callers of `hkdf_to_slice` may get a different derived status. Expected; stated in the PR.

Rejected or deferred:

- 7 (template names in the aux-def set are not resolved): pre-existing, separate from function matching; listed as a follow-up.
- 10 (check that `crate` is really the Lean namespace prefix): the resolver accepts only a key that exists in the Lean atoms and only for local template records, so a wrong prefix gives no match, never a wrong one. Not worth the extra rule.
