//! Evaluation of `#[cfg(...)]` predicates against the Aeneas build config.
//!
//! Implements the cfg-inactive half of KB P25 (Aeneas): a Rust function whose
//! governing cfg predicate is *false* under the Aeneas build's active
//! configuration is not compiled and therefore out of verification scope (not
//! backlog). The predicate itself is emitted per-function by probe-rust as the
//! `cfg` atom field (own `#[cfg]` plus enclosing `impl`/`mod`/`trait` gates).
//!
//! Evaluation is deliberately **conservative**: it returns `Some(false)` only
//! when a predicate is definitively inactive given fully-known inputs, and
//! `None` when it references a flag/key the tool cannot resolve. Callers must
//! treat `None` as "leave as-is" and never disable a backlog atom on a guess.
//!
//! Ported from `probe-verus/src/cfg_eval.rs`, adapted for Aeneas: there is no
//! `verus_keep_ghost` flag; the active configuration is the resolved cargo
//! feature set for the Aeneas build.

use std::collections::{HashMap, HashSet};

/// A parsed `#[cfg(...)]` predicate.
#[derive(Debug, Clone, PartialEq)]
pub enum CfgExpr {
    /// A bare flag: `test`, `unix`, `nightly`, `docsrs`, ...
    Flag(String),
    /// A `key = "value"` predicate: `feature = "alloc"`, `target_arch = "x86_64"`.
    KeyValue(String, String),
    Not(Box<CfgExpr>),
    All(Vec<CfgExpr>),
    Any(Vec<CfgExpr>),
}

/// The active configuration of the Aeneas build: the resolved set of active
/// cargo features.
#[derive(Debug, Clone, Default)]
pub struct CfgConfig {
    /// Resolved active cargo features for the Aeneas build.
    pub features: HashSet<String>,
}

impl CfgConfig {
    /// Evaluate a predicate. `Some(true)`/`Some(false)` when decidable from
    /// known inputs; `None` when it references something we cannot resolve
    /// (unknown flag or a `key = value` other than `feature`).
    pub fn eval(&self, expr: &CfgExpr) -> Option<bool> {
        match expr {
            CfgExpr::Flag(name) => match name.as_str() {
                // The Aeneas build is never a `#[cfg(test)]` build.
                "test" => Some(false),
                // Unknown flags (unix, nightly, docsrs, custom cfgs): undecidable.
                _ => None,
            },
            CfgExpr::KeyValue(key, value) => {
                if key == "feature" {
                    Some(self.features.contains(value))
                } else {
                    // target_arch / target_os / custom key=value: not resolved here.
                    None
                }
            }
            CfgExpr::Not(inner) => self.eval(inner).map(|b| !b),
            CfgExpr::All(items) => {
                let mut all_true = true;
                for it in items {
                    match self.eval(it) {
                        Some(false) => return Some(false),
                        Some(true) => {}
                        None => all_true = false,
                    }
                }
                if all_true {
                    Some(true)
                } else {
                    None
                }
            }
            CfgExpr::Any(items) => {
                let mut all_false = true;
                for it in items {
                    match self.eval(it) {
                        Some(true) => return Some(true),
                        Some(false) => {}
                        None => all_false = false,
                    }
                }
                if all_false {
                    Some(false)
                } else {
                    None
                }
            }
        }
    }

    /// Whether a predicate is definitively inactive in the Aeneas build (i.e.
    /// the guarded item is not compiled). Only `Some(false)` counts.
    pub fn is_inactive(&self, predicate: &str) -> bool {
        parse_cfg(predicate).is_some_and(|e| self.eval(&e) == Some(false))
    }
}

