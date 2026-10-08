# Manual Testing on Real Projects

`cargo test` runs on synthetic inputs only (see `TESTING.md`). This page
describes the checks on real Aeneas projects. The repo does not store their
output.

## Test projects

These three projects are the end-to-end test targets for `extract`:

| Project | Path through `extract` |
|---|---|
| [`curve25519-dalek-lean-verify`](https://github.com/Beneficial-AI-Foundation/curve25519-dalek-lean-verify) | Manifest path. `Curve25519Dalek/translation.json` exists, so a positional run gives probe-rust `--translation` and matches by `charon-def-id`. |
| [`SparsePostQuantumRatchet-verify`](https://github.com/Beneficial-AI-Foundation/SparsePostQuantumRatchet-verify) (spqr) | Manifest path. `translation.json` is at the project root, and its `types` and `trait_impls` arrays name the auxiliary defs. |
| [`SymCRust-lean`](https://github.com/Zhang-Liao/SymCRust-lean) | Heuristic path. The repo has no `translation.json` and no committed `functions.json`. |

## Commands

The positional form runs the full pipeline. It reads `aeneas-config.yml`,
runs probe-rust and probe-lean, and applies `charon.cargo_args` for `cfg`
classification:

```bash
probe-aeneas extract <project> --with-public-api --output /tmp/out.json
```

The merge-only form reuses extractor output and skips both extractors. Use it
for fast A/B runs. It does no `cfg` classification, so it reports no
`cfg-inactive` atoms. Omit `--translation` for a project without a
manifest. SymCRust-lean commits no extractor output, so only the positional
form works there:

```bash
probe-aeneas extract \
  --rust <project>/.verilib/probes/rust_extract.json \
  --lean <project>/.verilib/probes/lean_extract.json \
  --functions <project>/functions.json \
  --translation <path-to>/translation.json \
  --output /tmp/out.json
```

## A/B check

If a change touches translation, merge or enrichment, compare the output of
the `main` binary and the branch binary on the same inputs:

1. Build `main` in a separate worktree with its own `CARGO_TARGET_DIR`.
2. Run both binaries on the same project with the same command.
3. Compare `.data` with `jq`. The envelope `timestamp` always differs.
4. Make sure that only the intended `(atom, field)` pairs changed, and in the
   intended direction.

Run the check on a manifest-path project and on the heuristic-path project,
because they use different code.

## Latest record (2026-10-06)

Positional run on curve25519-dalek-lean-verify (commit `26d49052`) with
`--with-public-api`, probe-rust 0.12.0, probe-lean 0.16.0 and probe-aeneas
0.21.0:

- Rust: 672 atoms (450 functions, 222 external stubs), 143 with
  `is-public-api: true`. probe-rust sets `is-public-api` only with
  `--with-public-api`.
- Lean: 2352 atoms, 93 with `status-origin: "kernel-taint"`.
- Translations: 189, all `exact` through the `charon-def-id` join.
- Rust statuses: 183, all marked `status-origin: "translation"`, none
  `transitively-verified`. 6 translated functions have no primary spec and no
  status.
- Untracked: 171 Rust atoms (137 `cfg-inactive`, 29 `non-library-target`,
  5 `trait-signature`).
- Hub 0.5.0 `probe enrich` accepts the output and leaves `data` unchanged.

On the manifest path, probe-rust writes SCIP-style `rust-qualified-name`
values. For impl methods, these do not match the Charon-style `rust_name`
values in `functions.json`. A run without `--translation` therefore matches
few functions through strategy 1 (`rust-qualified-name`), and most matches
come from the file-based strategies.
