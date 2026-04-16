# probe-aeneas Data Schemas

Version: 2.7
Date: 2026-04-16

This document specifies the JSON output formats produced by each probe-aeneas
subcommand. It complements the language-agnostic
[envelope-rationale.md](https://github.com/Beneficial-AI-Foundation/probe/blob/main/docs/envelope-rationale.md)
which defines the envelope wrapper; this document defines what goes **inside**
the `data` field and the output of non-enveloped commands.

---

## Common: Schema 2.0 Envelope

Both `extract` and `translate` commands wrap their output in a standardized
metadata envelope. The envelope fields vary slightly between commands (see
sections below), but share this structure:

| Field | Type | Description |
|-------|------|-------------|
| `schema` | string | Data type identifier (e.g. `"probe-aeneas/extract"`) |
| `schema-version` | string | Interchange spec version (`"2.0"`) |
| `tool.name` | string | Always `"probe-aeneas"` |
| `tool.version` | string | Semver version of the probe-aeneas binary |
| `tool.command` | string | Subcommand that produced the file |
| `timestamp` | string | ISO 8601 timestamp of when the analysis ran |

---

## 1. `probe-aeneas/extract` -- Merged Call Graph

**Produced by:** `extract`
**Envelope schema:** `"probe-aeneas/extract"`

### Envelope Shape

```json
{
  "schema": "probe-aeneas/extract",
  "schema-version": "2.0",
  "tool": {
    "name": "probe-aeneas",
    "version": "0.9.0",
    "command": "extract"
  },
  "inputs": [
    {
      "schema": "probe-rust/extract",
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

### Envelope Fields (extract-specific)

| Field | Type | Description |
|-------|------|-------------|
| `inputs` | array of InputProvenance | Metadata about each input atom file |

### InputProvenance

| Field | Type | Description |
|-------|------|-------------|
| `schema` | string | Schema of the input file (e.g. `"probe-rust/extract"`, `"probe-lean/extract"`) |
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

**Rust atom example** (with translation metadata, verification status, and cross-language edge):

```json
{
  "probe:curve25519-dalek/4.1.3/scalar/Scalar#from_bytes_mod_order()": {
    "display-name": "Scalar::from_bytes_mod_order",
    "dependencies": [
      "probe:curve25519-dalek/4.1.3/scalar/Scalar#reduce()",
      "probe:curve25519_dalek.scalar.Scalar.reduce"
    ],
    "code-module": "scalar",
    "code-path": "curve25519-dalek/src/scalar.rs",
    "code-text": { "lines-start": 237, "lines-end": 246 },
    "kind": "exec",
    "language": "rust",
    "rust-qualified-name": "curve25519_dalek::scalar::Scalar::from_bytes_mod_order",
    "is-disabled": false,
    "is-public": true,
    "is-public-api": true,
    "verification-status": "verified",
    "translation-name": "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order",
    "translation-path": "Curve25519Dalek/Funs.lean",
    "translation-text": { "lines-start": 7089, "lines-end": 7098 }
  }
}
```

In this example, `from_bytes_mod_order` calls Rust `reduce`, which has a
Lean translation `probe:curve25519_dalek.scalar.Scalar.reduce`. The
cross-language edge to the Lean `reduce` is added automatically by the
merge step (see [Cross-Language Edges](#cross-language-edges) below).

**Lean atom example** (def with specs, cross-language edges, and translation metadata):

```json
{
  "probe:curve25519_dalek.scalar.Scalar.reduce": {
    "display-name": "reduce",
    "dependencies": [
      "probe:curve25519-dalek/4.1.3/backend/serial/u64/scalar/impl<Scalar52>#[Scalar52]montgomery_reduce()",
      "probe:curve25519_dalek.backend.serial.u64.scalar.Scalar52.montgomery_reduce",
      "probe:curve25519_dalek.scalar.Scalar",
      "probe:curve25519_dalek.scalar.Scalar.unpack",
      "..."
    ],
    "type-dependencies": [
      "probe:curve25519_dalek.scalar.Scalar"
    ],
    "term-dependencies": [
      "probe:curve25519_dalek.scalar.Scalar",
      "probe:curve25519_dalek.scalar.Scalar.unpack",
      "probe:curve25519_dalek.backend.serial.u64.scalar.Scalar52.montgomery_reduce",
      "..."
    ],
    "code-module": "Curve25519Dalek.Funs",
    "code-path": "Curve25519Dalek/Funs.lean",
    "code-text": { "lines-start": 7079, "lines-end": 7087 },
    "kind": "def",
    "language": "lean",
    "verification-status": "verified",
    "specs": [
      "probe:curve25519_dalek.scalar.Scalar.reduce_spec",
      "probe:curve25519_dalek.scalar.Scalar.from_bytes_mod_order_spec",
      "probe:curve25519_dalek.scalar.Scalar.is_canonical_spec"
    ],
    "primary-spec": "probe:curve25519_dalek.scalar.Scalar.reduce_spec",
    "is-in-package": true,
    "is-relevant": true,
    "is-ignored": false,
    "is-hidden": false,
    "is-extraction-artifact": false,
    "rust-source": "curve25519-dalek/src/scalar.rs"
  }
}
```

Here the Lean `reduce` depends on Lean definitions like `montgomery_reduce`
and `Scalar.unpack`, plus cross-language edges back to the corresponding
Rust atoms (added automatically by the merge step).

**Lean trusted atom example** (axiom from `*External.lean`):

```json
{
  "probe:curve25519_dalek.Array.Insts.ZeroizeZeroize.zeroize": {
    "display-name": "zeroize",
    "code-module": "Curve25519Dalek.FunsExternal",
    "code-path": "Curve25519Dalek/FunsExternal.lean",
    "kind": "axiom",
    "language": "lean",
    "verification-status": "trusted",
    "trusted-reason": "axiom",
    "..."
  }
}
```

Trusted atoms represent the verification trust base: axioms (`trusted-reason:
"axiom"`) and hand-written definitions in `*External.lean` files
(`trusted-reason: "external"`).

### Atom Field Reference

#### Core fields (all atoms)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `display-name` | string | yes | Human-readable name |
| `dependencies` | array of strings | yes | Sorted code-names of callees, including cross-language edges added by extract |
| `code-module` | string | yes | Module path |
| `code-path` | string | yes | Source file path relative to the repository root (empty for external stubs). For Rust atoms, includes the crate directory prefix when the crate is a subdirectory (e.g. `curve25519-dalek/src/scalar.rs`). |
| `code-text` | object | yes | `{"lines-start": N, "lines-end": M}` (1-based, inclusive) |
| `kind` | string | yes | Declaration kind (see below) |
| `language` | string | yes | `"rust"` or `"lean"` |

**`kind` values:**

| Value | Language | Description |
|-------|----------|-------------|
| `"exec"` | Rust | Functions and methods |
| `"def"` | Lean | Definitions (transpiled functions, helper defs) |
| `"theorem"` | Lean | Theorems and specs |
| `"abbrev"` | Lean | Abbreviations (type aliases, short defs) |
| `"structure"` | Lean | Structure declarations |
| `"opaque"` | Lean | Opaque definitions |
| `"axiom"` | Lean | Axioms (e.g. external function stubs) |
| `"inductive"` | Lean | Inductive type declarations |

#### Rust-specific fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `rust-qualified-name` | string | no | Rust-qualified path (when available from Charon) |
| `is-disabled` | bool | yes | `false` if the function's `rust-qualified-name` appears as a `rust_name` in `functions.json` or it has a `translation-name`; `true` otherwise. Indicates whether Aeneas processed this function. |
| `is-relevant` | bool | yes | Inverse of `is-disabled`. `true` when Aeneas transpiled this function. |
| `is-public` | bool | yes | `true` if the Rust function is declared `pub` (from Charon LLBC `AttrInfo.public`). `false` for non-`pub` functions or when Charon data is unavailable. |
| `is-public-api` | bool | no | `true` if the function is part of the crate's public API (reachable by external consumers). Set by probe-rust; absent on external stubs. More selective than `is-public` — a `pub fn` inside a private module has `is-public: true` but `is-public-api: false`. |
| `verification-status` | string | no | Verification status derived from the Lean translation's primary spec theorem. When the Lean definition is `"trusted"` or `"failed"`, that status is propagated directly. Otherwise, if a primary spec exists (via `primary-spec` extension or `<name>_spec` naming convention), the spec's `verification-status` is used; if no spec exists, the status is `"unverified"`. Always present when `translation-name` is set; absent when the Rust function has no Lean translation. |
| `translation-name` | string | no | Code-name of the primary Lean translation (added by extract) |
| `translation-path` | string | no | Relative source file path of the Lean translation |
| `translation-text` | object | no | `{"lines-start": N, "lines-end": M}` of the Lean translation |

#### Lean-specific fields

These fields include data from `probe-lean extract` (passed through via the
atom's extension map) and additional fields computed by probe-aeneas during
the enrichment pass:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `verification-status` | string | yes | `"verified"`, `"unverified"`, `"trusted"`, or `"failed"`. `"trusted"` indicates the declaration belongs to the trust base (axioms or `*External.lean` files). |
| `trusted-reason` | string | no | Why the atom is trusted: `"axiom"` (axiomatic declaration) or `"external"` (defined in an `*External.lean` file). Present only when `verification-status` is `"trusted"`. |
| `type-dependencies` | array of strings | yes | Code-names of dependencies used in the type signature |
| `term-dependencies` | array of strings | yes | Code-names of dependencies used in the definition body |
| `is-in-package` | bool | yes | Whether the declaration belongs to the Lean package (from probe-lean) |
| `is-relevant` | bool | yes | Whether the atom is relevant for verification tracking |
| `is-ignored` | bool | yes | Whether the atom is explicitly ignored from progress metrics |
| `is-hidden` | bool | yes | Whether the atom is hidden from default views |
| `is-extraction-artifact` | bool | yes | Whether the atom is an Aeneas extraction artifact |
| `is-externally-verified` | bool | no | Whether the atom is verified externally (e.g. in Verus). Present only when `true`. |
| `attributes` | array of strings | no | Lean tag attributes detected on this declaration (from probe-lean). Absent when empty. |
| `specs` | array of strings | no | Code-names of spec theorems (present on defs/abbrevs that have associated specs) |
| `primary-spec` | string | no | Code-name of the primary spec theorem for this definition |
| `is-primary-spec` | bool | no | Whether this atom is the primary spec for a function (present on spec theorems) |

> **Note on spec discovery**: probe-aeneas resolves the primary spec via the `primary-spec` extension on the definition atom, falling back to the `<name>_spec` naming convention. It does not currently walk the `specs` array. Definitions whose specs do not match either pattern will be classified as `"unverified"` on the Rust side.
| `rust-source` | string or null | no | Rust source reference from Aeneas docstring |

### `is-disabled` -- Aeneas Scope Indicator

Every Rust atom in the merged output carries an `is-disabled` boolean that
records whether the function was processed by Aeneas during transpilation.

**Semantics:**
- `is-disabled: false` -- Aeneas transpiled this Rust function into Lean.
  Either the function's `rust-qualified-name` appears as a `rust_name` entry
  in the project's `functions.json`, or a translation was found by one of the
  secondary matching strategies (`file+display-name` or `file+line-overlap`).
- `is-disabled: true` -- Aeneas did **not** process this function. It exists
  in the Rust crate but has no corresponding Lean transpilation. This
  typically means the function is out of scope for formal verification.

**How it is computed:** During the `extract` merge step, probe-aeneas first
populates `translation-name` for Rust atoms that have a Lean translation
(found via any of the three matching strategies). Then for each Rust atom,
`is-disabled` is `false` if the atom's `rust-qualified-name` (normalized)
appears in `functions.json` **or** the atom already has a `translation-name`;
otherwise `is-disabled` is `true`.

**Relationship to translation fields:** A function with `is-disabled: false`
*may* still lack `translation-name`/`translation-path`/`translation-text` if
the matching strategies could not resolve which specific Lean definition
corresponds to it (the name appeared in `functions.json` but no Lean atom
could be paired).

**Consumer guidance:** Downstream tools can use `is-disabled` to partition the
Rust call graph into "in scope for verification" (`false`) and "out of scope"
(`true`). This is useful for computing verification coverage, filtering
dependency trees, or highlighting functions that still need transpilation.

### `is-public` -- Rust Visibility Indicator

Every Rust atom in the merged output carries an `is-public` boolean that
records whether the function is declared with the `pub` keyword.

**Semantics:**
- `is-public: true` -- the Rust function carries the `pub` visibility
  keyword. This is item-level visibility, not crate-level API reachability --
  a `pub fn` inside a private module will still have `is-public: true`.
- `is-public: false` -- the function is not declared `pub`, or Charon
  enrichment did not produce visibility data (e.g. Charon was not used, or
  the atom did not match a Charon entry).

**How it is computed:** When `probe-rust extract --with-charon` is used,
Charon's LLBC output includes `item_meta.attr_info.public` for each function
declaration. probe-rust reads this field and emits `is-public` on matched
atoms. During the `extract` merge step, probe-aeneas preserves any existing
`is-public` value from the Rust atoms and defaults missing ones to `false`.

**Consumer guidance:** Downstream tools can use `is-public` together with
`is-disabled` to identify functions that are both part of the public API and
in scope for verification, useful for prioritizing verification effort.

### Computed Fields: Auto vs Manual

probe-aeneas computes several Lean atom fields using Aeneas-specific
heuristics applied to the generic facts provided by probe-lean. This section
documents the computation method, coverage, and when manual configuration is
needed for each field.

| Field | Method | Computation Details |
|-------|--------|---------------------|
| `is-extraction-artifact` | **AUTO** | `true` when the display name ends with an Aeneas-standard suffix: `_body`, `_loop`, `_loop0`–`_loop3`. These suffixes are universal Aeneas conventions, identical across all Aeneas projects. No config needed. |
| `is-hidden` | **HYBRID** | **Auto:** `true` when `attributes` contains `"rust_trait_impl"`, OR the name matches a boilerplate `.Insts.` trait (Clone, Copy, Default, Zeroize), OR the name is a borrow-pattern delegator variant (`SharedA`/`SharedB` in receiver or `SharedB` in trait args — `Shared0` primary forms are kept visible), OR the name ends with `.mutual` (loop mutual recursion), OR the name contains `.closure` (closures), OR the name contains `.Blanket.` (blanket impls), OR the name contains `DOC_HIDDEN` (doc-hidden constants), OR the entry is an `.Insts.` parent with exactly one nested child method (single-child parent collapsing). **Manual:** project-specific entries via `aeneas.json` config (inner constants, project-specific helpers). |
| `is-relevant` | **AUTO** | For Lean atoms without `rust-source`: inherits `is-in-package` from probe-lean. For Lean atoms with `rust-source`: `true` when the source path contains the Rust crate name, does not start with `/`, and does not contain `/cargo/registry/`. This subsumes `excluded-namespace-prefixes` — external Rust dependencies that Aeneas transpiled will have `rust-source` paths from other crates. For Rust atoms: `is-relevant` = `!is-disabled` (i.e. `true` when in `functions.json` or has a translation). |
| `is-ignored` | **MANUAL** | Always requires explicit configuration in `aeneas.json`. This is a human editorial decision about what to exclude from verification progress percentages. probe-aeneas never auto-sets this to `true`. |
| `is-externally-verified` | **AUTO** | `true` when `attributes` (from probe-lean) contains `"externally_verified"`. Applied to spec theorems where the proof uses `sorry` but is verified externally (e.g. in Verus). |

#### Aeneas Config File

An optional configuration file for the manual tail of `is-hidden` and all of
`is-ignored`. Specified via `--aeneas-config` CLI flag or auto-discovered at
`.verilib/aeneas.json` in the Lean project directory.

```json
{
  "is-hidden": [
    "curve25519_dalek.field.FieldElement51.coset4",
    "curve25519_dalek.edwards.EdwardsPoint.inner"
  ],
  "is-ignored": [
    "curve25519_dalek.traits.Identity.identity"
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `is-hidden` | array of strings | Lean declaration names (without `probe:` prefix) to mark as hidden |
| `is-ignored` | array of strings | Lean declaration names (without `probe:` prefix) to mark as ignored |

Both fields are optional. Omitted lists default to empty.

### Translation Metadata

When a Rust atom has a matching Lean translation, the merged output enriches
the Rust atom with explicit translation metadata:

```json
{
  "probe:curve25519-dalek/4.1.3/.../add_assign()": {
    "display-name": "impl::add_assign",
    "language": "rust",
    "is-disabled": false,
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

In addition to the translation metadata fields above, `extract` adds
cross-language dependency edges via transitive expansion. For each atom
in the merged graph, every dependency that has a known translation gains
the translated code-name as an additional dependency:

- Rust atom A calls Rust atom B; B has Lean translation B' →
  A gains a dependency on B'.
- Lean atom X calls Lean atom Y; Y has Rust translation Y' →
  X gains a dependency on Y'.

This creates cross-language edges wherever a call site crosses the
Rust/Lean boundary through translated functions. The edges are
bidirectional in aggregate (Rust callers reach into the Lean graph and
vice versa) but each individual edge follows the call direction.

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
    "version": "0.9.0",
    "command": "translate"
  },
  "timestamp": "2026-03-16T12:00:00Z",
  "sources": {
    "from": {
      "schema": "probe-rust/extract",
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
| `schema` | string | Schema of the input (e.g. `"probe-rust/extract"`) |
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
| `method` | string or absent | Strategy that produced this mapping (see below). Always present in probe-aeneas output; may be absent in other producers. |

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
**Envelope:** None

The `listfuns` command has three modes:

1. **Enriched (default):** Parses Aeneas-generated `.lean` files, runs
   `probe-lean extract` internally, and produces an enriched function list
   with verification data, dependencies, and classification flags.
2. **Basic (`--no-enrich`):** Parses Aeneas-generated `.lean` files and
   produces a minimal function list without verification data.
3. **Lake (`--lake`):** Delegates entirely to `lake exe listfuns` in the
   Lean project. The output format is defined by the project's `listfuns`
   executable.

### Enriched Output (default)

```json
{
  "functions": [
    {
      "lean_name": "Curve25519Dalek.Field.FieldElement51.reduce",
      "rust_name": "curve25519_dalek::field::FieldElement51::reduce",
      "source": "curve25519-dalek/src/backend/serial/u64/field.rs",
      "lines": "L292-L325",
      "dependencies": ["Curve25519Dalek.Field.FieldElement51.mul", "..."],
      "nested_children": [],
      "is_relevant": true,
      "is_extraction_artifact": false,
      "is_hidden": false,
      "is_ignored": false,
      "specified": true,
      "verified": true,
      "fully_verified": true,
      "externally_verified": false,
      "spec_file": "Curve25519Dalek/Specs.lean",
      "spec_docstring": null,
      "spec_statement": null
    }
  ]
}
```

### Enriched FunctionRecord

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `lean_name` | string | yes | Fully qualified Lean name |
| `rust_name` | string | no | Corresponding Rust qualified name (from Charon LLBC) |
| `source` | string | no | Relative path to the Rust source file |
| `lines` | string | no | Line range in `"L<start>-L<end>"` format |
| `dependencies` | array of strings | yes | Lean dependency names (probe prefix stripped) |
| `nested_children` | array of strings | yes | For `.Insts.` parents with exactly one child method: contains that child's name (parent is auto-hidden). Empty otherwise. |
| `is_relevant` | bool | yes | Whether the function belongs to the target crate |
| `is_extraction_artifact` | bool | yes | Whether the function is an Aeneas extraction artifact |
| `is_hidden` | bool | yes | Whether the function is hidden from default views |
| `is_ignored` | bool | yes | Whether the function is ignored from progress metrics |
| `specified` | bool | yes | Whether a spec theorem exists for this function |
| `verified` | bool | yes | Whether the spec theorem has `verification-status: verified` (Lean proof only; exclusive of `externally_verified`) |
| `fully_verified` | bool | yes | Whether the function and all transitive Funs.lean dependencies are verified |
| `externally_verified` | bool | yes | Whether the function is verified externally (e.g. in Verus) |
| `spec_file` | string | no | Path to the file containing the spec theorem |
| `spec_docstring` | string | no | Spec theorem docstring (reserved, currently `null`) |
| `spec_statement` | string | no | Spec theorem statement text (reserved, currently `null`) |

### Basic Output (`--no-enrich`)

```json
{
  "functions": [
    {
      "lean_name": "Curve25519Dalek.Field.FieldElement51.reduce",
      "rust_name": "curve25519_dalek::field::FieldElement51::reduce",
      "source": "curve25519-dalek/src/backend/serial/u64/field.rs",
      "lines": "L292-L325",
      "is_hidden": false,
      "is_extraction_artifact": false
    }
  ]
}
```

### Basic FunctionRecord

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `lean_name` | string | yes | Fully qualified Lean name |
| `rust_name` | string | no | Corresponding Rust qualified name (from Charon LLBC) |
| `source` | string | no | Relative path to the Rust source file |
| `lines` | string | no | Line range in `"L<start>-L<end>"` format |
| `is_hidden` | bool | yes | Whether the function is hidden (name-pattern heuristic only) |
| `is_extraction_artifact` | bool | yes | Whether the function is an Aeneas extraction artifact |

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

probe-aeneas consumes `probe-rust/extract` (Schema 2.0) files as input.
The `--with-charon` flag on `probe-rust extract` is recommended for best
translation accuracy (enables strategy 1: `rust-qualified-name`).

### With probe-lean

probe-aeneas consumes `probe-lean/extract` files as input. These follow a
similar Schema 2.0 envelope with `"lean"` language atoms.

### With probe (shared crate)

probe-aeneas is an instantiation of the generic `probe merge` engine for
the Aeneas Rust-to-Lean case. The `extract` command generates
Aeneas-specific translations, calls `merge_atom_maps` from
`probe::commands::merge` for the combine + cross-language-edge step, then
enriches the result with Aeneas metadata (`translation-*`, `is-disabled`).
Shared types (`Atom`, `TranslationMapping`, `MergedAtomEnvelope`,
`InputProvenance`, `Tool`, `load_atom_file`) come from `probe::types`.
See [architecture.md](architecture.md) for the full architectural
description.
