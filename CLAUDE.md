# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

probe-aeneas is a Rust CLI tool that bridges Rust and Lean call graphs for [Aeneas](https://github.com/AeneasVerif/aeneas)-transpiled projects. It has four subcommands:
- **extract**: Full pipeline -- point at an Aeneas project directory (containing `aeneas-config.yml`) to auto-detect Rust/Lean paths, extract atoms, generate translation mappings, and merge into a unified atom file. Also supports explicit `--rust-project`/`--lean-project` flags or pre-generated JSON files.
- **translate**: Generate translation mappings between Rust and Lean code-names using `functions.json` as the bridge.
- **listfuns**: Generate enriched `functions.json` with verification data (default), or delegate to `lake exe listfuns`, or produce a basic function list.
- **setup**: Install external tool dependencies (probe-rust, rust-analyzer, charon). probe-lean is version-matched per project and auto-installed during `extract`.

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
├── translation_manifest.rs # Parses Aeneas translation.json; overlays authoritative loop/primary classification onto FunctionRecords
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

1. **Extract Pipeline** (`extract` command): resolve project (parse `aeneas-config.yml` if positional arg given) → resolve inputs → extract atoms (if project paths given) → load atoms + functions.json → generate translations → merge atom maps → Schema 3.0 envelope → output
2. **Translate Pipeline** (`translate` command): load Rust atoms + Lean atoms + functions.json → priority-ordered matching → translations JSON
3. **Listfuns Pipeline** (`listfuns` command): `lake exe listfuns` → functions.json

### Key Architectural Patterns

**Translation Matching** (in priority order, 1-to-1: each Rust function maps to one primary Lean definition):
0. `charon-def-id` -- precise integer join on the charon `FunDeclId`: a Rust atom's `charon-def-id` extension equals Aeneas's `translation.json` `def_id`, binding to the family's primary (non-loop) Lean def with no name normalization. Runs first; **provenance-gated** on the atom's `charon-version` matching the manifest's `charon_version` (best-effort provenance). A no-op for atoms without the field, which fall through to the name/location strategies below.
1. `rust-qualified-name` -- exact match via Charon-derived qualified names joined with `functions.json` `rust_name` entries
2. `file+display-name` -- same source file path + matching base method name (unambiguous only)
3. `file+line-overlap` -- same source file + overlapping line ranges (best overlap wins)

**Translation Metadata on Merged Atoms**: Merged Rust atoms carry `translation-name`, `translation-path`, and `translation-text` fields pointing to the primary Lean translation. All Rust atoms also carry `untracked` following the KB P24/P25 two-state scope model: `false` (tracked backlog) by default for every compiled function; `true` (out of scope, no `verification-status`) only when the function has no status **and** is cfg-inactive in the Aeneas build (its probe-rust `cfg` predicate is false — probe-rust >= 0.10.0 folds parent-file mod-chain gates into it, with `file-cfg` carrying the chain component for reason granularity), unmounted (probe-rust's `is-unmounted`: no `mod` chain from the package's lib/bin roots reaches its file), a foreign declaration (probe-rust's `is-foreign`: declared inside an `extern` block), or its Lean translation carries `@[out_of_scope]`. probe-aeneas evaluates and applies policy only; all source-fact extraction lives in probe-rust. When `untracked` is set, `untracked-reason` records the cause. Membership in `functions.json` does not affect scope — a compiled-but-untranslated function is backlog, not disabled. The active feature set is resolved via `cargo metadata` (default features overlaid by `charon.cargo_args`) in `cfg_eval.rs`; when unresolvable, cfg classification is skipped (conservative).

**Project Auto-Detection**: When a positional `PROJECT` path is given, `aeneas-config.yml` is parsed to derive `rust_project` (from `crate.dir`) and `lean_project` (the project root). If `crate.dir` lacks its own `Cargo.toml` but the project root is a Cargo `[workspace]`, the workspace root is used as `rust_project` (so probe-rust indexes all member crates), and the target member is validated via `cargo metadata --no-deps` using `crate.name`, `charon.package`, or `-p` from `charon.cargo_args`. The resolved package name is backfilled into `charon_config.package` to ensure `ensure_charon_llbc()` targets the correct crate. If `functions.json` exists at the project root, it is reused.

**Parallel Extraction**: When both Rust and Lean extractions are needed (via positional `PROJECT` or `--rust-project` + `--lean-project`), `probe-rust extract` and `probe-lean extract` run in parallel via scoped threads. On the manifest path `probe-rust` is invoked with `--translation <translation.json>` (it reads charon `def_id`s from the manifest, so the `ensure_charon_llbc()` pre-flight is skipped); the legacy (no-manifest) path uses `--with-charon` and pre-generates the LLBC.

**Auto-Install**: `probe-rust` is installed via `cargo install --git`. `probe-lean` is version-aware: the target project's `lean-toolchain` is read, then a versioned binary (`~/.local/bin/probe-lean-<version>`) is looked up or installed (pre-built download from GitHub Releases, falling back to source build with `lake build`). A `~/.local/bin/probe-lean` symlink points to the active version.

**Schema 3.0 Metadata Envelope**: Merged output uses the `probe-aeneas/extract` schema (`schema-version: "3.0"`, for the optional `charon-def-id`/`charon-version` atom fields); translation output uses the `probe/mappings` schema (`schema-version: "3.0"`). Both wrap payloads with tool info, source provenance, and timestamps.

**Relationship to `probe merge`**: probe-aeneas's `extract` command is an instantiation of the generic `probe merge` engine for the Aeneas Rust-to-Lean case. It generates translations (Aeneas-specific), calls `merge_atom_maps` from `probe::commands::merge` for the generic combine + cross-language-edge step, then enriches the result with Aeneas-specific metadata (`translation-*`, `untracked`). See [docs/architecture.md](docs/architecture.md) for the full picture. Shared types (`Atom`, `Mapping`, `MergedAtomEnvelope`, `InputProvenance`, `Tool`, `load_atom_file`) come from `probe::types`.

### Key Types

- `FunctionRecord`: Entry from `functions.json` mapping Lean names to Rust names with source locations
- `LineRange`: Parsed "L292-L325" line range with overlap detection
- `TranslateStats`: Statistics from translation generation (counts by confidence level)

## External Tool Dependencies

- **probe-rust**: Rust atom extractor (auto-installable via `cargo install`)
- **probe-lean**: Lean atom extractor (auto-installable; pre-built binary download or source build, version-matched to target project's `lean-toolchain`)
- **lake**: Lean build tool for running `listfuns` (must be installed via elan)

## Knowledge Base

The ecosystem KB lives at `../probe/kb/`. Read `../probe/kb/index.md` for orientation. Your implementation must conform to `../probe/kb/engineering/properties.md`. Use terminology from `../probe/kb/engineering/glossary.md`.

## Development Loop (Ralph Loop)

The Ralph Loop is defined in `../probe/CLAUDE.md` and applies to all probe ecosystem repos. For probe-aeneas, the loop is:

1. Implement the change
2. Run auditor skills if available (`/ambiguity-auditor`, `/code-quality-auditor`, `/test-quality-auditor`); read reports in `../probe/kb/reports/`
3. Fix every issue found
4. Repeat steps 2-3 until all auditors pass clean
5. Run the validation suite:
   ```bash
   cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
   ```
6. Also validate the upstream probe crate (since probe-aeneas depends on it):
   ```bash
   cd ../probe && cargo fmt -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test
   ```

Never skip the audit step. Never mark a task complete with unresolved audit findings.

### When to run the full loop

Run it when touching:
- Translation logic or enrichment pipeline (`translate.rs`, `enrich.rs`, `extract.rs`)
- Extract runner or auto-install logic (`extract_runner.rs`)
- Atom field consumption (any change to how probe-lean/probe-rust output is parsed)
- Anything that could violate a property in `../probe/kb/engineering/properties.md`

For trivial changes (typo fixes, comment updates, dependency bumps), just run `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test`.

## Before Committing

Always run at minimum:

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test
```

## Commit Message Style

Use conventional commits: `feat(module):`, `fix(module):`, `perf(module):`, `refactor(module):`

## Versioning Policy

The version is defined in `Cargo.toml`. CI auto-tags `v<version>` on pushes to `main` when the tag doesn't already exist, which triggers the release workflow.

Bump levels:
- **major**: breaking CLI or output-schema changes (e.g., renamed flags, removed fields, changed JSON structure)
- **minor**: new additive features (e.g., new subcommand, new output field, new matching strategy)
- **patch**: bug fixes, documentation, performance improvements
