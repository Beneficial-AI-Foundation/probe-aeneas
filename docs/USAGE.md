# Usage Guide

## Commands

### `extract`

Full pipeline: extract atoms (if needed), generate translation mappings, and
merge Rust + Lean call graphs into a unified atom file with cross-language
dependency edges.

```
probe-aeneas extract [OPTIONS] [PROJECT]
```

#### Project path (simplest form)

The simplest way to run `extract` is with a single Aeneas project directory.
The project must contain an `aeneas-config.yml` file; probe-aeneas reads
`crate.dir` to locate the Rust crate and uses the project root as the Lean
project.

```bash
probe-aeneas extract path/to/aeneas/project
```

If `functions.json` exists at the project root, it is reused automatically.
Otherwise, it is generated from the Lean sources.

**Aeneas project structure expected:**

| File | Required | Purpose |
|------|----------|---------|
| `aeneas-config.yml` | Yes | Must contain `crate.dir` (path to Rust crate relative to project root) |
| `lakefile.toml` or `lakefile.lean` | Yes | Identifies the Lean project root |
| `Cargo.toml` | Yes | Must exist at the resolved Rust crate path (`project / crate.dir`) |
| `functions.json` | No | Reused if present; otherwise auto-generated from Lean sources |

**Charon configuration** (optional `charon` section in `aeneas-config.yml`):

When present, probe-aeneas pre-generates the Charon LLBC file with the full
project-specific settings before running `probe-rust`. Supported fields:

| Field | Description |
|-------|-------------|
| `preset` | Charon preset (default: `aeneas`) |
| `package` | Cargo package name (passed as `--package`) |
| `cargo_args` | Extra cargo args (e.g. `["--no-default-features", "--features", "alloc"]`) |
| `start_from` | Rust item paths to use as translation starting points |
| `exclude` | Rust item paths to exclude from translation |
| `opaque` | Rust item paths to keep opaque |

The generated LLBC is cached at `<rust_project>/data/charon.llbc` and reused
on subsequent runs.

#### Advanced input options

For advanced usage (pre-generated JSON files, mixed inputs), use the named
flags below. These are mutually exclusive with the positional `PROJECT` argument.

**Input options (Rust):**

| Flag | Description |
|------|-------------|
| `--rust <PATH>` | Path to pre-generated Rust atoms JSON (from `probe-rust extract`). |
| `--rust-project <PATH>` | Path to a Rust project directory. Runs `probe-rust extract --with-charon --auto-install` automatically. |

Exactly one of `--rust` or `--rust-project` is required (when not using `PROJECT`).

**Input options (Lean):**

| Flag | Description |
|------|-------------|
| `--lean <PATH>` | Path to pre-generated Lean atoms JSON (from `probe-lean extract`). |
| `--lean-project <PATH>` | Path to a Lean project directory. Runs `probe-lean extract` and `lake exe listfuns` automatically. |

At least one of `--lean` or `--lean-project` is required (when not using `PROJECT`).

**Other options:**

| Flag | Short | Description |
|------|-------|-------------|
| `--functions <PATH>` | | Path to `functions.json` (Aeneas name mapping). Auto-generated when `--lean-project` or `PROJECT` is given. Required when using `--lean` alone. |
| `--output <PATH>` | `-o` | Output file path. Default: `<project>/.verilib/probes/aeneas_<pkg>_<ver>.json` when a project root is available; otherwise `aeneas_<pkg>_<ver>.json` in the current directory. |
| `--aeneas-config <PATH>` | | Path to Aeneas config JSON for manual overrides (`is-hidden`, `is-ignored`). Defaults to `.verilib/aeneas.json` in the Lean project. |
| `--lake` | | Use `lake exe listfuns` to generate `functions.json` instead of parsing Aeneas-generated Lean files directly. |

### Examples

**From an Aeneas project directory (recommended):**

```bash
# curve25519-dalek-lean-verify (crate.dir = "curve25519-dalek")
probe-aeneas extract ~/git_repos/baif/curve25519-dalek-lean-verify

# SparsePostQuantumRatchet (crate.dir = ".")
probe-aeneas extract ~/git_repos/baif/spqr_aeneas
```

**From separate project paths:**

