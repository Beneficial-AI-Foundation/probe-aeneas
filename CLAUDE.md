# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

probe-aeneas is a Rust CLI tool that bridges Rust and Lean call graphs for [Aeneas](https://github.com/AeneasVerif/aeneas)-transpiled projects. It has three subcommands:
- **extract**: Full pipeline -- extract atoms (if needed), generate translation mappings, and merge Rust + Lean call graphs into a unified atom file with cross-language edges.
- **translate**: Generate translation mappings between Rust and Lean code-names using `functions.json` as the bridge.
- **listfuns**: Run `lake exe listfuns` in a Lean project to produce `functions.json`.

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
├── translate.rs       # Translation logic: three matching strategies, JSON I/O, unit tests
├── extract_runner.rs  # Runs probe-rust and probe-lean extractors, auto-install logic
├── listfuns.rs        # Runs `lake exe listfuns` in a Lean project
└── types.rs           # FunctionRecord, FunctionsFile, LineRange for functions.json parsing
docs/
├── SCHEMA.md          # JSON schema specification for all output formats
├── USAGE.md           # Full command reference with examples
└── testing.md         # Manual testing notes
examples/              # Sample input/output JSON files (curve25519-dalek ↔ Curve25519Dalek)
```

## Architecture

### Pipeline

1. **Extract Pipeline** (`extract` command): resolve inputs → extract atoms (if project paths given) → load atoms + functions.json → generate translations → merge atom maps → Schema 2.0 envelope → output
2. **Translate Pipeline** (`translate` command): load Rust atoms + Lean atoms + functions.json → three-strategy matching → translations JSON
3. **Listfuns Pipeline** (`listfuns` command): `lake exe listfuns` → functions.json

### Key Architectural Patterns

**Three-Strategy Translation Matching** (in priority order, 1-to-1: each Rust function maps to one primary Lean definition):
1. `rust-qualified-name` -- exact match via Charon-derived qualified names joined with `functions.json` `rust_name` entries
2. `file+display-name` -- same source file path + matching base method name (unambiguous only)
3. `file+line-overlap` -- same source file + overlapping line ranges (best overlap wins)

**Translation Metadata on Merged Atoms**: Merged Rust atoms carry `translation-name`, `translation-path`, and `translation-text` fields pointing to the primary Lean translation. All Rust atoms also carry `is-disabled` (`false` when the function's `rust-qualified-name` appears in `functions.json`, `true` otherwise).

**Parallel Extraction**: When both `--rust-project` and `--lean-project` are given, `probe-rust extract` and `probe-lean extract` run in parallel via scoped threads.

**Auto-Install**: `probe-rust` is installed via `cargo install --git`, `probe-lean` is cloned and built with `lake build`, then copied to `~/.local/bin/`.

**Schema 2.0 Metadata Envelope**: Merged output uses `probe-aeneas/extract` schema; translation output uses `probe/translations` schema. Both wrap payloads with tool info, source provenance, and timestamps.

**Shared Types via `probe` Crate**: Core types (`Atom`, `TranslationMapping`, `MergedAtomEnvelope`, `merge_atom_maps`, `load_atom_file`) come from the shared `probe` crate.

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
