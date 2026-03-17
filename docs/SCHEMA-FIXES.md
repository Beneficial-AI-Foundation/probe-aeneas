# SCHEMA.md Fixes Plan

Findings from comparing `docs/SCHEMA.md` against the implementation in
`src/` (probe-aeneas) and the shared `probe` crate (`probe::types`,
`probe::commands::merge`).

---

## 1. Cross-language edges: doc vs code behavior (significant)

**Location:** SCHEMA.md lines 222-225 ("Cross-Language Edges" section) and
the Rust/Lean atom examples (lines 106-153).

**What the doc says:**

> the Lean code-name is added to the Rust atom's `dependencies` array and
> vice versa. This creates bidirectional cross-language edges in the merged
> graph.

This implies **direct bidirectional edges** between each translated pair
(Rust atom R ↔ Lean atom L).

**What the code does:**

`merge_atom_maps` in `probe/src/commands/merge.rs` (lines 130-161) adds
**transitive** cross-language edges. For each atom it iterates through
*existing* dependencies and, if any dependency has a translation, appends
the translated code-name as a new dependency:

- Rust atom A depends on Rust atom B; B translates to Lean B' →
  A gets B' as a dependency.
- Lean atom X depends on Lean atom Y; Y translates to Rust Y' →
  X gets Y' as a dependency.

It does **not** create a direct edge between B and B' themselves.

**Why the example is misleading:**

The Rust atom example shows:

```json
"probe:curve25519-dalek/4.1.3/scalar/Scalar#from_bytes_mod_order()": {
  "dependencies": [
    "probe:curve25519-dalek/4.1.3/scalar/helper()",
    "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order"
  ],
  ...
}
```

Here the Rust `from_bytes_mod_order` lists its own Lean translation in
`dependencies`. The merge code can only produce this if the atom already
had a pre-existing dependency whose translation is the Lean
`from_bytes_mod_order`—which would require a self-loop. The example
doesn't reflect what the code can actually produce.

**Fix:**

- Rewrite the "Cross-Language Edges" section to describe transitive
  expansion (callers gain cross-language edges to their callees'
  translations, not to their own).
- Update both the Rust and Lean atom examples so the dependencies shown
  are achievable through the actual merge logic.

---

## 2. Lean atom example: wrong dependency ordering (DONE)

**Status:** Applied. Dependencies in the Lean atom example now appear in
correct lexicographic order (`probe:curve25519-dalek/...` before
`probe:curve25519_dalek...`).

---

## 3. `TranslationMapping.method` optionality not documented (DONE)

**Status:** Applied. SCHEMA.md now documents `method` as `string or
absent` with note: "Always present in probe-aeneas output; may be absent
in other producers".

---

## 4. Adopt tool-specific schema name `probe-aeneas/extract` (DONE)

**Status:** Applied. Schema is now `"probe-aeneas/extract"`.

**Rationale:** probe-aeneas's `extract` command is an instantiation of
`probe merge` (it calls `merge_atom_maps` for the generic
combine + cross-language-edge step), but the output is semantically
richer than a plain merge: it includes Aeneas-specific `translation-*`
metadata and `is-disabled` flags added in a post-merge enrichment phase.
The distinct schema name lets downstream consumers distinguish the two
and apply appropriate validation.

See `docs/architecture.md` for the full description of how probe-aeneas
relates to probe merge.

---

## 5. `specs` field description slightly misleading (DONE)

**Status:** Applied. SCHEMA.md now reads "may be present when
`specified` is true".
