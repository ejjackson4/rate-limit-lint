use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct SuppressError {
    pub line: usize,
    pub message: String,
}

const WILDCARD: &str = "*";

const KNOWN_CODES: &[&str] = &[
    "missing-path",
    "missing-limit",
    "missing-window",
    "invalid-value",
    "path-format",
    "duplicate-path",
    "burst-below-limit",
    "missing-burst",
    "limit-too-large",
    "window-too-large",
];

/// Per-rule suppressions for specific lint checks, loaded from a
/// `--suppress` file. Lets one rule opt out of one check without turning
/// that check off for the whole file the way `--lenient` does.
#[derive(Debug, Default)]
pub struct Suppressions {
    by_rule: HashMap<String, HashSet<String>>,
}

impl Suppressions {
    pub fn is_suppressed(&self, rule_name: &str, code: &str) -> bool {
        self.matches(rule_name, code) || self.matches(WILDCARD, code)
    }

    fn matches(&self, rule_name: &str, code: &str) -> bool {
        self.by_rule
            .get(rule_name)
            .map_or(false, |codes| codes.contains(code))
    }
}

/// Parses a suppression file: one `rule: check[, check...]` entry per line.
/// `rule` may be `*` to suppress a check across every rule in the file.
/// `#` starts a comment (inline or on its own line), and blank lines are
/// ignored.
///
/// ```text
/// # legacy endpoint, burst intentionally left to the limiter's default
/// legacy-export: missing-burst
///
/// # every rule here intentionally exceeds the sane window ceiling
/// *: window-too-large
/// ```
///
/// A rule name may appear on more than one line; the checks it suppresses
/// accumulate rather than the later line replacing the earlier one.
pub fn parse(source: &str) -> Result<Suppressions, SuppressError> {
    let mut by_rule: HashMap<String, HashSet<String>> = HashMap::new();

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = match raw_line.find('#') {
            Some(i) => &raw_line[..i],
            None => raw_line,
        }
        .trim();
        if line.is_empty() {
            continue;
        }

        let (rule_name, codes) = line.split_once(':').ok_or_else(|| SuppressError {
            line: line_no,
            message: format!("expected 'rule: check[, check...]', found '{}'", line),
        })?;
        let rule_name = rule_name.trim();
        if rule_name.is_empty() {
            return Err(SuppressError {
                line: line_no,
                message: "rule name must not be empty".to_string(),
            });
        }

        let entry = by_rule.entry(rule_name.to_string()).or_default();
        for code in codes.split(',') {
            let code = code.trim();
            if code.is_empty() {
                return Err(SuppressError {
                    line: line_no,
                    message: "check name must not be empty".to_string(),
                });
            }
            if !KNOWN_CODES.iter().any(|&known| known == code) {
                return Err(SuppressError {
                    line: line_no,
                    message: format!(
                        "unknown check '{}', expected one of: {}",
                        code,
                        KNOWN_CODES.join(", ")
                    ),
                });
            }
            entry.insert(code.to_string());
        }
    }

    Ok(Suppressions { by_rule })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_single_suppression() {
        let s = parse("search: missing-burst\n").unwrap();
        assert!(s.is_suppressed("search", "missing-burst"));
        assert!(!s.is_suppressed("search", "duplicate-path"));
        assert!(!s.is_suppressed("login", "missing-burst"));
    }

    #[test]
    fn parses_multiple_comma_separated_checks() {
        let s = parse("search: missing-burst, limit-too-large\n").unwrap();
        assert!(s.is_suppressed("search", "missing-burst"));
        assert!(s.is_suppressed("search", "limit-too-large"));
    }

    #[test]
    fn wildcard_rule_name_applies_to_every_rule() {
        let s = parse("*: window-too-large\n").unwrap();
        assert!(s.is_suppressed("anything", "window-too-large"));
        assert!(!s.is_suppressed("anything", "missing-burst"));
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let s = parse("# a comment\n\nsearch: missing-burst # trailing note\n").unwrap();
        assert!(s.is_suppressed("search", "missing-burst"));
    }

    #[test]
    fn merges_multiple_lines_for_the_same_rule() {
        let s = parse("search: missing-burst\nsearch: limit-too-large\n").unwrap();
        assert!(s.is_suppressed("search", "missing-burst"));
        assert!(s.is_suppressed("search", "limit-too-large"));
    }

    #[test]
    fn rejects_a_line_without_a_colon() {
        let err = parse("search missing-burst\n").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("expected 'rule: check"));
    }

    #[test]
    fn rejects_an_empty_rule_name() {
        let err = parse(": missing-burst\n").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("rule name must not be empty"));
    }

    #[test]
    fn rejects_an_unknown_check_name() {
        let err = parse("search: not-a-real-check\n").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("unknown check 'not-a-real-check'"));
    }

    #[test]
    fn empty_source_yields_no_suppressions() {
        let s = parse("").unwrap();
        assert!(!s.is_suppressed("anything", "missing-burst"));
    }
}
