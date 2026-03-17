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

## 2. Lean atom example: wrong dependency ordering (minor)

**Location:** SCHEMA.md lines 133-136 (Lean atom example).

**What the doc shows:**

```json
"dependencies": [
  "probe:curve25519_dalek.scalar.Scalar.reduce",
  "probe:curve25519-dalek/4.1.3/scalar/Scalar#from_bytes_mod_order()"
],
```

**What the code produces:**

`dependencies` is `BTreeSet<String>`, which serializes in lexicographic
order. Since `-` (ASCII 45) < `_` (ASCII 95), the name starting with
`probe:curve25519-dalek/...` must come **before**
`probe:curve25519_dalek...`.

**Fix:**

Swap the two entries so they appear in correct sorted order.

---

## 3. `TranslationMapping.method` optionality not documented (minor)

**Location:** SCHEMA.md line 302 (TranslationMapping table).

**What the doc says:**

| Field | Type |
|-------|------|
| `method` | string |

**What the shared type declares (`probe/src/types.rs` line 287-288):**

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub method: Option<String>,
```

probe-aeneas always sets `method` to `Some(...)`, so output always
includes the field. But the shared type permits it to be absent. Other
consumers of `TranslationMapping` may omit it.

**Fix:**

Change type to `string or null` (or add a note: "Always present in
probe-aeneas output; may be absent in other producers").

---

## 4. Adopt tool-specific schema name `probe-aeneas/extract` (code + doc change)

**Location:** `src/extract.rs` line 222; SCHEMA.md section 1.

**Current state:**

probe-aeneas produces `"schema": "probe/merged-atoms"` — the same generic
schema identifier used by `probe merge`:

| Producer | `tool.name` | `tool.command` | `schema` |
|----------|-------------|----------------|----------|
| `probe-aeneas extract` | `"probe-aeneas"` | `"extract"` | `"probe/merged-atoms"` |
| `probe merge` | `"probe"` | `"merge"` | `"probe/merged-atoms"` |

**Why this should change:**

probe-aeneas `extract` has diverged from `probe merge`. It is no longer
just a merge — it performs translation matching, adds cross-language
dependency edges, and enriches Rust atoms with `translation-*` metadata.
The output is semantically richer than what `probe merge` produces.

Other tools in the ecosystem already use tool-specific schema names:

| Tool | Schema |
|------|--------|
| probe-rust | `"probe-rust/extract"` |
| probe-lean | `"probe-lean/extract"` |

**Decision:** rename to `"probe-aeneas/extract"`.

**Changes required:**

1. `src/extract.rs` line 222: change `"probe/merged-atoms"` →
   `"probe-aeneas/extract"`.
2. `docs/SCHEMA.md`: update the schema identifier, section heading,
   and all references.
3. `probe` crate — `detect_category` in `probe/src/types.rs`
   (lines 148-162): add `"probe-aeneas/extract"` as a recognized
   atoms-category schema so that downstream `probe merge` and other
   consumers can still load these files.
4. Update CLAUDE.md and CHANGELOG.md references.

---

## 5. `specs` field description slightly misleading (minor)

**Location:** SCHEMA.md line 191 (Lean-specific fields table).

**What the doc says:**

> `specs` | array of strings | no | Code-names of spec theorems (present
> when `specified` is true)

**What actual probe-lean output shows:**

Atoms exist with `"specified": true` but no `specs` field at all (e.g.
`probe:tacticExpand_With_` in
`examples/lean_Curve25519Dalek_0.1.0.json` line 13). This is a
passthrough from probe-lean, but since SCHEMA.md documents these fields
it should be accurate.

**Fix:**

Change parenthetical to "may be present when `specified` is true" or
drop the conditional note entirely, since the "no" in the Required column
already communicates optionality.
