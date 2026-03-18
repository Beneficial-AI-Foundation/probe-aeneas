use serde::Deserialize;

/// A single entry from `functions.json`, produced by `lake exe listfuns`.
#[derive(Debug, Clone, Deserialize)]
pub struct FunctionRecord {
    pub lean_name: String,
    #[serde(default)]
    pub rust_name: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub lines: Option<String>,
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
    /// Parse "L292-L325" into LineRange { start: 292, end: 325 }.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let rest = s.strip_prefix('L')?;
        let (start_str, end_part) = rest.split_once('-')?;
        let end_str = end_part.strip_prefix('L')?;
        let start = start_str.parse().ok()?;
        let end = end_str.parse().ok()?;
        if start > end {
            return None;
        }
        Some(LineRange { start, end })
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
        assert!(LineRange::parse("L100-L50").is_none());
    }

    #[test]
    fn line_range_accepts_valid_range() {
        let r = LineRange::parse("L50-L100").expect("valid range");
        assert_eq!(r.start, 50);
        assert_eq!(r.end, 100);
    }
}
