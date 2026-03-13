# Changelog

All notable changes to probe-aeneas are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

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
- Schema 2.0 metadata envelopes for merged atoms (`probe/merged-atoms`) and translations (`probe/translations`).
- Project documentation: README, usage guide, schema specification, and changelog.

[Unreleased]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/releases/tag/v0.1.0
