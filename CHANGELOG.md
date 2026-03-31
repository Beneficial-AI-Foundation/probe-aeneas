# Changelog

All notable changes to probe-aeneas are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.2] - 2026-03-31

### Fixed
- **probe-lean version resolution**: when a Lean project specifies a toolchain version, only accept the exact versioned binary (`probe-lean-<version>`). Previously, an unversioned `probe-lean` on PATH could be returned even when it was built for a different Lean version, causing olean incompatibility errors.

### Changed
- Regenerated `examples/aeneas_curve25519-dalek_4.1.3.json` with latest extractors.

## [0.3.1] - 2026-03-27

### Fixed
- Resolved clippy and fmt CI failures.

## [0.3.0] - 2026-03-27

### Added
- **Borrow-pattern delegator hiding**: `SharedA`/`SharedB` Aeneas borrow-pattern delegator variants are now auto-hidden; `Shared0` primary forms remain visible.
- **Single-child parent collapsing**: `.Insts.` parents with exactly one nested child method are auto-hidden; `nested_children` is populated with the child name.
- **`compute_fully_verified` with external verification**: externally verified functions now count as verified in the fully-verified transitive walk.

### Changed
- **`verified` semantics**: `verified` is now `true` when the spec has `verification-status: verified` OR the spec is externally verified (previously only proof-verified).
- **`nested_children` populated**: was always `[]`; now contains child names for single-child `.Insts.` parents.

### Removed
- **`specs` field** from enriched `listfuns` output (`EnrichedFunctionOutput`). The `specs` array on Lean atoms (from probe-lean) is unaffected.
- **`atom_specs` helper** and the specs-array fallback strategy in `find_primary_spec`.

## [0.2.0] - 2026-03-27

### Added
- **`is-public` field** on all Rust atoms in merged output. `true` when the item is declared `pub` per Charon LLBC; `false` when private or visibility data unavailable.
- **`is-disabled` field** on all Rust atoms in merged output. `false` when the function's `rust-qualified-name` appears as a `rust_name` in `functions.json` (i.e. Aeneas processed it); `true` otherwise.
- **`is-relevant` field** on all Rust atoms (inverse of `is-disabled`).
- **Positional project path**: `probe-aeneas extract <project>` parses `aeneas-config.yml` to auto-detect Rust/Lean paths and `functions.json`.
- **Enriched `listfuns`**: `probe-aeneas listfuns --enriched` parses Aeneas-generated Lean files directly to produce `functions.json` with verification data.

### Changed
- Release workflow migrated to cargo-dist v0.31.0 (aligned with probe-rust and probe-verus). Adds Windows target, shell/powershell installers, checksums, and PR dry-run builds.
- CI workflow simplified: removed redundant `probe` repo checkout (now fetched via git dep), updated to `actions/checkout@v6`.

## [0.1.0] - 2026-03-13

Initial release.

### Added
- `merge` command: full pipeline that extracts atoms (if needed), generates translation mappings, and merges Rust + Lean call graphs into a unified atom file with cross-language dependency edges.
- `translate` command: generate Rust ↔ Lean translation mappings from pre-generated atom files and `functions.json`.
- `listfuns` command: run `lake exe listfuns` in a Lean project to produce `functions.json`.
- Three-strategy translation matching (in priority order):
  1. `rust-qualified-name` -- exact match via Charon-derived qualified names.
  2. `file+display-name` -- same source file + matching base method name.
  3. `file+line-overlap` -- same source file + overlapping line ranges.
- Flexible input modes: accept pre-generated JSON files, project paths, or a mix of both.
- Parallel extraction when both `--rust-project` and `--lean-project` are given.
- Auto-install for `probe-rust` (via `cargo install`) and `probe-lean` (clone + `lake build`).
- Schema 2.0 metadata envelopes for merged atoms (`probe-aeneas/extract`) and translations (`probe/translations`).
- Project documentation: README, usage guide, schema specification, and changelog.

[Unreleased]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.3.2...HEAD
[0.3.2]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/releases/tag/v0.1.0
