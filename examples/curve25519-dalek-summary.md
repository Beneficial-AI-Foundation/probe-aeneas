# Verification Statistics: curve25519-dalek 4.1.3 (Aeneas)

Generated from `examples/aeneas_curve25519-dalek_4.1.3.json` produced by
`probe-aeneas extract`.

All queries below use `jq` on the extract JSON:

```bash
FILE=examples/aeneas_curve25519-dalek_4.1.3.json
```

Rust functions that Aeneas transpiles to Lean receive a `verification-status`
field (`verified` or `trusted`). Functions outside Aeneas's scope (excluded
by `aeneas-config.yml`, feature-gated trait impls, functions Charon cannot
extract) have no `verification-status` and are marked `is-disabled: true`.
The ratios below use `verification-status != null` as the denominator so
they reflect only functions Aeneas actually processed.

---

## 1. Verified Rust functions (all)

**208** Rust functions were processed by Aeneas: **208 verified + 0 trusted,
0 unverified** (100%).

Note: 672 total Rust functions exist in the output, but 464 have no
`verification-status` because they are outside Aeneas's scope (not in
`start_from`, explicitly excluded, feature-gated ecosystem trait impls,
functions Charon cannot extract).

```bash
# In-scope Rust = those with a verification-status
jq '[.data | to_entries[] | select(.value.language == "rust" and .value."verification-status" != null)] | length' "$FILE"
# => 208

# Verified
jq '[.data | to_entries[] | select(.value.language == "rust" and .value."verification-status" == "verified")] | length' "$FILE"
# => 208

# Trusted
jq '[.data | to_entries[] | select(.value.language == "rust" and .value."verification-status" == "trusted")] | length' "$FILE"
# => 0

# List all in-scope Rust by status
jq -r '.data | to_entries[] | select(.value.language == "rust" and .value."verification-status" != null) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

---

## 2. Verified pub fn Rust functions

**167** `pub fn` Rust functions were processed by Aeneas: **167 verified +
0 trusted, 0 unverified** (100%).

Note: 252 total `pub fn` Rust functions exist in the output, but 85 have no
`verification-status` because they are outside Aeneas's scope (feature-gated
trait impls for `group`/`serde`/`ff`, backend dispatch functions, `Debug` fmt
implementations, functions excluded from Charon extraction).

```bash
# In-scope pub fn = pub + Rust + has status
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.language == "rust" and .value."verification-status" != null)] | length' "$FILE"
# => 167

# Verified
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.language == "rust" and .value."verification-status" == "verified")] | length' "$FILE"
# => 167

# Trusted
jq '[.data | to_entries[] | select(.value."is-public" == true and .value.language == "rust" and .value."verification-status" == "trusted")] | length' "$FILE"
# => 0

# List in-scope pub fn by status
jq -r '.data | to_entries[] | select(.value."is-public" == true and .value.language == "rust" and .value."verification-status" != null) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

---

## 3. Verified public API functions

**101** public API functions were processed by Aeneas: **101 verified +
0 trusted, 0 unverified** (100%).

Public API = `pub fn` + all ancestor modules are `pub` + library crate
(`is-public-api: true`). More selective than `is-public` — a `pub fn`
inside a private module has `is-public: true` but `is-public-api: false`.

Note: 147 total public API functions exist, but 46 have no
`verification-status` because they are outside Aeneas's scope.

```bash
# In-scope public API = is-public-api + has status
jq '[.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" != null)] | length' "$FILE"
# => 101

# Verified
jq '[.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" == "verified")] | length' "$FILE"
# => 101

# Trusted
jq '[.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" == "trusted")] | length' "$FILE"
# => 0

# List in-scope public API by status
jq -r '.data | to_entries[] | select(.value."is-public-api" == true and .value."verification-status" != null) | "\(.value."verification-status")\t\(.value."display-name")\t\(.value."code-path")"' "$FILE" | sort
```

Trusted is 0 for Rust atoms because `"trusted"` is a Lean-side concept
(axioms and `*External.lean` definitions — see sections 4 and 5). In Aeneas
projects, the trust base lives in the Lean layer; Rust functions translate to
verified Lean definitions that *depend on* trusted atoms, but the translation
target itself is verified.

---

## 4. Axioms

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

## 5. External functions assumed correct

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

| Metric | Verified | Trusted | In-scope | % (v+t) |
|--------|--------:|---------:|---------:|--------:|
| All Rust functions | 208 | 0 | 208 | 100% |
| `pub fn` Rust | 167 | 0 | 167 | 100% |
| Public API | 101 | 0 | 101 | 100% |

"In-scope" = Rust functions with a `verification-status` (i.e., Aeneas
transpiled them to Lean and the Lean proof was verified). Functions outside
scope (464 Rust functions with no status) are excluded from Charon extraction,
feature-gated ecosystem trait impls, or functions Charon cannot extract.
These are marked `is-disabled: true`.

| Trust base | Count |
|------------|------:|
| Axioms (`trusted-reason: "axiom"`) | 21 |
| External (`trusted-reason: "external"`) | 94 |
| **Total trust base** | **115** |
