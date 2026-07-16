//! `types` module: shared data types for `functions.json` parsing and atom-file loading.
//!
//! ## Error model
//!
//! [`LineRange::parse`] returns [`Result<LineRange, ParseError>`]. The
//! `ParseError` enum names categorical failure modes for line-range string
//! parsing. Callers that treat parse failure as optional use `.ok()`.

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Typed error
// ---------------------------------------------------------------------------

/// Errors produced when parsing a `"L<start>-L<end>"` line-range string.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid line-range format '{0}': expected 'L<start>-L<end>'")]
    InvalidFormat(String),

    #[error("inverted range: start {start} > end {end}")]
    InvertedRange { start: usize, end: usize },

    #[error("line number is not a valid integer: {0}")]
    InvalidNumber(#[from] std::num::ParseIntError),
}

/// A single entry from `functions.json`, produced by `lake exe listfuns`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FunctionRecord {
    pub lean_name: String,
    #[serde(default)]
    pub rust_name: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub lines: Option<String>,
    #[serde(default)]
    pub is_hidden: bool,
    #[serde(default)]
    pub is_extraction_artifact: bool,

    // -- Authoritative overlay from Aeneas's `translation.json` (in-memory only;
    //    never (de)serialized to/from `functions.json`). Populated by
    //    `translation_manifest::annotate` when the manifest is available. --
    /// `Some(true)` when Aeneas marks this `lean_name` as a loop helper (its
    /// manifest entry carries a `loop` field), `Some(false)` when it is the
    /// authoritative top-level def, `None` when the manifest does not cover it
    /// (callers fall back to name-suffix heuristics).
    #[serde(skip)]
    pub is_loop_artifact: Option<bool>,
    /// `lean_name` of the enclosing top-level def, for loop helpers.
    #[serde(skip)]
    pub parent_lean_name: Option<String>,
    /// Aeneas `FunDeclId`, shared by a top-level def and all its loop helpers.
    #[serde(skip)]
    pub def_id: Option<u64>,
    /// Whether `def_id` is a charon `FunDeclId` (from the manifest's `functions`
    /// array) rather than a `GlobalDeclId`/`TraitImplId` (from `globals`/
    /// `trait_impls`). charon numbers each declaration kind in its own id space,
    /// so only `FunDeclId`s are comparable to probe-rust's `charon-def-id`. Only
    /// records with this set participate in the `charon-def-id` integer join;
    /// `false` (the default) keeps globals/trait-impl records out of it.
    #[serde(skip)]
    pub def_id_is_fun_decl: bool,
}

/// Top-level structure of `functions.json`.
#[derive(Debug, Deserialize)]
pub struct FunctionsFile {
    pub functions: Vec<FunctionRecord>,
}

/// Parsed line range from a "L<start>-L<end>" string.
#[derive(Debug, Clone, Copy)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

impl LineRange {
    /// Parse `"L292-L325"` into `LineRange { start: 292, end: 325 }`.
    pub fn parse(s: &str) -> Result<Self, ParseError> {
        let s = s.trim();
        let rest = s
            .strip_prefix('L')
            .ok_or_else(|| ParseError::InvalidFormat(s.to_string()))?;
        let (start_str, end_part) = rest
            .split_once('-')
            .ok_or_else(|| ParseError::InvalidFormat(s.to_string()))?;
        let end_str = end_part
            .strip_prefix('L')
            .ok_or_else(|| ParseError::InvalidFormat(s.to_string()))?;
        let start: usize = start_str.parse()?;
        let end: usize = end_str.parse()?;
        if start > end {
            return Err(ParseError::InvertedRange { start, end });
        }
        Ok(LineRange { start, end })
    }

    pub fn overlaps(&self, other: &LineRange, tolerance: usize) -> bool {
        self.start <= other.end + tolerance && other.start <= self.end + tolerance
    }

    pub fn overlap_amount(&self, other: &LineRange) -> i64 {
        let end = std::cmp::min(self.end, other.end) as i64;
        let start = std::cmp::max(self.start, other.start) as i64;
        end - start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_range_rejects_inverted_range() {
        assert!(LineRange::parse("L100-L50").is_err());
    }

    #[test]
    fn line_range_accepts_valid_range() {
        let r = LineRange::parse("L50-L100").expect("valid range");
        assert_eq!(r.start, 50);
        assert_eq!(r.end, 100);
    }
}