/// Parse a cfg predicate string (the tokens inside `#[cfg(...)]`, e.g.
/// `all(feature = "alloc", not(test))`). Returns `None` on malformed input,
/// which callers treat as "cannot decide" (conservative).
pub fn parse_cfg(s: &str) -> Option<CfgExpr> {
    let tokens = tokenize(s);
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.parse_expr()?;
    if p.pos == p.tokens.len() {
        Some(expr)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    LParen,
    RParen,
    Comma,
    Eq,
}

fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                out.push(Tok::LParen);
            }
            ')' => {
                chars.next();
                out.push(Tok::RParen);
            }
            ',' => {
                chars.next();
                out.push(Tok::Comma);
            }
            '=' => {
                chars.next();
                out.push(Tok::Eq);
            }
            '"' => {
                chars.next();
                let mut val = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '"' {
                        closed = true;
                        break;
                    }
                    val.push(ch);
                }
                // An unterminated string literal is malformed: emit the bad
                // sentinel so the parse fails (⇒ None ⇒ never inactive), rather
                // than silently accepting a truncated value.
                if closed {
                    out.push(Tok::Str(val));
                } else {
                    out.push(Tok::Ident("\u{0}bad".to_string()));
                }
            }
            c if c.is_alphanumeric() || c == '_' || c == '-' => {
                let mut ident = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                        ident.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(Tok::Ident(ident));
            }
            _ => {
                // Unexpected character: bail (whole parse fails → conservative).
                chars.next();
                out.push(Tok::Ident("\u{0}bad".to_string()));
            }
        }
    }
    out
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Option<CfgExpr> {
        let ident = match self.next()? {
            Tok::Ident(i) => i,
            _ => return None,
        };
        match self.peek() {
            // `ident(...)` — only all/any/not take parens.
            Some(Tok::LParen) => {
                self.next(); // consume (
                match ident.as_str() {
                    "not" => {
                        let inner = self.parse_expr()?;
                        self.expect(Tok::RParen)?;
                        Some(CfgExpr::Not(Box::new(inner)))
                    }
                    "all" | "any" => {
                        let items = self.parse_list()?;
                        self.expect(Tok::RParen)?;
                        if ident == "all" {
                            Some(CfgExpr::All(items))
                        } else {
                            Some(CfgExpr::Any(items))
                        }
                    }
                    _ => None,
                }
            }
            // `ident = "value"`
            Some(Tok::Eq) => {
                self.next(); // consume =
                match self.next()? {
                    Tok::Str(v) => Some(CfgExpr::KeyValue(ident, v)),
                    _ => None,
                }
            }
            // bare flag
            _ => Some(CfgExpr::Flag(ident)),
        }
    }

    fn parse_list(&mut self) -> Option<Vec<CfgExpr>> {
        let mut items = vec![self.parse_expr()?];
        while matches!(self.peek(), Some(Tok::Comma)) {
            self.next(); // consume ,
                         // allow trailing comma before )
            if matches!(self.peek(), Some(Tok::RParen)) {
                break;
            }
            items.push(self.parse_expr()?);
        }
        Some(items)
    }

    fn expect(&mut self, t: Tok) -> Option<()> {
        if self.peek() == Some(&t) {
            self.next();
            Some(())
        } else {
            None
        }
    }
}

/// The transitive closure of `seeds` over the feature graph `edges`, following
/// only edges that enable *local* features (plain names present as keys in
/// `[features]`). Edges of the form `dep:x`, `x/y`, and `x?/y` enable
/// dependencies or dependency features, not local features, so they are ignored
/// for scope purposes. Unknown seed names (e.g. an implicit optional-dep
/// feature) are still recorded, since a `feature = "x"` cfg may reference them.
pub fn feature_closure(edges: &HashMap<String, Vec<String>>, seeds: &[String]) -> HashSet<String> {
    let mut active = HashSet::new();
    let mut work: Vec<String> = seeds.to_vec();
    while let Some(feat) = work.pop() {
        if feat.contains(':') || feat.contains('/') {
            continue;
        }
        if !edges.contains_key(&feat) {
            active.insert(feat);
            continue;
        }
        if active.insert(feat.clone()) {
            if let Some(next) = edges.get(&feat) {
                work.extend(next.iter().cloned());
            }
        }
    }
    active
}

/// Resolve the active feature set = transitive closure of the `default` feature.
/// See [`feature_closure`] for the edge-following rules.
pub fn resolve_default_features(cargo_toml: &str) -> HashSet<String> {
    let edges = parse_feature_edges(cargo_toml);
    let default_seeds = edges.get("default").cloned().unwrap_or_default();
    feature_closure(&edges, &default_seeds)
}

