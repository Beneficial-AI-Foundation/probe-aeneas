# probe-aeneas Data Schemas

Version: 2.0
Date: 2026-03-13

This document specifies the JSON output formats produced by each probe-aeneas
subcommand. It complements the language-agnostic
[envelope-rationale.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md)
which defines the envelope wrapper; this document defines what goes **inside**
the `data` field and the output of non-enveloped commands.

---

## Common: Schema 2.0 Envelope

Both `merge` and `translate` commands wrap their output in a standardized
metadata envelope. The envelope fields vary slightly between commands (see
sections below), but share this structure:

| Field | Type | Description |
|-------|------|-------------|
| `schema` | string | Data type identifier (e.g. `"probe/merged-atoms"`) |
| `schema-version` | string | Interchange spec version (`"2.0"`) |
| `tool.name` | string | Always `"probe-aeneas"` |
| `tool.version` | string | Semver version of the probe-aeneas binary |
| `tool.command` | string | Subcommand that produced the file |
| `timestamp` | string | ISO 8601 timestamp of when the analysis ran |

---

## 1. `probe/merged-atoms` -- Merged Call Graph

**Produced by:** `merge`
**Envelope schema:** `"probe/merged-atoms"`

### Envelope Shape

```json
{
  "schema": "probe/merged-atoms",
  "schema-version": "2.0",
  "tool": {
    "name": "probe-aeneas",
    "version": "0.1.0",
    "command": "merge"
  },
  "inputs": [
    {
      "schema": "probe-rust/atoms",
      "package": "curve25519-dalek",
      "package-version": "4.1.3",
      "language": "rust"
    },
    {
      "schema": "probe-lean/extract",
      "package": "Curve25519Dalek",
      "package-version": "0.1.0",
      "language": "lean"
    }
  ],
  "timestamp": "2026-03-13T12:00:00Z",
  "data": { ... }
}
```

### Envelope Fields (merge-specific)

| Field | Type | Description |
|-------|------|-------------|
| `inputs` | array of InputProvenance | Metadata about each input atom file |

### InputProvenance

| Field | Type | Description |
|-------|------|-------------|
| `schema` | string | Schema of the input file (e.g. `"probe-rust/atoms"`, `"probe-lean/extract"`) |
| `package` | string | Package/crate name from the input |
| `package-version` | string | Package version from the input |
| `language` | string | Language of the input (`"rust"` or `"lean"`) |

### Data Shape

`data` is an object keyed by code-name. Each value is an atom from one of the
input files, potentially enriched with cross-language dependency edges. The
atom format follows the shared `probe` atom schema:

```json
{
  "probe:curve25519-dalek/4.1.3/scalar/Scalar#from_bytes_mod_order()": {
    "display-name": "Scalar::from_bytes_mod_order",
    "dependencies": [
      "probe:curve25519-dalek/4.1.3/scalar/helper()",
      "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order"
    ],
    "code-module": "scalar",
    "code-path": "curve25519-dalek/src/scalar.rs",
    "code-text": { "lines-start": 142, "lines-end": 167 },
    "kind": "exec",
    "language": "rust",
    "rust-qualified-name": "curve25519_dalek::scalar::Scalar::from_bytes_mod_order"
  }
}
```

### Atom Field Reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `display-name` | string | yes | Human-readable name |
| `dependencies` | array of strings | yes | Sorted code-names of callees, including cross-language edges added by merge |
| `code-module` | string | yes | Module path |
| `code-path` | string | yes | Relative source file path (empty for external stubs) |
| `code-text` | object | yes | `{"lines-start": N, "lines-end": M}` (1-based, inclusive) |
| `kind` | string | yes | Declaration kind (`"exec"` for Rust, `"def"` for Lean) |
| `language` | string | yes | `"rust"` or `"lean"` |
| `rust-qualified-name` | string | no | Rust-qualified path (Rust atoms only, when available) |

### Cross-Language Edges

When a Rust atom has a matching Lean translation, the Lean code-name is added
to the Rust atom's `dependencies` array. Similarly, the Rust code-name is
added to the Lean atom's `dependencies`. This creates bidirectional
cross-language edges in the merged graph.

### External Stubs

Functions referenced as dependencies but not defined in either input get stub
entries with:
- `code-path`: `""`
- `code-text`: `{"lines-start": 0, "lines-end": 0}`
- `dependencies`: empty

