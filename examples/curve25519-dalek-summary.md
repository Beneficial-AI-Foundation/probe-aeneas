# Verification Statistics: curve25519-dalek 4.1.3 (Aeneas)

Generated from `examples/aeneas_curve25519-dalek_4.1.3.json` produced by
`probe-aeneas extract`.

All queries below use `jq` on the extract JSON:

```bash
FILE=examples/aeneas_curve25519-dalek_4.1.3.json
```

---

## 1. Public functions verified

**167 / 252** `pub fn` Rust functions are verified or trusted (66.3%):
167 verified + 0 trusted.

These are Rust functions with `is-public: true` that have a Lean translation
whose `verification-status` has been propagated onto the Rust atom.

The trusted count is 0 because `"trusted"` applies to Lean-side atoms
(axioms and `*External.lean` definitions — see sections 3 and 4). In Aeneas
projects, the trust base lives in the Lean layer; Rust functions translate to
verified Lean definitions that *depend on* trusted atoms, but the translation
target itself is verified.

```bash
# Count
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.language == "rust")] | length' "$FILE"
# => 252

# Count verified
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.language == "rust" and .value."verification-status" == "verified")] | length' "$FILE"
# => 167

# Count verified or trusted
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.language == "rust" and (.value."verification-status" == "verified" or .value."verification-status" == "trusted"))] | length' "$FILE"
# => 167

# List verified or trusted pub fn
jq -r '.data | to_entries[] | select(.value."is-public" == true and .value.language == "rust" and (.value."verification-status" == "verified" or .value."verification-status" == "trusted")) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

---

## 2. Public API functions verified

**101 / 147** public API functions are verified or trusted (68.7%):
101 verified + 0 trusted.

Public API = functions reachable by external crate consumers
(`is-public-api: true`). More selective than `is-public` — a `pub fn`
inside a private module has `is-public: true` but `is-public-api: false`.

Trusted is 0 for the same reason as above: the 115 trusted atoms (sections
3 and 4) are Lean declarations, not Rust translation targets.

```bash
# Count
jq '[.data | to_entries[] | select(.value."is-public-api" == true)] | length' "$FILE"
# => 147

# Count verified
jq '[.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" == "verified")] | length' "$FILE"
# => 101

# Count verified or trusted
jq '[.data | to_entries[] | select(.value."is-public-api" == true and (.value."verification-status" == "verified" or .value."verification-status" == "trusted"))] | length' "$FILE"
# => 101

# List verified or trusted public API
jq -r '.data | to_entries[] | select(.value."is-public-api" == true and (.value."verification-status" == "verified" or .value."verification-status" == "trusted")) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort

# Full public API breakdown by verification status
jq '[.data | to_entries[] | select(.value."is-public-api" == true)] | group_by(.value."verification-status" // "absent") | map({status: .[0].value."verification-status" // "absent", count: length})' "$FILE"
```

---

## 3. Axioms

**21** Lean axioms form part of the trust base.

In Aeneas-transpiled projects, axioms are declarations that Aeneas generates
as placeholders for Rust functions it cannot fully transpile. They are
marked `verification-status: "trusted"` with `trusted-reason: "axiom"`.

```bash
# Count
jq '[.data | to_entries[] | select(.value."trusted-reason" == "axiom")] | length' "$FILE"
# => 21

# List axioms
jq -r '.data | to_entries[] | select(.value."trusted-reason" == "axiom") | "\(.value."display-name")\t\(.value.kind)\t\(.value."code-path")"' "$FILE" | sort
```

---

## 4. External functions assumed correct

**94** Lean definitions in `*External.lean` files are assumed correct.

These are hand-written Lean implementations of Rust functions that Aeneas
cannot transpile automatically (e.g. trait method dispatches, FFI boundaries,
constant-time operations). They are marked `verification-status: "trusted"`
with `trusted-reason: "external"`.

```bash
# Count
jq '[.data | to_entries[] | select(.value."trusted-reason" == "external")] | length' "$FILE"
# => 94

# List external
jq -r '.data | to_entries[] | select(.value."trusted-reason" == "external") | "\(.value."display-name")\t\(.value.kind)\t\(.value."code-path")"' "$FILE" | sort

# All trusted (axioms + external)
jq '[.data | to_entries[] | select(.value."verification-status" == "trusted")] | group_by(.value."trusted-reason") | map({reason: .[0].value."trusted-reason", count: length})' "$FILE"
```

---

## Summary

| Metric | Count | Denominator | % |
|--------|------:|------------:|---:|
| `pub fn` verified or trusted | 167 | 252 | 66.3% |
| — of which verified | 167 | 252 | 66.3% |
| — of which trusted | 0 | 252 | 0.0% |
| Public API verified or trusted | 101 | 147 | 68.7% |
| — of which verified | 101 | 147 | 68.7% |
| — of which trusted | 0 | 147 | 0.0% |
| Axioms (`trusted-reason: "axiom"`) | 21 | — | — |
| External (`trusted-reason: "external"`) | 94 | — | — |
| **Total trust base** | **115** | 2264 | 5.1% |
