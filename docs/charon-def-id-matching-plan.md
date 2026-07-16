# Phase 2 plan: charon-`def_id` integer join for Rust↔Lean matching

Status: WS1 (join) and WS2 (provenance gate) implemented on branch
`feat/precise-rqn-matching` (stacked on `main`, which has the merged
`function_source` seam + manifest producer, PR #39). WS3 (probe-rust emitting
`charon-def-id`) is the remaining cross-repo work; until it ships the join is a
forward-compatible no-op.

## Context / why

probe-aeneas matches each Rust function to its Lean translation. Today this uses
a three-strategy **name** matcher (`translate.rs`): normalize the Rust
`rust-qualified-name` (RQN), join it to `functions.json`/`translation.json`
`rust_name`, with a two-pass deferral + `is_hidden` heuristic + line
disambiguation to resolve collisions.

Those collisions are almost entirely an **artifact of lossy name normalization**.
`normalize_rust_name` strips generic args and references, so genuinely-distinct
impls collapse together:
- owned vs borrowed (`AddAssign<GF16>` vs `AddAssign<&GF16>`),
- borrow-pattern variants (`Shared0` vs `SharedA`/`SharedB`).

The only *genuine* one-charon-item → many-Lean-defs fan-out is **loop helpers**,
which `translation.json` already marks (`loop` field, shared `def_id`).

### The key realization

`translation.json`'s `def_id` **is the charon `FunDeclId`** (proven: a loop
family shares one `def_id` because Aeneas generates the helpers from a single
charon `FunDecl`). probe-rust already locates each atom's charon `FunDecl` by
source span (the merged single-candidate-validation logic). So if probe-rust
emits that `FunDeclId`, probe-aeneas can join Rust↔Lean by **integer equality** —
precise for free, no name normalization, no `is_hidden`, no two-pass.

Rejected alternatives (see history in memory):
- **Join on the name string** — there is no canonical charon name *string*;
  charon stores a *structured* name (idents + `Impl` references), and Aeneas
  (`translation.json.rust_name`) and probe-rust (RQN) render it differently.
  Reconciling the two renderings is what the fuzzy matcher does; unavoidable
  unless both tools adopt one rendering (cross-repo, fragile).
- **Tighten `normalize_rust_name`** — works, but delicate string
  canonicalization; made unnecessary by the id-join for manifest projects.

## Coverage evidence (spqr)

Measured over **in-scope translated functions** (not all local atoms):
- span-coverage of local Rust atoms ⊇ current name matches: **all 236
  name-matched atoms are span-covered, +3 gained, 0 lost, 0 fallback needed**.
- The 173 "uncovered" local atoms are **correctly** unmatched: 118 `#[cfg(test)]`
  tests, 23 out-of-scope (`is-disabled`, e.g. `encoding::gf::accelerated::*` in
  the aeneas-config `exclude` list), 32 other non-targets (bare trait methods
  `Decoder::new`/`Encoder::encode_bytes`, `into_pb_test` helpers). None is a real
  missed target; none currently carries a translation.

So the id-join is a **strict superset** of name matching for real targets.
Coverage evidence is spqr-only — recheck on a second manifest project before
deleting any fallback.

## The safety principle (shapes everything)

The id-join is only correct when probe-rust's `charon-def-id`s come from the
**same charon version** that produced `translation.json`. A different version →
integers point at different functions → **silent wrong mappings**. (Observed:
`data/charon.llbc` is charon 0.1.174 while `translation.json` is 0.1.217 — a
stale cache; `def_id` 439 = `core::fmt::rt::new_display` in the llbc vs
`GF16.div_impl` in the manifest.)

Therefore the join must be **provenance-gated**: trust `charon-def-id` only when
its charon version matches `translation.json.charon_version`; otherwise fall back
to name matching. This degrades gracefully instead of corrupting.

**Version equality is best-effort provenance, not proof of an identical run.**
Two runs of the *same* charon version with different cargo flags, sources, or
commits can still assign different `def_id`s, which the version check cannot
detect. The durable fix is to gate on a charon commit hash or a digest of the
exact LLBC Aeneas consumed (see
`docs/upstream-issues/charon-expose-commit-and-versioned-releases.md` and
`docs/upstream-issues/aeneas-persist-consumed-llbc.md`). Until then the version
gate is the pragmatic guard: it catches the common stale-cache case and degrades
to name matching otherwise.

## Workstreams

