# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

probe-aeneas is a Rust CLI tool that bridges Rust and Lean call graphs for [Aeneas](https://github.com/AeneasVerif/aeneas)-transpiled projects. It has three subcommands:
- **extract**: Full pipeline -- point at an Aeneas project directory (containing `aeneas-config.yml`) to auto-detect Rust/Lean paths, extract atoms, generate translation mappings, and merge into a unified atom file. Also supports explicit `--rust-project`/`--lean-project` flags or pre-generated JSON files.
- **translate**: Generate translation mappings between Rust and Lean code-names using `functions.json` as the bridge.
- **listfuns**: Generate enriched `functions.json` with verification data (default), or delegate to `lake exe listfuns`, or produce a basic function list.

## Build and Test Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Optimized release build
cargo install --path .         # Install locally

# Test
cargo test                     # All tests
cargo test --lib --verbose     # Unit tests only

# Code quality (all enforced in CI)
cargo fmt --all                # Format code
cargo clippy --all-targets -- -D warnings  # Lint (no warnings allowed)

# Development workflow
cargo fmt && cargo clippy --all-targets && cargo test
```

## Project Structure

```
src/
├── main.rs            # CLI entry point with subcommand routing (clap)
├── extract.rs         # Extract pipeline: input resolution, translation, merge orchestration
├── enrich.rs          # Shared enrichment: heuristic classifiers, atom helpers, enrichment pipeline
├── translate.rs       # Translation logic: three matching strategies, JSON I/O, unit tests
├── extract_runner.rs  # Runs probe-rust and probe-lean extractors, auto-install logic
├── listfuns.rs        # Enriched listfuns pipeline, or delegates to `lake exe listfuns`
├── gen_functions.rs   # Parses Aeneas-generated .lean files into function records
└── types.rs           # FunctionRecord, FunctionsFile, LineRange for functions.json parsing
docs/
├── architecture.md    # How probe-aeneas relates to probe merge
├── SCHEMA.md          # JSON schema specification for all output formats
├── USAGE.md           # Full command reference with examples
└── testing.md         # Manual testing notes
examples/              # Sample input/output JSON files (curve25519-dalek ↔ Curve25519Dalek)
```

## Architecture

### Pipeline

1. **Extract Pipeline** (`extract` command): resolve project (parse `aeneas-config.yml` if positional arg given) → resolve inputs → extract atoms (if project paths given) → load atoms + functions.json → generate translations → merge atom maps → Schema 2.0 envelope → output
2. **Translate Pipeline** (`translate` command): load Rust atoms + Lean atoms + functions.json → three-strategy matching → translations JSON
3. **Listfuns Pipeline** (`listfuns` command): `lake exe listfuns` → functions.json

### Key Architectural Patterns

**Three-Strategy Translation Matching** (in priority order, 1-to-1: each Rust function maps to one primary Lean definition):
1. `rust-qualified-name` -- exact match via Charon-derived qualified names joined with `functions.json` `rust_name` entries
2. `file+display-name` -- same source file path + matching base method name (unambiguous only)
3. `file+line-overlap` -- same source file + overlapping line ranges (best overlap wins)

**Translation Metadata on Merged Atoms**: Merged Rust atoms carry `translation-name`, `translation-path`, and `translation-text` fields pointing to the primary Lean translation. All Rust atoms also carry `is-disabled` (`false` when the function's `rust-qualified-name` appears in `functions.json`, `true` otherwise).

**Project Auto-Detection**: When a positional `PROJECT` path is given, `aeneas-config.yml` is parsed to derive `rust_project` (from `crate.dir`) and `lean_project` (the project root). If `functions.json` exists at the project root, it is reused.

**Parallel Extraction**: When both Rust and Lean extractions are needed (via positional `PROJECT` or `--rust-project` + `--lean-project`), `probe-rust extract` and `probe-lean extract` run in parallel via scoped threads.

**Auto-Install**: `probe-rust` is installed via `cargo install --git`, `probe-lean` is cloned and built with `lake build`, then copied to `~/.local/bin/`.

**Schema 2.0 Metadata Envelope**: Merged output uses `probe-aeneas/extract` schema; translation output uses `probe/translations` schema. Both wrap payloads with tool info, source provenance, and timestamps.

**Relationship to `probe merge`**: probe-aeneas's `extract` command is an instantiation of the generic `probe merge` engine for the Aeneas Rust-to-Lean case. It generates translations (Aeneas-specific), calls `merge_atom_maps` from `probe::commands::merge` for the generic combine + cross-language-edge step, then enriches the result with Aeneas-specific metadata (`translation-*`, `is-disabled`). See [docs/architecture.md](docs/architecture.md) for the full picture. Shared types (`Atom`, `TranslationMapping`, `MergedAtomEnvelope`, `InputProvenance`, `Tool`, `load_atom_file`) come from `probe::types`.

### Key Types

- `FunctionRecord`: Entry from `functions.json` mapping Lean names to Rust names with source locations
- `LineRange`: Parsed "L292-L325" line range with overlap detection
- `TranslateStats`: Statistics from translation generation (counts by confidence level)

## External Tool Dependencies

- **probe-rust**: Rust atom extractor (auto-installable via `cargo install`)
- **probe-lean**: Lean atom extractor (auto-installable from source)
- **lake**: Lean build tool for running `listfuns` (must be installed via elan)

## Before Committing

Always run fmt and clippy before committing:

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

## Commit Message Style

Use conventional commits: `feat(module):`, `fix(module):`, `perf(module):`, `refactor(module):`
