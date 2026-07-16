# DRAFT (local, not filed) — target repo: AeneasVerif/charon

## Title

Expose charon's git commit in `charon version` / the LLBC, and make releases version-addressable

## Context

This is a companion to the Aeneas-side ask
(`aeneas-persist-consumed-llbc.md`). It is only needed for the *fallback* path —
where a downstream tool must fetch the exact charon that produced a given
manifest, rather than being handed the LLBC directly. If Aeneas persists the
consumed LLBC, this becomes unnecessary.

## Problem

Given only a charon version string (`0.1.217`, as recorded in an Aeneas
`translation.json`), there is no deterministic way to obtain the matching charon
binary:

1. `charon version` prints only `0.1.217` — no git commit. The LLBC embeds only
   `charon_version` too. So a version maps to a *range* of commits, and `def_id`
   assignment could in principle change within a version.
2. GitHub releases are tagged by date (`nightly-2026.06.26`) and by
   `build-<timestamp>-<sha>`, but **not** by `0.1.x` version. To find which
   release ships `0.1.217` we had to download candidate nightlies and run
   `charon version` (a binary search over dates).

## Proposal

- Include the git commit in `charon version` output and as a `charon_commit`
  field in the LLBC (alongside `charon_version`).
- Make releases version-addressable: either tag/name release assets by version,
  or publish a small `version → {tag, commit, rust_toolchain}` index.
- Surface the required rust toolchain (already in the release's `rust-toolchain`)
  in a machine-readable form so a consumer can install it without unpacking.

## Why it helps

Turns "reproduce Aeneas's charon run" from a fragile date binary-search into a
deterministic lookup. Still second-best to being handed the LLBC (which needs no
charon at all downstream), but it makes the fallback tractable.

## Links

- Aeneas-side (preferred) ask: `aeneas-persist-consumed-llbc.md`
- probe-aeneas plan: `docs/charon-def-id-matching-plan.md`
