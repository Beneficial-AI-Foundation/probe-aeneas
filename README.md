# probe-aeneas

Cross-language extract tool for [Aeneas](https://github.com/AeneasVerif/aeneas)-transpiled
projects. Bridges the gap between `probe-rust` (Rust atoms) and `probe-lean` (Lean atoms)
by generating translation mappings and producing a combined call graph with cross-language
dependency edges.

## Quick Start

```bash
# Install from source
cargo install --git https://github.com/Beneficial-AI-Foundation/probe-aeneas

# Extract merged Rust + Lean call graph (fully automated)
# Produces aeneas_{package}_{version}.json by default
probe-aeneas extract \
  --rust-project path/to/rust/project \
  --lean-project path/to/lean/project
```

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

## Commands

| Command | Description |
|---------|-------------|
| `extract` | Full pipeline: extract atoms (if needed), generate translations, merge Rust + Lean call graphs |
| `translate` | Generate Rust ↔ Lean translation mappings from pre-generated atom files |
| `listfuns` | Run `lake exe listfuns` in a Lean project to produce `functions.json` |

### `extract`

```bash
probe-aeneas extract [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--rust <PATH>` | Path to pre-generated Rust atoms JSON |
| `--rust-project <PATH>` | Path to a Rust project directory (runs `probe-rust extract` automatically) |
| `--lean <PATH>` | Path to pre-generated Lean atoms JSON |
| `--lean-project <PATH>` | Path to a Lean project directory (runs `probe-lean extract` automatically) |
| `--functions <PATH>` | Path to `functions.json` (auto-generated when `--lean-project` is given) |
| `-o, --output <PATH>` | Output file path (default: `aeneas_{package}_{version}.json` from Rust input) |

### `translate`

```bash
probe-aeneas translate --rust <PATH> --lean <PATH> --functions <PATH> [-o <PATH>]
```

### `listfuns`

```bash
probe-aeneas listfuns --lean-project <PATH> [-o <PATH>]
```

For the full command reference with all options, examples, and input modes, see **[docs/USAGE.md](docs/USAGE.md)**. For the complete JSON schema specification, see **[docs/SCHEMA.md](docs/SCHEMA.md)**.

## How It Works

1. **Input resolution** -- accepts pre-generated JSON files, project paths, or a mix. When project paths are given, runs `probe-rust extract` and `probe-lean extract` (in parallel if both are project paths).
2. **Translation generation** -- matches Rust atoms to Lean atoms via `functions.json` using three strategies in priority order:
   1. `rust-qualified-name` -- exact match via Charon-derived qualified names
   2. `file+display-name` -- same source file + matching base method name
   3. `file+line-overlap` -- same source file + overlapping line ranges
3. **Merge** -- combines Rust and Lean atom maps, adding cross-language dependency edges where translations exist.
4. **Schema 2.0 output** -- wraps the merged call graph in a metadata envelope containing input provenance, tool info, and timestamps.

## Auto-Install

When `--rust-project` or `--lean-project` is used, probe-aeneas automatically
finds or installs the required tools:

- **probe-rust**: checked on PATH, then `~/.cargo/bin/`. If not found, installed via
  `cargo install --git https://github.com/Beneficial-AI-Foundation/probe-rust.git`.
- **probe-lean**: checked on PATH, then `~/.local/bin/`. If not found, cloned and built
  with `lake build`, then copied to `~/.local/bin/probe-lean`.

## Prerequisites

- **Rust toolchain** (`cargo`) for building/installing `probe-rust`
- **Lean 4 toolchain** (`elan`, `lake`) for building/installing `probe-lean` and running `listfuns`
- `probe-rust` and `probe-lean` are auto-installed when using `--rust-project` / `--lean-project`

## License

MIT
