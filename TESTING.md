# Testing

## Quick start

```bash
cargo test
```

## Test layers

| Layer | Location | Requires |
|-------|----------|----------|
| Unit tests | `#[cfg(test)]` modules in `src/` | Nothing |
| Integration test | `tests/extract_check.rs` | Nothing |

All tests run without external tools or `#[ignore]`.

## Unit tests

The tests in `src/translate.rs` cover:

- Rust name normalization (generics stripping, ref removal)
- Line range parsing and overlap detection
- Translation strategy: file-based display-name, line overlap, Rust qualified name
- Duplicate mapping prevention
- One-to-one primary mapping wins over secondary
- Lean atom double-claim prevention
- `build_functions_rust_names` extraction from functions.json

The ADR-006 end-to-end regressions live in `src/extract.rs` (`copied_status_on_leaf_is_not_promoted` and the tests after it). They write small Rust and Lean envelopes to a temp dir and run the merge, metadata and enrichment phases. They cover leaf and caller laundering, demotion of an imported `transitively-verified`, a copied `trusted` not shielding callers, non-exact confidence and method in both records, `--skip-enrich`, dotted endpoints, a `kernel-taint`-marked Lean spec, and rejection of probe-lean < 0.16.0 input (at merge and, through `run_extract`, before any other work). `probe_lean_contract_version_matches_hub_gate` pins the runner's probe-lean floor against the hub validator. The `accept_probe_lean_*` tests in `src/extract_runner.rs` use fake binaries to show that a pre-contract or unreadable probe-lean is not reused, and that the rejection names the cause. `find_release_asset_url_skips_releases_below_floor` shows that the pre-built download never picks a release tagged below 0.16.0. The `install_extracted_*` tests show that a downloaded binary below 0.16.0 is not installed, and that a 0.16.0 binary and its lib dir are.

Run only unit tests: `cargo test --lib`

## Integration tests

`library_extract_on_synthetic_fixture` in `tests/extract_check.rs` runs
`run_extract` through the library API on the synthetic fixture in
`tests/fixtures/mini/`. The files are hand-written: `rust.json` is stamped
as probe-rust 0.12.0 output, `lean.json` as probe-lean 0.16.0 output, and
`functions.json` lists the translations. The test needs no external tools.
It checks the envelope fields and one outcome for each Rust function:

- `add`: an `exact` match through `rust-qualified-name`, a copied `verified`
  status marked `status-origin: "translation"`, and `is-public-api` passed
  through.
- `Point::scale`: a `file-and-name` match through `file+display-name`. Its
  primary spec carries `status-origin: "kernel-taint"`. The marker stays on
  the Lean spec, and the Rust copy is marked `"translation"`.
- `helper`: a `file-and-lines` match through `file+line-overlap`. The Lean
  def has no spec, so the Rust atom gets no status and stays tracked.
- `Point::new` in `src/point.rs`, `src/other.rs` and `src/shadow.rs`: the
  three share one `rust-qualified-name`. The `functions.json` source file
  picks the one in `src/point.rs` (`exact-disambiguated`), and the others stay
  unmatched. The matched atom key sorts between the other two, so the test
  fails if the match takes the first or the last candidate.
- `untranslated`: no match, no status, and `untracked: false`.

The test also checks that enrichment runs: the unmarked Lean spec
`Mini.add_spec` becomes `transitively-verified`.

The fixture has no `translation.json`, so strategy 0 (`charon-def-id`) does
not run. The unit tests in `src/translate.rs` cover it. Real-data checks run
outside the repo on the canonical test projects (see `docs/testing.md`).

## CI

`.github/workflows/ci.yml` runs on push/PR to `main`:

1. **Format** -- `cargo fmt --all -- --check`
2. **Clippy** -- `cargo clippy --all-targets -- -D warnings`
3. **Test** -- `cargo test --verbose`

Cargo fetches the `probe` dependency from its git tag (see `Cargo.toml`).

## Adding tests

- **Unit tests:** add to the `#[cfg(test)] mod tests` block in `src/translate.rs` (or create one in another module).
- **Integration tests:** add to `tests/extract_check.rs`. Read the output envelope as `serde_json::Value`.
- **Fixture changes:** edit the files in `tests/fixtures/mini/` by hand. Keep them small: add an atom only for a new outcome that the test checks.

## See also

- `docs/testing.md` -- manual testing on real projects
