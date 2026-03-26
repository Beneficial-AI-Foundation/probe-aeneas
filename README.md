# probe-aeneas

Cross-language extract tool for [Aeneas](https://github.com/AeneasVerif/aeneas)-transpiled projects.

`probe-aeneas` bridges `probe-rust` (Rust atoms) and `probe-lean` (Lean atoms) by generating translation mappings and producing a combined call graph with cross-language dependency edges. Output follows the Schema 2.0 envelope format; see [docs/SCHEMA.md](docs/SCHEMA.md) for the full specification.

## Prerequisites

probe-aeneas itself is a pure Rust binary, but the `extract` pipeline depends on
two language toolchains to build and run the extractors:

| Toolchain | Required for | Install guide |
|-----------|-------------|---------------|
| **Rust** (`cargo`) | Building probe-aeneas and `probe-rust` | [rustup.rs](https://rustup.rs/) |
| **Lean 4** (`elan`, `lake`) | Building `probe-lean` and running `listfuns` | [elan](https://github.com/leanprover/elan#installation), [probe-lean README](https://github.com/Beneficial-AI-Foundation/probe-lean#readme) |

The extractor tools (`probe-rust`, `probe-lean`) are auto-installed on first use,
but the underlying language toolchains must already be present.

## Installation

### From source (recommended)

```bash
cargo install --git https://github.com/Beneficial-AI-Foundation/probe-aeneas
```

Or clone and build locally:

```bash
git clone https://github.com/Beneficial-AI-Foundation/probe-aeneas
cd probe-aeneas
cargo install --path .
```

## Quick Start

```bash
# Point at an Aeneas project directory (reads aeneas-config.yml)
probe-aeneas extract path/to/aeneas/project

# Or use pre-generated JSON files
probe-aeneas extract \
  --rust rust_atoms.json \
  --lean lean_atoms.json \
  --functions functions.json
```

Output lands in `aeneas_{package}_{version}.json` by default.

## Commands

| Command | Description |
|---------|-------------|
| `extract` | Full pipeline: extract atoms (if needed), generate translations, merge Rust + Lean call graphs |
| `translate` | Generate Rust ↔ Lean translation mappings from pre-generated atom files |
| `listfuns` | Run `lake exe listfuns` in a Lean project to produce `functions.json` |

### `extract`

```bash
probe-aeneas extract [OPTIONS] [PROJECT]
```

| Argument / Option | Description |
|-------------------|-------------|
| `PROJECT` | Path to an Aeneas project directory (contains `aeneas-config.yml`). Auto-detects Rust and Lean paths. |
| `--rust <PATH>` | Path to pre-generated Rust atoms JSON |
| `--rust-project <PATH>` | Path to a Rust project directory (runs `probe-rust extract` automatically) |
| `--lean <PATH>` | Path to pre-generated Lean atoms JSON |
| `--lean-project <PATH>` | Path to a Lean project directory (runs `probe-lean extract` automatically) |
| `--functions <PATH>` | Path to `functions.json` (auto-generated when `--lean-project` or `PROJECT` is given) |
| `-o, --output <PATH>` | Output file path (default: `aeneas_{package}_{version}.json` from Rust input) |

For the full command reference with all options, examples, and input modes, see **[docs/USAGE.md](docs/USAGE.md)**. For the complete JSON schema specification, see **[docs/SCHEMA.md](docs/SCHEMA.md)**.

## Example Output

Running `probe-aeneas extract` produces a JSON envelope. Each entry in `data` describes a function from either language, with cross-language dependency edges:

```json
{
  "schema": "probe-aeneas/extract",
  "schema-version": "2.0",
  "tool": { "name": "probe-aeneas", "version": "0.1.0", "command": "extract" },
  "inputs": [
    { "schema": "probe-rust/extract", "package": "curve25519-dalek", "package-version": "4.1.3" },
    { "schema": "probe-lean/extract", "package": "Curve25519Dalek", "package-version": "0.1.0" }
  ],
  "timestamp": "2026-03-17T12:00:00Z",
  "data": {
    "probe:curve25519-dalek/4.1.3/scalar/Scalar#add()": {
      "display-name": "Scalar::add",
      "dependencies": ["probe:Curve25519Dalek.Scalar.add"],
      "code-module": "scalar",
      "code-path": "src/scalar.rs",
      "code-text": { "lines-start": 42, "lines-end": 67 },
      "kind": "exec",
      "language": "rust",
      "translation-name": "probe:Curve25519Dalek.Scalar.add",
      "translation-path": "Curve25519Dalek/Scalar.lean",
      "translation-text": { "lines-start": 10, "lines-end": 25 },
      "is-disabled": false
    }
  }
}
```

## How It Works

1. **Input resolution** -- accepts an Aeneas project directory (auto-detects paths from `aeneas-config.yml`), pre-generated JSON files, explicit project paths, or a mix. When both Rust and Lean extractions are needed, they run in parallel.
2. **Translation generation** -- matches Rust atoms to Lean atoms via `functions.json` using three strategies in priority order:
   1. `rust-qualified-name` -- exact match via Charon-derived qualified names
   2. `file+display-name` -- same source file + matching base method name
   3. `file+line-overlap` -- same source file + overlapping line ranges
3. **Merge** -- combines Rust and Lean atom maps, adding cross-language dependency edges where translations exist.
4. **Enrich** -- adds `translation-name`, `translation-path`, `translation-text`, and `is-disabled` to Rust atoms.
5. **Schema 2.0 output** -- wraps the merged call graph in a metadata envelope containing input provenance, tool info, and timestamps.

## License

MIT