/// Parse the `[features]` table of a `Cargo.toml` into a `feature -> edges` map.
pub fn parse_feature_edges(cargo_toml: &str) -> HashMap<String, Vec<String>> {
    let Ok(value) = cargo_toml.parse::<toml::Value>() else {
        return HashMap::new();
    };
    let Some(features) = value.get("features").and_then(|f| f.as_table()) else {
        return HashMap::new();
    };
    features
        .iter()
        .map(|(name, deps)| {
            let list = deps
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            (name.clone(), list)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(features: &[&str]) -> CfgConfig {
        CfgConfig {
            features: features.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn parse_and_eval_basic() {
        let c = cfg(&["alloc", "digest"]);
        assert_eq!(
            c.eval(&parse_cfg(r#"feature = "alloc""#).unwrap()),
            Some(true)
        );
        assert_eq!(
            c.eval(&parse_cfg(r#"feature = "serde""#).unwrap()),
            Some(false)
        );
        assert_eq!(c.eval(&parse_cfg("test").unwrap()), Some(false));
        assert_eq!(c.eval(&parse_cfg("not(test)").unwrap()), Some(true));
    }

    #[test]
    fn all_any_not() {
        let c = cfg(&["alloc"]);
        // all: any false ⟹ false (inactive)
        assert!(c.is_inactive(r#"all(feature = "alloc", feature = "serde")"#));
        // all of active ⟹ active (not inactive)
        assert!(!c.is_inactive(r#"all(feature = "alloc", not(test))"#));
        // any: all false ⟹ false (inactive)
        assert!(c.is_inactive(r#"any(feature = "serde", feature = "group")"#));
        // any with one active ⟹ active
        assert!(!c.is_inactive(r#"any(feature = "serde", feature = "alloc")"#));
        // #[cfg(test)] is always inactive in the Aeneas build.
        assert!(c.is_inactive("test"));
    }

    #[test]
    fn unknown_is_undecidable_not_inactive() {
        let c = cfg(&["alloc"]);
        // Unknown target key ⟹ None ⟹ not treated as inactive (conservative).
        assert_eq!(
            c.eval(&parse_cfg(r#"target_arch = "x86_64""#).unwrap()),
            None
        );
        assert!(!c.is_inactive(r#"target_arch = "x86_64""#));
        assert_eq!(c.eval(&parse_cfg("nightly").unwrap()), None);
        // all(unknown, nightly) ⟹ None ⟹ not inactive
        assert!(!c.is_inactive(r#"all(target_arch = "x86_64", nightly)"#));
    }

    #[test]
    fn malformed_is_not_inactive() {
        let c = cfg(&["alloc"]);
        assert!(!c.is_inactive("all(feature ="));
        assert!(parse_cfg("").is_none());
        // Unterminated string literal is malformed ⇒ None ⇒ never inactive,
        // even though the (missing) value names an inactive feature.
        assert!(parse_cfg("feature = \"serde").is_none());
        assert!(!c.is_inactive("feature = \"serde"));
    }

    #[test]
    fn resolve_default_features_closure() {
        let toml = r#"
[package]
name = "x"
version = "0.1.0"

[features]
default = ["alloc", "precomputed-tables", "zeroize", "lizard"]
alloc = ["zeroize?/alloc"]
precomputed-tables = []
zeroize = []
digest = ["dep:digest", "dep:sha2"]
lizard = ["digest"]
group = ["dep:group", "rand_core"]
"#;
        let active = resolve_default_features(toml);
        assert!(active.contains("alloc"));
        assert!(active.contains("precomputed-tables"));
        assert!(active.contains("zeroize"));
        assert!(active.contains("lizard"));
        assert!(active.contains("digest")); // via lizard
        assert!(!active.contains("group")); // not default
        assert!(!active.contains("rand_core")); // only via group
        assert!(!active.contains("serde"));
    }

    #[test]
    fn feature_closure_from_explicit_seeds() {
        let edges = parse_feature_edges(
            r#"
[features]
default = []
group = ["rand_core"]
rand_core = []
"#,
        );
        // Explicitly enabling `group` (e.g. via --features group) pulls rand_core.
        let active = feature_closure(&edges, &["group".to_string()]);
        assert!(active.contains("group"));
        assert!(active.contains("rand_core"));
    }
}