### WS1 — probe-aeneas: the join (pure-local, ships first)
- Build `def_id → primary lean_name` from the `FunctionRecord`s (primary =
  `def_id` present, `is_loop_artifact != Some(true)`; deterministic tie-break).
- Add `strategy_charon_def_id` (runs **first**): for each Rust atom carrying a
  `charon-def-id`, bind to `primary(def_id)` if the Lean atom exists, respecting
  `matched_rust`/`matched_lean` (P11). `confidence: "exact"`,
  `method: "charon-def-id"`.
- **Provenance gate**: only run when the atoms' charon version matches
  `translation.json.charon_version`; else skip.
- Existing three strategies run after, unchanged, as fallback. They must **not**
  chase test/OOS atoms (they already don't — those have no manifest entry).
- **Forward-compatible = no-op today**: no atom carries `charon-def-id` yet, so
  output is byte-identical. A/B spqr+dalek to confirm. Lights up when probe-rust
  emits the field.
- Docs (P12): add `charon-def-id` method to `../probe/kb/engineering/properties.md`
  P12, `docs/SCHEMA.md`, `docs/architecture.md`, `docs/USAGE.md`.

### WS2 — probe-aeneas: `ensure_charon_llbc` provenance (safety gate)
- Compare the `.llbc`'s `charon_version` to `translation.json.charon_version`.
  On mismatch: regenerate, or (if impossible with the installed charon) disable
  the id-join and log why. Never feed mismatched ids to the join.
- Investigate where Aeneas persists *its* `.llbc` and whether probe-rust can
  consume that exact file (one shared run — the durable fix). `ensure_charon_llbc`
  is in `extract_runner.rs`.

### WS3 — probe-rust: emit `charon-def-id` (cross-repo)
- File an issue: surface the resolved `FunDeclId` (from probe-rust's existing
  span→`FunDecl` match) as a `charon-def-id` atom field, plus the charon version
  used (for provenance-gating).
- Implement + release. probe-aeneas depends on the field but degrades gracefully
  without it (WS1).

## Sequencing (with a de-risking prototype before any cross-repo work)

1. **WS1** (join, forward-compatible). A/B byte-identical. Commit.
2. **Prototype-validate without probe-rust**: a script/test that assigns each
   spqr Rust atom its `def_id` by span-matching `translation.json`'s own spans,
   injects it into `rust_extract.json`, runs the new join. Acceptance: ≥236
   matches, the 3 gains, **0 regressions**, each gain manually confirmed. Proves
   the join + coverage before asking probe-rust for anything.
3. **WS2** (provenance gate) — independently useful (stale cache is a latent bug).
4. **WS3** (probe-rust emit) — file issue, implement, release.
5. **Activate + real A/B** on spqr; recheck coverage on a second manifest project.
6. **Later** (with legacy removal, see memory `legacy-docstring-scraper-removal`):
   once id-join is proven across projects, the fuzzy matcher /
   `normalize_rust_name` / two-pass remain only for no-manifest projects and are
   deleted when those go.

## Verification
- WS1: A/B spqr (manifest) + dalek (legacy) → byte-identical (no-op).
- Prototype: ≥236/+3/0-regression + spot-check the 3 gains.
- Post-WS3: full A/B; negative test — feed a mismatched `.llbc`, confirm the
  provenance gate *disables* the join.
- Throughout: `cargo fmt` / `clippy --all-targets -D warnings` / `test`.

## Explicitly NOT doing
- **Not** tightening `normalize_rust_name` / building a reference-signature
  canonicalizer (id-join makes it unnecessary for manifest projects; legacy keeps
  the current matcher).
- **Not** matching test/OOS code — no `translation.json` entry; no fallback
  should reach for them.
- **Not** joining on `def_id` without the provenance gate.

## Artifacts
- `tests/fixtures/rqn_pairs.json` (commit on this branch) — 435 matched pairs +
  20 must-stay-distinct pairs. Primarily a **normalizer oracle**: it pins the RQN
  naming noise so a future `normalize_rust_name` change is deliberate. It also
  exercises the id-join's *mechanics* (distinct ids map to distinct Lean defs;
  the split the name matcher cannot make), but note this proves integer-lookup
  behavior on synthetic ids, **not** that real probe-rust `charon-def-id`s equal
  the manifest's `def_id`s — that end-to-end equality can only be validated
  against a real manifest once probe-rust emits the field (WS3).
