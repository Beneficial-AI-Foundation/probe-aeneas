# Testing

## Quick start

```bash
cargo test
```

## Test layers

| Layer | Count | Location | Requires |
|-------|-------|----------|----------|
| Unit tests | 13 | `src/translate.rs` (`#[cfg(test)]` module) | Nothing |
| Integration tests | 5 | `tests/extract_check.rs` | Nothing |

All tests run without external tools or `#[ignore]`.

## Unit tests

13 tests in `src/translate.rs` covering:

- Rust name normalization (generics stripping, ref removal)
- Line range parsing and overlap detection
- Translation strategy: file-based display-name, line overlap, Rust qualified name
- Duplicate mapping prevention
- One-to-one primary mapping wins over secondary
- Lean atom double-claim prevention
- `build_functions_rust_names` extraction from functions.json

Run only unit tests: `cargo test --lib`

## Integration tests

5 tests in `tests/extract_check.rs`:

| Test | What it checks |
|------|---------------|
| `example_merged_json_has_valid_structure` | Validates `MergedEnvelope` top-level fields: schema (`probe-aeneas/extract`), schema-version, tool, inputs array, timestamp, data object |
| `example_merged_json_atoms_have_required_fields` | All atoms have `probe:` key prefix, non-empty `display-name`, `kind`, and `language` |
| `example_merged_json_rust_atoms_have_translations` | Rust atoms have `is-disabled` field; at least some have `translation-name` |
| `micro_fixture_structural_check` | Loads the `aeneas_micro` fixture from `probe-extract-check` as `AtomEnvelope` and runs `check_all` (skips gracefully if fixture not found) |
| `library_extract_with_pregenerated_json` | Runs `run_extract` via the library API with pre-generated example files (`examples/rust_*.json`, `examples/lean_*.json`, `examples/functions.json`). Validates the merged output has both Rust and Lean atoms with translation metadata. |

The `library_extract_with_pregenerated_json` test exercises the full merge
pipeline (load atoms, generate translations, merge, write output) without
needing any external tools -- it uses the pre-generated example JSON files
shipped in `examples/`.

## CI

`.github/workflows/ci.yml` runs on push/PR to `main`:

1. **Format** -- `cargo fmt --all -- --check`
2. **Clippy** -- `cargo clippy --all-targets -- -D warnings`
3. **Test** -- `cargo test --verbose`

The CI checks out the sibling `probe` repo alongside for both the
`probe` build dependency and the `probe-extract-check` dev-dependency.

## Adding tests

- **Unit tests:** add to the `#[cfg(test)] mod tests` block in `src/translate.rs` (or create one in another module).
- **Integration tests:** add to `tests/extract_check.rs`. For `MergedEnvelope` tests, use `serde_json::Value`; for `AtomEnvelope` tests, use `probe_extract_check`.
- **New example JSON:** place in `examples/` and add corresponding test assertions.

## See also

- `docs/testing.md` -- manual testing record with example commands
