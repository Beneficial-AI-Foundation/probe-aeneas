# Code Review: `feat/precise-rqn-matching`

Multi-model adversarial review of the `charon-def-id` join and provenance gate.
Reviewers: Claude Opus 4.7, Grok 4.5, GPT-5.6 Sol.

Date: 2026-07-16

---

## Consensus findings (2+ models agree)

### Act on

#### 1. Version equality != same charon run (all 3 models, critical)

The provenance gate compares version strings, but same-version runs with
different flags/sources/commits can assign different `def_id`s. Docs claim
"same charon run" which is stronger than what the code enforces.

**Recommendation:** Stop claiming "same charon run" in docs. Acknowledge this
is best-effort provenance, not full identity. Long-term: use an LLBC digest or
charon commit hash.

#### 2. `def_id` namespace mixing across `functions`/`globals`/`trait_impls` (Opus 4.7 + Sol, warning)

`records_from_manifest` flattens all three into one vector;
`build_def_id_to_primary_lean` indexes them together. If charon uses separate
ID counters per kind, integers can collide across kinds.

**Recommendation:** Restrict `build_def_id_to_primary_lean` to records from
`manifest.functions` only, or document the upstream invariant that IDs are
globally unique.

#### 3. Cache fails open when version is unreadable (all 3 models, warning)

`(cached=None, expected=Some(...))` reuses the cache. If the LLBC format
changes or `read_llbc_charon_version` hits a short read, the stale cache is
silently trusted.

**Recommendation:** Treat `(None, Some(_))` as stale -- regenerate.

#### 4. `read_llbc_charon_version` is fragile (all 3 models, warning)

Exact substring match `"charon_version":"` breaks on pretty-printed JSON,
whitespace, or format changes. `File::read` can short-read.

**Recommendation:** Use `read_to_end` with a size cap, or a lightweight serde
parse. Add a test for spaced JSON.

#### 5. Missing tests for coupling-invariant edge cases (all 3 models, warning)

No test for:
- `charon-def-id` present without `charon-version`
- `charon-def-id` as a JSON string
- Two Rust atoms with the same `def_id`

**Recommendation:** Add tests for these three cases.

#### 6. Docs say "same charon run" but code checks version string (all 3 models, warning)

Systematic overclaim across SCHEMA.md, USAGE.md, architecture.md, and plan
doc. Plan status still says "planned."

**Recommendation:** Fix doc language; update plan status.

#### 7. No tests for WS2 stale-cache decision logic (Grok + Opus, warning)

Only `read_llbc_charon_version` is unit-tested, not the
mismatch/match/unreadable decision branches in `ensure_charon_llbc`.

**Recommendation:** Add integration-style tests or at minimum unit tests for
the decision matrix.

### Consider

#### 8. Vacuous assertion in `gated_off_without_manifest_version` test (Opus 4.7 critical, Grok implicit, warning)

`mappings` is empty so `.all()` is trivially true.

**Recommendation:** Seed fixture so a name strategy binds, then assert
`len == 1` and method != `charon-def-id`.

#### 9. `annotate` only overlays `manifest.functions`, not globals/trait_impls (Opus 4.7, warning)

Override path silently disables id-join for those records.

**Recommendation:** Extend or document.

#### 10. Two Rust atoms with same `charon-def-id`: first wins silently (Opus 4.7 + Grok, warning)

Second falls to name matcher without warning.

**Recommendation:** Add an `eprintln!` diagnostic.

#### 11. RQN fixture overclaims "join-correctness oracle" (Grok + Sol, warning)

Tests only prove integer lookup works, not real-world id equality.

**Recommendation:** Rename claims or add a real-manifest integration test.

#### 12. `Option<&str>` could be a newtype for type safety (all 3, nit)

Low priority given small surface area.

#### 13. Empty-string charon version would pass the gate (Opus 4.7, nit)

**Recommendation:** Add `.filter(|s| !s.is_empty())`.

#### 14. Missing CHANGELOG `[Unreleased]` entry (Opus 4.7, warning)

Required by workspace rules before commit.

#### 15. Schema version should bump 2.0 -> 2.1 (Opus 4.7, nit)

Per SCHEMA.md's own policy. Pre-existing pattern violation, but this branch
adds new optional fields.

### Noted (no action needed)

- **SCHEMA table broken by blockquote insertion** (Grok) -- nit, easy fix.
- **`unwrap_or("?")` unreachable in stale branch** (Grok) -- dead code, harmless.
- **`matched_rust.contains()` check is redundant** (Opus 4.7) -- future-safety, add comment.

### Dismissed

- **Sol's claim that specific fixture matched pairs are "visibly wrong"** (e.g.
  `Error::from` pairing) -- these are `From` trait impls where the same Rust
  function has different rendering styles between probe-rust and Aeneas.
  Exactly the naming noise the fixture documents. Not actual data errors.

---

## Bottom line

The core join logic is correct and well-tested for its main paths. The biggest
real risks are:

1. **Doc overclaim** -- "same charon run" language should be softened to "same
   charon version (best-effort provenance)"
2. **`def_id` namespace mixing** -- restrict to `manifest.functions` until
   cross-kind uniqueness is confirmed upstream
3. **Cache fail-open** -- treat unreadable cache version as stale when an
   expected version exists

None of these cause wrong behavior today (the join is a forward-compatible
no-op until probe-rust emits the field), but they are latent risks that should
be fixed before the join goes live.
