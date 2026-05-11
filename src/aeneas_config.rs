//! `aeneas_config` module: load optional per-project Aeneas enrichment config.
//!
//! ## Error model
//!
//! [`AeneasConfig::load`] returns [`Result<AeneasConfig, AeneasConfigError>`].
//! All failures are IO or JSON parse errors and flow through
//! `Other(anyhow::Error)` via `.context("...")` chains.
//!
//! Callers in `extract` and `listfuns` bridge via `.map_err(anyhow::Error::new)?`
//! so errors propagate into `ExtractError::Other` / `ListfunsError::Other`
//! with their full source chain preserved.

use anyhow::Context as _;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

// ---------------------------------------------------------------------------
// Typed error
// ---------------------------------------------------------------------------

/// Errors produced by [`AeneasConfig::load`].
#[derive(Debug, thiserror::Error)]
pub enum AeneasConfigError {
    /// Catch-all wrapping `anyhow::Error`. Covers `io::Error` from file reads
    /// and `serde_json::Error` from JSON parsing.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience alias used throughout this module.
pub type Result<T> = std::result::Result<T, AeneasConfigError>;

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
}

/// Resolved config used during enrichment.
#[derive(Debug, Default)]
pub struct AeneasConfig {
    pub hidden: HashSet<String>,
    pub ignored: HashSet<String>,
}

impl AeneasConfig {
    /// Load config from an explicit path, or try `.verilib/aeneas.json`
    /// relative to the Lean project directory. Missing files are not errors.
    pub fn load(explicit_path: Option<&Path>, lean_project: Option<&Path>) -> Result<Self> {
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

        Ok(Self {
            hidden: file.is_hidden.into_iter().collect(),
            ignored: file.is_ignored.into_iter().collect(),
        })
    }
}
