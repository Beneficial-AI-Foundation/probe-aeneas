# Changelog

All notable changes to probe-aeneas are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.9.2] - 2026-04-16

### Fixed
- **Spec-based `verification-status` for Rust atoms**: translated functions
  without a proven spec are now correctly marked `"unverified"` instead of
  inheriting `"verified"` from the Lean definition. The status is derived
  from the primary spec theorem (via `primary-spec` extension or `_spec`
  naming convention). `"trusted"` and `"failed"` statuses are preserved
  as-is. ([#5](https://github.com/Beneficial-AI-Foundation/probe-aeneas/issues/5))

## [0.9.1] - 2026-04-16

### Fixed
- **`setup` installs rust-analyzer for the default toolchain**: after
  delegating to `probe-rust setup`, `probe-aeneas setup` now runs
  `rustup component add rust-analyzer` directly as a fallback (probe-rust
  setup previously only warned about a missing rust-analyzer without
  installing it).
- **`extract` installs rust-analyzer for the project's toolchain**: before
  running `probe-rust extract`, probe-aeneas now detects the Rust project's
  toolchain from `rust-toolchain.toml` or `rust-toolchain` and ensures
  rust-analyzer is installed for that specific toolchain via
  `rustup component add rust-analyzer --toolchain <channel>`. This fixes
  Docker environments where the project uses a nightly toolchain (e.g.
  `nightly-2026-03-23`) and rust-analyzer was only installed for the
  default toolchain.

## [0.9.0] - 2026-04-14

### Added
- **Cargo workspace support**: `probe-aeneas extract <project>` now handles
  projects where `crate.dir` in `aeneas-config.yml` points to a source
  subdirectory without its own `Cargo.toml` (e.g. libsignal with
  `crate.dir = "rust"` and a workspace `Cargo.toml` at the project root).
  Uses `cargo metadata --no-deps` to resolve the target member crate from
  `crate.name`, `charon.package`, or `-p` in `charon.cargo_args`.
- **`start_from_pub` and `include` charon config forwarding**: the `charon`
  section in `aeneas-config.yml` now supports `start_from_pub: true` (passed
  as `--start-from-pub`) and `include` lists (passed as `--include <name>`).
  Previously these fields were silently ignored, causing incomplete LLBC
  generation for projects like libsignal that rely on `start_from_pub`
  instead of explicit `start_from` entries for the target crate.

### Changed
- **`setup` command delegates to `probe-rust setup`**: after installing
  probe-rust, `probe-aeneas setup` now runs `probe-rust setup` to install
  probe-rust's own dependencies (rust-analyzer, scip).

## [0.8.0] - 2026-04-09

### Added
- **`--with-public-api` flag** on `extract`: passes `--with-public-api` to
  `probe-rust extract` so that `cargo public-api` computes accurate
  `is-public-api` values on Rust atoms. Requires `cargo-public-api` and a
  nightly Rust toolchain. Off by default.

## [0.7.0] - 2026-04-07

### Added
- **`verification-status` on Rust atoms**: Rust atoms with a Lean translation
  now carry `verification-status` propagated from the corresponding Lean atom.
  Values are `"verified"`, `"unverified"`, `"trusted"`, or `"failed"`. Absent
  when no translation exists. Enables uniform verification queries across both
  languages (same key as probe-verus uses on Rust atoms).
- **`is-public-api` pass-through**: the `is-public-api` field from probe-rust
  (distinguishing crate-level public API from item-level `pub` visibility) is
  now preserved in the merged output. More selective than `is-public` — a
  `pub fn` inside a private module has `is-public: true` but
  `is-public-api: false`.
- **`trusted` verification status** for Lean atoms: probe-lean v0.4.5 now
  classifies axioms and `*External.lean` declarations as
  `verification-status: "trusted"` with a `trusted-reason` field (`"axiom"`
  or `"external"`). These are passed through in the merged output.
- **Public API coverage summary** printed during `extract`: shows how many
  public API functions are verified, unverified, trusted, or not in scope.

### Changed
- Schema version bumped to 2.6; SCHEMA.md documents all new fields.

## [0.6.0] - 2026-04-04

### Added
- **`setup` command**: installs external tool dependencies (probe-rust and
  charon) into their managed directories. Use `--status` to check which
  tools are installed. probe-lean is version-matched per project and
  auto-installed during `extract`.

### Fixed
- **Charon auto-install on first run**: when charon is not yet installed,
  `ensure_charon_llbc` now builds it from source automatically (into
  `~/.probe-rust/tools/`, the same managed directory probe-rust uses) and
  then pre-generates the LLBC with the full `aeneas-config.yml` settings.
  Previously, running `probe-aeneas extract` on a machine without charon
  already installed would fail with a hard error before probe-rust had a
  chance to auto-install it.

### Changed
- **Tool installation refactored into `setup.rs`**: installation logic for
  probe-rust and charon is now centralized in the `setup` module. Both the
  `setup` command and `extract` (as fallback) call the same functions.

## [0.5.0] - 2026-04-02

### Added
- **Charon config forwarding from `aeneas-config.yml`**: the `charon` section
  (`cargo_args`, `start_from`, `exclude`, `opaque`, `package`, `preset`) is now
  parsed and used to pre-generate the Charon LLBC file before `probe-rust`
  runs. Previously, `probe-rust --with-charon` ran charon with only
  `--preset aeneas`, missing project-specific cargo args (e.g.
  `--no-default-features --features alloc,zeroize`) and filter lists, causing
  LLBC generation to silently fail on projects like curve25519-dalek.
- **`CharonConfig` struct** in `extract.rs` for typed parsing of the `charon`
  YAML section.
- **`ensure_charon_llbc` pre-flight** in `extract_runner.rs`: runs charon with
  the full config and caches the LLBC at `<rust_project>/data/charon.llbc`
  so `probe-rust` reuses it.

## [0.4.0] - 2026-03-31

### Added
- **Repo-relative `code-path` for Rust atoms**: when running `probe-aeneas extract <project>` with `crate.dir` pointing to a subdirectory (e.g. `curve25519-dalek`), Rust atom `code-path` values are now prefixed with the crate directory. This produces repo-relative paths like `curve25519-dalek/src/backend/mod.rs` instead of crate-relative `src/backend/mod.rs`, matching file paths stored when the full repository is ingested.
- **`.verilib/probes/` output directory**: the default output path for `extract` now follows the probe ecosystem convention, writing to `<project>/.verilib/probes/aeneas_<pkg>_<ver>.json`. The `-o` flag still overrides.
- **Intermediate extractor outputs saved alongside merged output**: when extracting from a project path, probe-rust and probe-lean intermediate JSONs are written to `<project>/.verilib/probes/rust_extract.json` and `lean_extract.json` (previously written to temp files in `/tmp/`).

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

[Unreleased]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.9.1...HEAD
[0.9.1]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Beneficial-AI-Foundation/probe-aeneas/releases/tag/v0.1.0
