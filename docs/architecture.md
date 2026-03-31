# probe-aeneas Architecture

## probe-aeneas as an instantiation of probe merge

`probe merge` is a generic merge engine in the
[probe](https://github.com/Beneficial-AI-Foundation/probe) crate. It
combines multiple atom maps into one and optionally adds cross-language
dependency edges using a translations mapping. The engine is
language-agnostic: as long as it receives a bidirectional mapping
between code-names in language S and code-names in language T, it can
merge heterogeneous atom files and wire up cross-language call edges.

probe-aeneas is an **instantiation** of this generic pattern for the
specific case of Rust and Lean projects transpiled by
[Aeneas](https://github.com/AeneasVerif/aeneas). It generates the
translations that `merge_atom_maps` needs, calls the generic merge, and
then layers on Aeneas-specific metadata that the generic engine does not
know about.

## The extract pipeline

The `extract` command runs a three-phase pipeline. The first and third
phases are Aeneas-specific; the second phase delegates to the generic
merge engine from the probe crate.

```
                     Aeneas-specific              Generic               Aeneas-specific
                 ┌─────────────────────┐  ┌────────────────────┐  ┌─────────────────────────┐
  Rust atoms ──▶ │                     │  │                    │  │                         │
  Lean atoms ──▶ │  1. Generate        │─▶│  2. merge_atom_maps│─▶│  3. Enrich with         │──▶ Output
  functions  ──▶ │     translations    │  │     (probe crate)  │  │     Aeneas metadata     │
  .json          │                     │  │                    │  │                         │
                 └─────────────────────┘  └────────────────────┘  └─────────────────────────┘
```

### Phase 1: Generate translations (Aeneas-specific)

Uses `functions.json` (produced by `lake exe listfuns`) as the bridge
between Rust and Lean namespaces. Three matching strategies, applied in
priority order, produce `TranslationMapping` entries:

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

The output of this phase is a bidirectional map
`(from_to: HashMap, to_from: HashMap)` -- the format that
`merge_atom_maps` accepts.

Implementation: `src/translate.rs` (matching logic),
`src/extract.rs::run_translate` (orchestration).

### Phase 2: Merge with cross-language edges (generic)

Calls `probe::commands::merge::merge_atom_maps` from the probe crate
with `vec![rust_atoms, lean_atoms]` and the translations from phase 1.

The generic engine performs three operations:

- **Combine**: unions the two atom maps. Stubs in the first map are
  replaced by real atoms from the second; new atoms are added;
  real-vs-real conflicts keep the first (but in practice the Rust and
  Lean namespaces are disjoint, so conflicts do not arise).
- **Cross-language edges**: for each atom, iterates its existing
  dependencies and, if any dependency has a known translation, inserts
  the translated code-name as an additional dependency. This creates
  edges wherever a call site crosses the Rust/Lean boundary through
  a translated function.
- **Stub accounting**: counts stubs remaining, entries added, and
  cross-language edges applied.

This step is identical to what `probe merge --translations` does from
the command line. probe-aeneas simply calls the library function
directly.

Implementation: `src/extract.rs::run_extract_with_translations` calls
`merge_atom_files` which handles file loading, provenance flattening,
and the merge in one step. The function itself lives in
`probe/src/commands/merge.rs`.

### Phase 3: Enrich with Aeneas metadata (Aeneas-specific)

After the generic merge, probe-aeneas makes two enrichment passes over
the merged atom map:

1. **Translation metadata**: for each Rust atom that has a Lean
   translation, sets `translation-name`, `translation-path`, and
   `translation-text` from the corresponding Lean atom.

2. **`is-disabled` flag**: for every Rust atom, `is-disabled` is `false`
   when its `rust-qualified-name` appears as a `rust_name` in
   `functions.json` or the atom already has a `translation-name` from
   step 1. Otherwise it is out of scope (`is-disabled: true`).

Implementation: `src/extract.rs::enrich_with_aeneas_metadata`.

## Why probe-aeneas uses its own schema

The output carries `"schema": "probe-aeneas/extract"` rather than the
generic `"probe/merged-atoms"` used by `probe merge`. This is because
the enrichment in phase 3 makes the output semantically richer than a
plain merge: it contains `translation-*` fields and `is-disabled` that
the generic merge engine does not produce. The distinct schema name lets
downstream consumers distinguish the two and apply appropriate
validation or display logic.

## Shared types from the probe crate

probe-aeneas depends on the `probe` crate for:

| Import | Source | Role |
|--------|--------|------|
| `merge_atom_files` | `probe::commands::merge` | Load + merge atom files with provenance flattening (phase 2) |
| `Atom` | `probe::types` | Core atom representation |
| `TranslationMapping` | `probe::types` | Translation entry type |
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

1. Produce `TranslationMapping`s by whatever means the bridge provides.
2. Call `merge_atom_maps` with the two atom files and the translations.
3. Add domain-specific metadata to the merged output.

Future tools bridging other language pairs (e.g., Rust + Dafny,
Rust + Verus specs) could reuse the same generic merge step.