```bash
probe-aeneas extract \
  --rust-project path/to/rust/project \
  --lean-project path/to/lean/project \
  --output merged.json
```

**From pre-generated JSON files:**

```bash
probe-aeneas extract \
  --rust path/to/rust_atoms.json \
  --lean path/to/lean_atoms.json \
  --functions path/to/functions.json \
  --output merged.json
```

**Mixed mode (one project path, one JSON):**

```bash
probe-aeneas extract \
  --rust-project path/to/rust/project \
  --lean path/to/lean_atoms.json \
  --functions path/to/functions.json \
  --output merged.json
```

---

### `translate`

Generate a translations file mapping Rust code-names to Lean code-names.
Requires pre-generated atom files and `functions.json`.

```
probe-aeneas translate [OPTIONS]
```

**Options:**

| Flag | Short | Description |
|------|-------|-------------|
| `--rust <PATH>` | | Path to Rust atoms JSON (from `probe-rust extract`). Required. |
| `--lean <PATH>` | | Path to Lean atoms JSON (from `probe-lean extract`). Required. |
| `--functions <PATH>` | | Path to `functions.json` (from `lake exe listfuns`). Required. |
| `--output <PATH>` | `-o` | Output file path. Default: `translations.json`. |

### Examples

```bash
probe-aeneas translate \
  --rust rust_atoms.json \
  --lean lean_atoms.json \
  --functions functions.json \
  --output translations.json
```

---

### `listfuns`

Generate `functions.json` from a Lean project. By default, parses
Aeneas-generated `.lean` files directly and enriches with verification data
from `probe-lean extract`. Use `--no-enrich` for a basic function list without
verification data. Use `--lake` to delegate to the project's own
`lake exe listfuns` executable.

```
probe-aeneas listfuns [OPTIONS]
```

**Options:**

| Flag | Short | Description |
|------|-------|-------------|
| `--lean-project <PATH>` | | Path to the Lean project directory. Required. |
| `--output <PATH>` | `-o` | Output file path. Default: `functions.json`. |
| `--lake` | | Use `lake exe listfuns` instead of parsing Lean files directly. |
| `--no-enrich` | | Skip enrichment (no probe-lean call, basic function list only). |
| `--atoms <PATH>` | | Path to pre-computed atoms JSON (from `probe-lean extract`). Skips the internal probe-lean invocation. |
| `--module <PREFIX>` | | Module prefix filter passed to `probe-lean extract` via `-m`. |
| `--aeneas-config <PATH>` | | Path to Aeneas config JSON for manual overrides (`is-hidden`). Defaults to `.verilib/aeneas.json`. |

### Examples

**Enriched output (default):**

```bash
probe-aeneas listfuns \
  --lean-project path/to/lean/project \
  --output functions.json
```

**With pre-computed atoms (skip probe-lean invocation):**

```bash
probe-aeneas listfuns \
  --lean-project path/to/lean/project \
  --atoms path/to/lean_atoms.json \
  --output functions.json
```

**Basic function list without verification data:**

```bash
probe-aeneas listfuns \
  --lean-project path/to/lean/project \
  --no-enrich \
  --output functions.json
```

**Delegate to lake exe listfuns:**

```bash
probe-aeneas listfuns \
  --lean-project path/to/lean/project \
  --lake \
  --output functions.json
```

---

## Translation Strategies

Translations are generated by matching Rust atoms to Lean atoms via
`functions.json` (which maps Rust qualified names to Lean names). Three
strategies are used in priority order. Once an atom is matched by an earlier
strategy, it is excluded from later ones.

### 1. `rust-qualified-name` (confidence: `exact`)

Matches via the `rust-qualified-name` extension field on Rust atoms (derived
from Charon LLBC), joined with the `rust_name` field in `functions.json`.
Names are normalized: lifetime parameters, reference markers, brace wrappers,
and generics are stripped before comparison.

**Requires:** `probe-rust extract --with-charon` for the Rust atoms.

### 2. `file+display-name` (confidence: `file-and-name`)

Matches when a Rust atom's `code-path` matches a `functions.json` entry's
`source` field, and the Rust atom's base method name (last `::` segment of
`display-name`) matches the Lean function's base name. Only unambiguous
matches (exactly one candidate) are accepted.

