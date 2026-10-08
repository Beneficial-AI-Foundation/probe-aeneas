# probe-aeneas Architecture

## probe-aeneas as an instantiation of probe merge

`probe merge` is a generic merge engine in the
[probe](https://github.com/Beneficial-AI-Foundation/probe) crate. It
combines multiple atom maps into one and, given mapping records, attaches
`maps-to`/`mapped-from` correspondence records that link code-names across
languages (hub ADR-006). It never adds dependency edges for a mapping. The
engine is language-agnostic: as long as it receives mapping records
between code-names in language S and code-names in language T, it can
merge heterogeneous atom files and link them.

probe-aeneas is an **instantiation** of this generic pattern for the
specific case of Rust and Lean projects transpiled by
[Aeneas](https://github.com/AeneasVerif/aeneas). It generates the
translations the merge needs, calls the generic merge, layers on
Aeneas-specific metadata that the generic engine does not know about, and
then runs the hub's enrichment once.

## The extract pipeline

The `extract` command runs a four-phase pipeline. Phases 1 and 3 are
Aeneas-specific; phases 2 and 4 delegate to the probe crate.

Before phase 1, `check_input_authority` checks both inputs: the hub's ADR-006
version gate, and the provenance guard (every provenance entry of the Rust
input has schema `probe-rust/extract`, of the Lean input
`probe-lean/extract`). Pre-generated inputs are checked before any extractor
runs, extracted inputs right after extraction.

```
                  Aeneas-specific          Generic                Aeneas-specific        Generic
               ┌──────────────────┐  ┌──────────────────────┐  ┌──────────────────┐  ┌──────────────┐
 Rust atoms ──▶│                  │  │                      │  │                  │  │              │
 Lean atoms ──▶│ 1. Generate      │─▶│ 2. merge_atom_files_ │─▶│ 3. Aeneas        │─▶│ 4. Enrich    │──▶ Output
 functions  ──▶│    translations  │  │    raw (probe crate) │  │    metadata      │  │    (once)    │
 .json         │                  │  │                      │  │                  │  │              │
               └──────────────────┘  └──────────────────────┘  └──────────────────┘  └──────────────┘
```

### Phase 1: Generate translations (Aeneas-specific)

Uses `functions.json` (produced by `lake exe listfuns`) as the bridge
between Rust and Lean namespaces. Matching strategies, applied in
priority order, produce `Mapping` entries:

0. **`charon-def-id`** -- integer join on the charon `FunDeclId`: probe-rust's
   `charon-def-id` atom field equals Aeneas's `translation.json` `def_id`, so
   equal ids bind the Rust atom to the family's primary (non-loop) Lean def with
   no name normalization. Confidence: `exact`. Runs first, but is
   **provenance-gated** -- it fires only when the atom's `charon-version` matches
   the manifest's `charon_version` (same charon version, best-effort provenance);
   otherwise mismatched ids would point at different functions. Version equality
   is not proof of an identical run -- two runs of the same version with
   different cargo flags/sources can still diverge; a charon commit hash or LLBC
   digest would be the durable fix. A no-op for atoms that do not carry a
   `charon-def-id`, which then fall through to the name/location strategies.
1. **`rust-qualified-name`** -- exact match via Charon-derived qualified
   names joined with `functions.json` `rust_name` entries. Confidence:
   `exact`.
2. **`file+display-name`** -- same source file path + matching base
   method name (unambiguous only). Confidence: `file-and-name`.
3. **`file+line-overlap`** -- same source file + overlapping line
   ranges (best overlap wins). Confidence: `file-and-lines`.

Each Rust function maps to at most one Lean definition (1-to-1). Once a
Rust or Lean atom is claimed by an earlier strategy it is excluded from
later ones.

Aeneas expands one looping Rust function into a top-level def plus `_loop` /
`_loop.body` helpers that share its `rust_name`; only the top-level def is a
valid mapping target. When Aeneas's `translation.json` is available it is
overlaid onto the loaded `functions.json` records (joined by exact `lean_name`,
via `src/translation_manifest.rs`): an entry is a loop helper iff its manifest
record carries a `loop` field. This authoritative classification replaces the
name-suffix heuristic (`_loop`/`_body`/`.body`), which remains the fallback when
the manifest is absent. See the manifest overlay note in
[USAGE.md](USAGE.md#translation-strategies).

The output of this phase is the list of `Mapping` records, each with its
`confidence` and `method`. The records stay authoritative for the rest of
the pipeline: endpoints are normalized by the hub's P8 rule (strip trailing
`.`), an empty `method` is dropped, and the `from → [to]` lookup map that
phase 3 uses is derived from them. Confidence is never reconstructed from
the lookup map.

Implementation: `src/translate.rs` (matching logic),
`src/translation_manifest.rs` (manifest overlay),
`src/extract.rs::run_translate` (orchestration).

### Phase 2: Merge with correspondence records (generic)

Calls `probe::commands::merge::merge_atom_files_raw` from the probe crate
with the Rust and Lean atom files and the mapping records from phase 1.

The generic engine performs these operations:

- **Authority validation**: rejects projections and pre-contract inputs
  (hub ADR-006 version gate: probe-lean output must come from >= 0.16.0)
  and malformed `status-origin` markers.
- **Combine**: normalizes keys per input, then unions the two atom maps.
  Stubs in the first map are replaced by real atoms from the second; new
  atoms are added; real-vs-real conflicts keep the first (but in practice
  the Rust and Lean namespaces are disjoint, so conflicts do not arise).
- **Correspondence records**: for each mapping, attaches a `maps-to`
  record on the Rust atom and a `mapped-from` record on the Lean atom,
  each carrying the mapping's `confidence` and `method`. `dependencies`
  is never modified.
- **Stub accounting**: counts stubs remaining, entries added, and
  records attached.

This is the raw staging variant of what `probe merge --mappings` does:
it skips the enrichment recomputation, because phase 3 writes statuses
and enrichment must run after that, exactly once (phase 4).

Implementation: `src/extract.rs::run_extract_with_translations` calls
`merge_atom_files_raw`, which handles file loading, validation,
provenance flattening and the merge in one step. The function itself
lives in `probe/src/commands/merge.rs`.

### Phase 3: Add Aeneas metadata (Aeneas-specific)

After the generic merge, probe-aeneas makes these passes over the merged
atom map:

1. **Translation metadata**: for each Rust atom that has a Lean
   translation, sets `translation-name`, `translation-path`, and
   `translation-text` from the corresponding Lean atom.

2. **Verification status**: for each translated Rust atom, derives
   `verification-status` from the Lean definition's primary spec
   theorem (via `primary-spec` extension or `_spec` naming convention).
   If the Lean def is `"trusted"` or `"failed"`, that status is
   copied directly. Otherwise, the spec's status is used (a
   `"transitively-verified"` spec is copied as `"verified"`, and a spec
   without a status gives `"unverified"`). If no spec is found, the atom
   gets no status and no marker (#73). Every copied status is marked
   `status-origin: "translation"` (hub ADR-006 Decision 2): it is
   imported evidence, so phase 4 never promotes the atom, or a caller that
   reaches it along a path without a trusted boundary, to
   `"transitively-verified"`.

3. **`untracked` flag**: every Rust atom is tracked backlog by default
   (`untracked: false`); membership in `functions.json` does **not** decide
   scope. An atom with a `verification-status`, or with a matched
   translation (`translation-name`) that does not carry `@[out_of_scope]`,
   is always tracked: only `@[out_of_scope]` can untrack a matched
   translation. The input guard in `check_input_authority` makes sure that
   these fields come from this run. An atom flips to
   `untracked: true` only when this in-scope rule does not apply **and**
   is genuinely out of the Aeneas verification build — a foreign declaration
   (probe-rust's `is-foreign`: an extern-block member with no Rust body), a
   bodyless trait method signature with no matched translation (probe-rust's
   `trait-required`: no default body, so the `impl`s carry the obligations), in
   a file no lib/bin `mod` chain reaches (probe-rust's `is-unmounted`),
   cfg-inactive in the resolved feature set (the complete `cfg` predicate,
   with `file-cfg` refining the reason), its Lean translation carries
   `@[out_of_scope]`, it is a non-library target
   (`build.rs`/`tests`/`examples`/`benches`), or it matches a curated
   `out-of-scope` glob in `aeneas.json` (KB P24/P25). The cause is emitted
   as `untracked-reason`.

Implementation: `src/extract.rs::enrich_with_aeneas_metadata`. Phase 3
also prefixes Rust `code-path`s with the crate directory
(`prefix_rust_code_paths`) and sets the Lean atom flags
(`enrich::enrich_lean_atom_flags`).

### Phase 4: Enrich verification status (generic)

Calls `probe::commands::propagate::enrich_verification_status` once over
the whole merged graph (hub P23). Translation-marked atoms and probe-lean's
`kernel-taint` atoms are blocker seeds. `--skip-enrich` skips this phase,
and with it all enrichment in the pipeline.

## Why probe-aeneas uses its own schema

The output carries `"schema": "probe-aeneas/extract"` rather than the
generic `"probe/merged-atoms"` used by `probe merge`. This is because
the Aeneas metadata in phase 3 makes the output semantically richer than a
plain merge: it contains `translation-*` fields and `untracked` that
the generic merge engine does not produce. The distinct schema name lets
downstream consumers distinguish the two and apply appropriate
validation or display logic.

## Shared types from the probe crate

probe-aeneas depends on the `probe` crate for:

| Import | Source | Role |
|--------|--------|------|
| `merge_atom_files_raw` | `probe::commands::merge` | Load, validate and merge atom files with provenance flattening, no enrichment (phase 2) |
| `enrich_verification_status` | `probe::commands::propagate` | The single enrichment pass (phase 4) |
| `Atom` | `probe::types` | Core atom representation |
| `Mapping` | `probe::types` | Cross-language mapping record (`from`, `to`, `confidence`, `method`) |
| `endpoint_lookup_maps` | `probe::types` | Derives the endpoint lookup maps from mapping records (phase 3) |
| `MergedAtomEnvelope` | `probe::types` | Output envelope (multi-input variant) |
| `InputProvenance` | `probe::types` | Per-input provenance metadata |
| `Tool` | `probe::types` | Tool metadata in the envelope |

These are shared infrastructure types used across the probe ecosystem
(probe-rust, probe-lean, probe-verus, probe merge). probe-aeneas does
not re-define them.

## Generalizability

The pattern -- generate translations, merge, enrich -- is not specific
to Aeneas. Any cross-language bridge that can produce a bidirectional
code-name mapping can follow the same architecture:

1. Produce `Mapping` records by whatever means the bridge provides.
2. Call `merge_atom_files_raw` with the two atom files and the records.
3. Add domain-specific metadata to the merged output, marking any status
   copied across languages with `status-origin: "translation"`.
4. Run `enrich_verification_status` once.

Future tools bridging other language pairs (e.g., Rust + Dafny,
Rust + Verus specs) could reuse the same generic merge step.