---

## 2. `probe/translations` -- Translation Mappings

**Produced by:** `translate`
**Envelope schema:** `"probe/translations"`

### Envelope Shape

```json
{
  "schema": "probe/translations",
  "schema-version": "2.0",
  "tool": {
    "name": "probe-aeneas",
    "version": "0.1.0",
    "command": "translate"
  },
  "timestamp": "2026-03-13T12:00:00Z",
  "sources": {
    "from": {
      "schema": "probe-rust/atoms",
      "package": "curve25519-dalek",
      "package-version": "4.1.3"
    },
    "to": {
      "schema": "probe-lean/extract",
      "package": "Curve25519Dalek",
      "package-version": "0.1.0"
    }
  },
  "mappings": [ ... ]
}
```

### Envelope Fields (translate-specific)

| Field | Type | Description |
|-------|------|-------------|
| `sources.from` | object | Metadata about the "from" (Rust) input |
| `sources.to` | object | Metadata about the "to" (Lean) input |
| `mappings` | array of TranslationMapping | The translation entries |

### Source Entry

| Field | Type | Description |
|-------|------|-------------|
| `schema` | string | Schema of the input (e.g. `"probe-rust/atoms"`) |
| `package` | string | Package name |
| `package-version` | string | Package version |

### TranslationMapping

```json
{
  "from": "probe:curve25519-dalek/4.1.3/scalar/Scalar#from_bytes_mod_order()",
  "to": "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order",
  "confidence": "exact",
  "method": "rust-qualified-name"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `from` | string | Rust code-name |
| `to` | string | Lean code-name |
| `confidence` | string | Match confidence level (see below) |
| `method` | string | Strategy that produced this mapping (see below) |

### Confidence Levels

| Value | Strategy | Description |
|-------|----------|-------------|
| `"exact"` | `rust-qualified-name` | Matched via Charon-derived `rust-qualified-name` joined with `functions.json` `rust_name` |
| `"file-and-name"` | `file+display-name` | Same source file + matching base method name (unambiguous) |
| `"file-and-lines"` | `file+line-overlap` | Same source file + overlapping line ranges |

### Strategy Priority

Strategies are applied in order. Once an atom is matched by an earlier
strategy, it is excluded from later strategies. This prevents duplicate
mappings.

---

## 3. `listfuns` -- Function Listing

**Produced by:** `listfuns`
**Envelope:** None (pass-through from `lake exe listfuns`)

The `listfuns` command delegates entirely to `lake exe listfuns` in the Lean
project. The output format is defined by the Lean project's `listfuns`
executable, not by probe-aeneas. Typical structure:

```json
{
  "functions": [
    {
      "lean_name": "Curve25519Dalek.Field.FieldElement51.reduce",
      "rust_name": "curve25519_dalek::field::FieldElement51::reduce",
      "source": "curve25519-dalek/src/backend/serial/u64/field.rs",
      "lines": "L292-L325"
    }
  ]
}
```

### FunctionRecord

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `lean_name` | string | yes | Fully qualified Lean name |
| `rust_name` | string | no | Corresponding Rust qualified name (from Charon LLBC) |
| `source` | string | no | Relative path to the Rust source file |
| `lines` | string | no | Line range in `"L<start>-L<end>"` format |

---

## Schema Evolution

When adding new optional fields, increment the minor version (`2.0` -> `2.1`).
When changing required fields or their semantics, increment the major version
(`2.0` -> `3.0`).

Consumers should check `schema-version` and reject files with an unsupported
major version.

---

## Compatibility

### With probe-rust

probe-aeneas consumes `probe-rust/atoms` (Schema 2.0) files as input.
The `--with-charon` flag on `probe-rust extract` is recommended for best
translation accuracy (enables strategy 1: `rust-qualified-name`).

### With probe-lean

probe-aeneas consumes `probe-lean/extract` files as input. These follow a
similar Schema 2.0 envelope with `"lean"` language atoms.

### With probe (shared crate)

The `probe` crate provides shared types (`Atom`, `TranslationMapping`,
`MergedAtomEnvelope`, `InputProvenance`) and the `merge_atom_maps` function.
probe-aeneas depends on these for consistent atom handling across the
ecosystem.
