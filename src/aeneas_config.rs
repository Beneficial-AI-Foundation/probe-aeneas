//! `aeneas_config` module: load optional per-project Aeneas enrichment config.
//!
//! ## Error model
//!
//! [`AeneasConfig::load`] returns [`anyhow::Result<AeneasConfig>`]. All
//! failures are IO or JSON parse errors, so no typed variants are needed.
//! Callers in `extract` and `listfuns` use `?` to propagate these into their
//! own `Other(anyhow::Error)` catch-all variants.

use anyhow::Context as _;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

/// Aeneas project configuration for fields that cannot be auto-detected.
///
/// Loaded from `--aeneas-config` CLI flag or `.verilib/aeneas.json` in the
/// Lean project directory. All fields are optional; omitted lists default to
/// empty.
#[derive(Debug, Default, Deserialize)]
pub struct AeneasConfigFile {
    /// Additional function names to mark as hidden (beyond auto-detected
    /// trait impls, `.Insts.` patterns, and `.mutual` loop bodies).
    #[serde(default, rename = "is-hidden")]
    pub is_hidden: Vec<String>,

    /// Function names to mark as ignored (excluded from verification
    /// progress percentages). This is always a manual editorial decision.
    #[serde(default, rename = "is-ignored")]
    pub is_ignored: Vec<String>,

    /// Rust functions to mark **out of verification scope** (`is-disabled: true`,
    /// no status), given as glob patterns (`*` matches any run of characters)
    /// matched against each Rust atom's `rust-qualified-name` and `display-name`.
    /// For functions Aeneas structurally does not translate — e.g. `Debug`/
    /// `Display` `fmt`, `Zeroize` — which never appear in `functions.json` and
    /// so cannot carry a Lean `@[out_of_scope]` attribute, yet are not
    /// verification targets. A curated, reviewable opt-out (KB P25).
    #[serde(default, rename = "out-of-scope")]
    pub out_of_scope: Vec<String>,
}

/// Resolved config used during enrichment.
#[derive(Debug, Default)]
pub struct AeneasConfig {
    pub hidden: HashSet<String>,
    pub ignored: HashSet<String>,
    /// Out-of-scope glob patterns (see [`AeneasConfigFile::out_of_scope`]).
    pub out_of_scope: Vec<String>,
}

impl AeneasConfig {
    /// Load config from an explicit path, or try `.verilib/aeneas.json`
    /// relative to the Lean project directory. Missing files are not errors.
    pub fn load(explicit_path: Option<&Path>, lean_project: Option<&Path>) -> anyhow::Result<Self> {
        let path = explicit_path
            .map(|p| p.to_path_buf())
            .or_else(|| lean_project.map(|lp| lp.join(".verilib").join("aeneas.json")));

        let Some(path) = path else {
            return Ok(Self::default());
        };

        if !path.exists() {
            return Ok(Self::default());
        }

        let content =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let file: AeneasConfigFile =
            serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))?;

        println!("Loaded Aeneas config from {}", path.display());
        if !file.is_hidden.is_empty() {
            println!("  is-hidden: {} entries", file.is_hidden.len());
        }
        if !file.is_ignored.is_empty() {
            println!("  is-ignored: {} entries", file.is_ignored.len());
        }
        if !file.out_of_scope.is_empty() {
            println!("  out-of-scope: {} pattern(s)", file.out_of_scope.len());
        }

        Ok(Self {
            hidden: file.is_hidden.into_iter().collect(),
            ignored: file.is_ignored.into_iter().collect(),
            out_of_scope: file.out_of_scope,
        })
    }
}
