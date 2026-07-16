# DRAFT (local, not filed) — target repo: AeneasVerif/aeneas

## Title

Persist the charon LLBC that produced `translation.json` (so downstream tools don't re-run charon)

## Problem

Aeneas runs charon to obtain an LLBC, consumes it, and emits `translation.json`
(with `--emit-json`). The LLBC is then discarded. Downstream tooling that needs
to correlate the manifest back to the Rust source — e.g. probe-aeneas joining
Rust functions to their Lean translations by charon `FunDeclId` — must run
charon a **second time** to recover that LLBC.

Running charon twice is wasteful (charon is a full rustc driver over the whole
crate) and, worse, **not reliably reproducible**: the second run must match the
first in charon version, rust toolchain, and args, or the `def_id`s
(`FunDeclId`s) diverge and any id-based join silently points at the wrong
functions.

Concrete evidence (SparsePostQuantumRatchet-verify):
- `translation.json` is committed and reports `charon_version: 0.1.217`.
- Its source LLBC is **not** persisted (`*.llbc` is gitignored); the only local
  `data/charon.llbc` is a stale 0.1.174 artifact from an unrelated earlier run.
- `def_id` numberings differ between 0.1.174 and 0.1.217 (e.g. `mac_ct` is 135 in
  the 0.1.174 LLBC vs 82 in the 0.1.217 manifest), so a join across them is wrong.

There is currently no way to recover the exact LLBC behind a shipped manifest.

## Proposal (preferred)

Emit the exact LLBC Aeneas consumed **in the same run** as `--emit-json`, to a
discoverable location, so consumers can use *that file* instead of regenerating:

- Write it to a fixed path (e.g. next to `translation.json`), and/or
- Record a reference in the manifest: `llbc_path` and/or `llbc_sha256`.
- Guarantee `translation.json` and the LLBC are emitted atomically from one run
  (the failure mode above is precisely a manifest and an LLBC from different runs).

Because `def_id` in `translation.json` *is* the charon `FunDeclId` from that
LLBC, any consumer reading the same file gets matching ids by construction — no
version-matching, no toolchain install, no provenance gating needed.

Size note: LLBCs are large (~18 MB for spqr). Mitigations: gzip (compresses
well), or emit to a build-artifact directory rather than committing to git.

## Fallback (if persisting the LLBC is undesirable)

Record full charon provenance in `translation.json` so consumers can fetch the
exact charon deterministically:

- `charon_commit` (git sha) — today the manifest has `aeneas_version` as a
  commit but `charon_version` only as `0.1.217`, which maps to a *range* of
  commits.
- `rust_toolchain` (the nightly channel charon required, e.g. `nightly-2026-06-01`).

This still requires a second charon run + a heavy toolchain install, so it is
strictly weaker than persisting the LLBC. (Depends on the companion charon-side
change — see `charon-expose-commit-and-versioned-releases.md`.)

## Consumer impact (probe-aeneas / probe-rust)

Small, and already almost in place:
- probe-rust's charon enrichment already takes an LLBC *path*
  (`enrich_atoms_with_charon_names(atoms, llbc_path)`); probe-aeneas already
  reuses `data/charon.llbc` when present.
- With an Aeneas-emitted LLBC, probe-aeneas's `ensure_charon_llbc` becomes
  "locate the Aeneas LLBC" instead of "generate it", and can drop the
  regenerate-on-stale logic (WS2). The provenance gate in the matcher (WS1) is no
  longer load-bearing for manifest projects, since ids match by construction.

## Links

- probe-aeneas plan: `docs/charon-def-id-matching-plan.md` (WS2 "durable fix")
- probe-aeneas consumer PR: Beneficial-AI-Foundation/probe-aeneas#41
- probe-rust emitter PR: Beneficial-AI-Foundation/probe-rust#21
