# probe-aeneas Data Schemas

Version: 2.0
Date: 2026-03-16

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
      "source": {
        "repo": "https://github.com/dalek-cryptography/curve25519-dalek.git",
        "commit": "5312a0311ec40df95be953eacfa8a11b9a34bc54",
        "language": "rust",
        "package": "curve25519-dalek",
        "package-version": "4.1.3"
      }
    },
    {
      "schema": "probe-lean/extract",
      "source": {
        "repo": "https://github.com/Beneficial-AI-Foundation/curve25519-dalek-lean-verify.git",
        "commit": "924fd9b5249edbd5dd0765bc21891f8bb0eb5d86",
        "language": "lean",
        "package": "Curve25519Dalek",
        "package-version": "0.1.0"
      }
    }
  ],
  "timestamp": "2026-03-16T12:00:05Z",
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
| `source` | Source | Source metadata propagated from the input envelope |

### Source

| Field | Type | Description |
|-------|------|-------------|
| `repo` | string | Git repository URL |
| `commit` | string | Git commit hash |
| `language` | string | Language of the input (`"rust"` or `"lean"`) |
| `package` | string | Package/crate name |
| `package-version` | string | Package version |

### Data Shape

`data` is an object keyed by code-name. Each value is an atom from one of the
input files, potentially enriched with cross-language dependency edges. The
atom format follows the shared `probe` atom schema with language-specific
extension fields passed through verbatim.

**Rust atom example** (with translation metadata added by merge):

```json
{
  "probe:curve25519-dalek/4.1.3/scalar/Scalar#from_bytes_mod_order()": {
    "display-name": "Scalar::from_bytes_mod_order",
    "dependencies": [
      "probe:curve25519-dalek/4.1.3/scalar/helper()",
      "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order"
    ],
    "code-module": "scalar",
    "code-path": "src/scalar.rs",
    "code-text": { "lines-start": 237, "lines-end": 246 },
    "kind": "exec",
    "language": "rust",
    "rust-qualified-name": "curve25519_dalek::scalar::Scalar::from_bytes_mod_order",
    "translation-name": "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order",
    "translation-path": "Curve25519Dalek/Funs.lean",
    "translation-text": { "lines-start": 7089, "lines-end": 7098 }
  }
}
```

**Lean atom example** (with probe-lean extension fields):

```json
{
  "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order": {
    "display-name": "from_bytes_mod_order",
    "dependencies": [
      "probe:curve25519_dalek.scalar.Scalar.reduce",
      "probe:curve25519-dalek/4.1.3/scalar/Scalar#from_bytes_mod_order()"
    ],
    "code-module": "Curve25519Dalek.Funs",
    "code-path": "Curve25519Dalek/Funs.lean",
    "code-text": { "lines-start": 7089, "lines-end": 7098 },
    "kind": "def",
    "language": "lean",
    "name": "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order",
    "verification-status": "verified",
    "specified": true,
    "specs": ["probe:from_bytes_mod_order_spec"],
    "is-relevant": true,
    "is-ignored": false,
    "is-hidden": false,
    "is-extraction-artifact": false,
    "rust-source": null
  }
}
```

### Atom Field Reference

#### Core fields (all atoms)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `display-name` | string | yes | Human-readable name |
| `dependencies` | array of strings | yes | Sorted code-names of callees, including cross-language edges added by merge |
| `code-module` | string | yes | Module path |
| `code-path` | string | yes | Relative source file path (empty for external stubs) |
| `code-text` | object | yes | `{"lines-start": N, "lines-end": M}` (1-based, inclusive) |
| `kind` | string | yes | Declaration kind (see below) |
| `language` | string | yes | `"rust"` or `"lean"` |

**`kind` values:** `"exec"` (Rust functions), `"def"` (Lean definitions),
`"theorem"` (Lean theorems/specs).

#### Rust-specific fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `rust-qualified-name` | string | no | Rust-qualified path (when available from Charon) |
| `translation-name` | string | no | Code-name of the primary Lean translation (added by merge) |
| `translation-path` | string | no | Relative source file path of the Lean translation |
| `translation-text` | object | no | `{"lines-start": N, "lines-end": M}` of the Lean translation |

#### Lean-specific fields

These fields originate from `probe-lean extract` and are passed through
verbatim via the atom's extension map:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | yes | Full code-name (same as the map key) |
| `verification-status` | string | yes | `"verified"`, `"unverified"`, or `"failed"` |
| `specified` | bool | yes | Whether the definition has associated specs |
| `specs` | array of strings | no | Code-names of spec theorems (present when `specified` is true) |
| `is-relevant` | bool | yes | Whether the atom is part of the project's own code |
| `is-ignored` | bool | yes | Whether the atom is explicitly ignored |
| `is-hidden` | bool | yes | Whether the atom is hidden from default views |
| `is-extraction-artifact` | bool | yes | Whether the atom is an Aeneas extraction artifact |
| `rust-source` | string or null | no | Rust source reference from Aeneas docstring |

### Translation Metadata

When a Rust atom has a matching Lean translation, the merged output enriches
the Rust atom with explicit translation metadata:

```json
{
  "probe:curve25519-dalek/4.1.3/.../add_assign()": {
    "display-name": "impl::add_assign",
    "language": "rust",
    "translation-name": "probe:curve25519_dalek...add_assign",
    "translation-path": "Curve25519Dalek/Funs.lean",
    "translation-text": { "lines-start": 446, "lines-end": 456 },
    ...
  }
}
```

Each Rust function maps to exactly one primary Lean definition (1-to-1).
Aeneas loop decompositions (e.g. `add_assign_loop`, `add_assign_loop.mutual`)
are reachable via the Lean definition's own dependency graph, not listed as
separate translations.

### Cross-Language Edges

In addition to the translation metadata fields above, the Lean code-name is
added to the Rust atom's `dependencies` array and vice versa. This creates
bidirectional cross-language edges in the merged graph.

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
  "timestamp": "2026-03-16T12:00:00Z",
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

Strategies are applied in order. Once a Rust atom or Lean atom is matched by
an earlier strategy, it is excluded from later strategies. Each Rust function
maps to exactly one Lean definition (1-to-1).

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