### 3. `file+line-overlap` (confidence: `file-and-lines`)

Matches when a Rust atom's `code-path` matches a `functions.json` entry's
`source` field, and the Rust atom's line range overlaps with the function
record's line range (with a 10-line tolerance). When multiple candidates
overlap, the one with the greatest overlap is chosen.

---

## Output Formats

For the complete JSON schema specification covering all commands, see
[SCHEMA.md](SCHEMA.md).

### Merged Atoms

The `extract` command produces a JSON file wrapped in a Schema 2.0 metadata
envelope with `"probe-aeneas/extract"` schema. The `data` field contains all
atoms from both inputs, with cross-language dependency edges added where
translations exist.

### Translations

The `translate` command produces a JSON file with `"probe/translations"`
schema containing an array of `{from, to, confidence, method}` mappings.

---

## Auto-Install Behavior

When a project path is given (positional `PROJECT` or `--rust-project` /
`--lean-project`), probe-aeneas locates or installs the required extractor
tools automatically.

### probe-rust

**Resolution order:**

1. `probe-rust` on `$PATH`
2. `~/.cargo/bin/probe-rust`
3. Install via `cargo install --git https://github.com/Beneficial-AI-Foundation/probe-rust.git`

### probe-lean

probe-aeneas detects the target project's Lean version from its
`lean-toolchain` file and installs a version-matched `probe-lean` binary.
Multiple Lean versions can coexist via per-version binaries.

**Resolution order:**

1. `~/.local/bin/probe-lean-<version>` (versioned binary matching the target project's Lean toolchain)
2. `probe-lean` on `$PATH`
3. `~/.local/bin/probe-lean` (unversioned symlink / fallback when no `lean-toolchain` is found)
4. Download pre-built binary from GitHub Releases (`probe-lean-<version>-<platform>.tar.gz`)
5. Clone from source, pin `lean-toolchain` to the target version, build with `lake build`, install to `~/.local/bin/probe-lean-<version>`

After installation, a `~/.local/bin/probe-lean` symlink is created pointing
to the versioned binary.

---

## Prerequisites

- **probe-aeneas** itself can be installed from [GitHub Releases](https://github.com/Beneficial-AI-Foundation/probe-aeneas/releases) (pre-built binaries) or via `cargo install --git`
- **Rust toolchain** (`cargo`) -- for building/installing `probe-rust`
- **Lean 4 toolchain** (`elan`, `lake`) -- for building/installing `probe-lean` and running `listfuns`
- `probe-rust` and `probe-lean` are auto-installed when using `--rust-project` / `--lean-project`

---

## Parallel Extraction

When both Rust and Lean extractions are needed (either via the positional
`PROJECT` argument or via `--rust-project` + `--lean-project`), `probe-rust
extract` and `probe-lean extract` are run in parallel using scoped threads.
This can significantly reduce wall-clock time for the extract pipeline.

---

## Output Files

When running `extract` from a project path (positional `PROJECT` or
`--lean-project`), all output files are written to
`<project>/.verilib/probes/`, following the probe ecosystem convention
(same layout as probe-rust, probe-verus, probe-lean):

```
<project>/.verilib/probes/
  rust_extract.json                  # Intermediate probe-rust output
  lean_extract.json                  # Intermediate probe-lean output
  aeneas_<package>_<version>.json    # Merged output from probe-aeneas
```

When using pre-generated JSON files (`--rust` / `--lean`) without a project
root, the merged output defaults to `aeneas_<package>_<version>.json` in the
current directory. The `-o` flag always overrides the output location.

### Code paths

Rust atom `code-path` values are relative to the repository root (the Aeneas
project directory). When the Rust crate lives in a subdirectory
(`crate.dir != "."` in `aeneas-config.yml`), probe-aeneas prefixes the
crate-relative paths from probe-rust with the crate directory. For example,
with `crate.dir = "curve25519-dalek"`:

- probe-rust produces: `src/backend/mod.rs`
- probe-aeneas emits: `curve25519-dalek/src/backend/mod.rs`

This ensures `code-path` matches file paths as stored when the full
repository is ingested into a database.
