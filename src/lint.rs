use crate::parser::{Field, Rule};
use crate::suppress::Suppressions;
use std::collections::HashMap;

pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn label(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

pub struct Finding {
    pub line: usize,
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
}

// Beyond these, a rule is almost certainly a typo (e.g. a limit meant to be
// per-minute that ended up per-second) rather than an intentionally generous
// rate. --lenient exists for the rare case that's not true.
const MAX_SANE_LIMIT: u64 = 1_000_000;
const MAX_SANE_WINDOW_SECS: u64 = 86_400;

pub fn check(rules: &[Rule], lenient: bool, suppressions: &Suppressions) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut seen_paths: HashMap<String, (String, usize)> = HashMap::new();

    for rule in rules {
        let path = require_field(rule, &rule.path, "path", "missing-path", suppressions, &mut findings);
        let limit = require_field(rule, &rule.limit, "limit", "missing-limit", suppressions, &mut findings)
            .and_then(|f| parse_positive(rule, f, "limit", suppressions, &mut findings));
        let window = require_field(rule, &rule.window, "window", "missing-window", suppressions, &mut findings)
            .and_then(|f| parse_positive(rule, f, "window", suppressions, &mut findings));

        if let Some(f) = path {
            if !f.value.starts_with('/') {
                push(
                    &mut findings,
                    suppressions,
                    rule,
                    "path-format",
                    f.line,
                    format!("path '{}' must start with '/'", f.value),
                );
            } else if let Some((other_rule, other_line)) = seen_paths.get(&f.value) {
                push(
                    &mut findings,
                    suppressions,
                    rule,
                    "duplicate-path",
                    f.line,
                    format!(
                        "path '{}' already defined at line {} by rule '{}'",
                        f.value, other_line, other_rule
                    ),
                );
            } else {
                seen_paths.insert(f.value.clone(), (rule.name.clone(), f.line));
            }
        }

        match &rule.burst {
            Some(f) => {
                if let Some(burst) = parse_positive(rule, f, "burst", suppressions, &mut findings) {
                    if let Some(limit) = limit {
                        if burst < limit {
                            push(
                                &mut findings,
                                suppressions,
                                rule,
                                "burst-below-limit",
                                f.line,
                                format!("burst ({}) must be >= limit ({})", burst, limit),
                            );
                        }
                    }
                }
            }
            None if !lenient => push(
                &mut findings,
                suppressions,
                rule,
                "missing-burst",
                rule.line,
                format!(
                    "rule '{}' has no explicit 'burst'; add one or pass --lenient to default it to 'limit'",
                    rule.name
                ),
            ),
            None => {}
        }

        if !lenient {
            if let (Some(limit), Some(f)) = (limit, &rule.limit) {
                if limit > MAX_SANE_LIMIT {
                    push(
                        &mut findings,
                        suppressions,
                        rule,
                        "limit-too-large",
                        f.line,
                        format!(
                            "limit {} exceeds the sane maximum of {}; use --lenient to allow it",
                            limit, MAX_SANE_LIMIT
                        ),
                    );
                }
            }
            if let (Some(window), Some(f)) = (window, &rule.window) {
                if window > MAX_SANE_WINDOW_SECS {
                    push(
                        &mut findings,
                        suppressions,
                        rule,
                        "window-too-large",
                        f.line,
                        format!(
                            "window {}s exceeds the sane maximum of {}s; use --lenient to allow it",
                            window, MAX_SANE_WINDOW_SECS
                        ),
                    );
                }
            }
        }
    }

    findings.sort_by_key(|f| f.line);
    findings
}

fn push(
    findings: &mut Vec<Finding>,
    suppressions: &Suppressions,
    rule: &Rule,
    code: &'static str,
    line: usize,
    message: String,
) {
    if suppressions.is_suppressed(&rule.name, code) {
        return;
    }
    findings.push(Finding {
        line,
        severity: Severity::Error,
        code,
        message,
    });
}

fn require_field<'a>(
    rule: &Rule,
    field: &'a Option<Field>,
    name: &str,
    code: &'static str,
    suppressions: &Suppressions,
    findings: &mut Vec<Finding>,
) -> Option<&'a Field> {
    match field {
        Some(f) => Some(f),
        None => {
            push(
                findings,
                suppressions,
                rule,
                code,
                rule.line,
                format!("rule '{}' is missing required field '{}'", rule.name, name),
            );
            None
        }
    }
}

fn parse_positive(
    rule: &Rule,
    field: &Field,
    name: &str,
    suppressions: &Suppressions,
    findings: &mut Vec<Finding>,
) -> Option<u64> {
    match field.value.parse::<u64>() {
        Ok(0) => {
            push(
                findings,
                suppressions,
                rule,
                "invalid-value",
                field.line,
                format!(
                    "rule '{}' field '{}' must be a positive integer, got 0",
                    rule.name, name
                ),
            );
            None
        }
        Ok(v) => Some(v),
        Err(_) => {
            push(
                findings,
                suppressions,
                rule,
                "invalid-value",
                field.line,
                format!(
                    "rule '{}' field '{}' must be a positive integer, got '{}'",
                    rule.name, name, field.value
                ),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::suppress;

    fn lines_with(findings: &[Finding]) -> Vec<usize> {
        findings.iter().map(|f| f.line).collect()
    }

    fn messages(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.message.as_str()).collect()
    }

    fn no_suppressions() -> Suppressions {
        Suppressions::default()
    }

    #[test]
    fn clean_strict_rule_has_no_findings() {
        let rules = parser::parse(
            "[login]\npath = /api/login\nlimit = 5\nwindow = 60\nburst = 10\n",
        )
        .unwrap();
        let findings = check(&rules, false, &no_suppressions());
        assert!(findings.is_empty());
    }

    #[test]
    fn reports_missing_required_fields() {
        let rules = parser::parse("[login]\nlimit = 5\nwindow = 60\nburst = 10\n").unwrap();
        let findings = check(&rules, false, &no_suppressions());
        assert!(messages(&findings)
            .iter()
            .any(|m| m.contains("missing required field 'path'")));
    }

    #[test]
    fn reports_non_positive_integers() {
        let rules =
            parser::parse("[login]\npath = /api/login\nlimit = 0\nwindow = abc\nburst = 10\n")
                .unwrap();
        let findings = check(&rules, false, &no_suppressions());
        let msgs = messages(&findings);
        assert!(msgs.iter().any(|m| m.contains("field 'limit' must be a positive integer, got 0")));
        assert!(msgs
            .iter()
            .any(|m| m.contains("field 'window' must be a positive integer, got 'abc'")));
    }

    #[test]
    fn reports_path_without_leading_slash() {
        let rules =
            parser::parse("[login]\npath = api/login\nlimit = 5\nwindow = 60\nburst = 10\n")
                .unwrap();
        let findings = check(&rules, false, &no_suppressions());
        assert!(messages(&findings)
            .iter()
            .any(|m| m.contains("must start with '/'")));
    }

    #[test]
    fn reports_duplicate_paths() {
        let rules = parser::parse(
            "[login]\npath = /api/login\nlimit = 5\nwindow = 60\nburst = 10\n\n[login2]\npath = /api/login\nlimit = 5\nwindow = 60\nburst = 10\n",
        )
        .unwrap();
        let findings = check(&rules, false, &no_suppressions());
        assert!(messages(&findings)
            .iter()
            .any(|m| m.contains("already defined at line 2 by rule 'login'")));
    }

    #[test]
    fn reports_burst_below_limit() {
        let rules =
            parser::parse("[login]\npath = /api/login\nlimit = 10\nwindow = 60\nburst = 5\n")
                .unwrap();
        let findings = check(&rules, false, &no_suppressions());
        assert!(messages(&findings)
            .iter()
            .any(|m| m.contains("burst (5) must be >= limit (10)")));
    }

    #[test]
    fn strict_mode_requires_explicit_burst() {
        let rules = parser::parse("[search]\npath = /api/search\nlimit = 100\nwindow = 60\n").unwrap();

        let strict_findings = check(&rules, false, &no_suppressions());
        assert!(messages(&strict_findings)
            .iter()
            .any(|m| m.contains("has no explicit 'burst'")));

        let lenient_findings = check(&rules, true, &no_suppressions());
        assert!(lenient_findings.is_empty());
    }

    #[test]
    fn strict_mode_flags_absurd_limit_and_window() {
        let rules = parser::parse(
            "[login]\npath = /api/login\nlimit = 5000000\nwindow = 999999\nburst = 5000000\n",
        )
        .unwrap();

        let strict_findings = check(&rules, false, &no_suppressions());
        let msgs = messages(&strict_findings);
        assert!(msgs.iter().any(|m| m.contains("exceeds the sane maximum of 1000000")));
        assert!(msgs.iter().any(|m| m.contains("exceeds the sane maximum of 86400s")));

        let lenient_findings = check(&rules, true, &no_suppressions());
        assert!(lenient_findings.is_empty());
    }

    #[test]
    fn findings_are_sorted_by_line() {
        let rules = parser::parse(
            "[login]\npath = api/login\nlimit = 0\nwindow = 60\nburst = 10\n",
        )
        .unwrap();
        let findings = check(&rules, false, &no_suppressions());
        let lines = lines_with(&findings);
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted);
    }

    #[test]
    fn suppressed_finding_is_removed() {
        let rules =
            parser::parse("[search]\npath = /api/search\nlimit = 100\nwindow = 60\n").unwrap();
        let suppressions = suppress::parse("search: missing-burst\n").unwrap();
        let findings = check(&rules, false, &suppressions);
        assert!(findings.is_empty());
    }

    #[test]
    fn suppression_is_scoped_to_the_named_rule() {
        let rules = parser::parse(
            "[login]\npath = /api/login\nlimit = 5\nwindow = 60\n\n[search]\npath = /api/search\nlimit = 100\nwindow = 60\n",
        )
        .unwrap();
        let suppressions = suppress::parse("search: missing-burst\n").unwrap();
        let findings = check(&rules, false, &suppressions);
        assert_eq!(findings.len(), 1);
        assert!(messages(&findings)
            .iter()
            .any(|m| m.contains("rule 'login' has no explicit 'burst'")));
    }

    #[test]
    fn wildcard_suppression_applies_to_every_rule() {
        let rules = parser::parse(
            "[login]\npath = /api/login\nlimit = 5\nwindow = 60\n\n[search]\npath = /api/search\nlimit = 100\nwindow = 60\n",
        )
        .unwrap();
        let suppressions = suppress::parse("*: missing-burst\n").unwrap();
        let findings = check(&rules, false, &suppressions);
        assert!(findings.is_empty());
    }

    #[test]
    fn suppressing_one_check_leaves_others_active() {
        let rules =
            parser::parse("[login]\npath = /api/login\nlimit = 10\nwindow = 60\nburst = 5\n")
                .unwrap();
        let suppressions = suppress::parse("login: missing-burst\n").unwrap();
        let findings = check(&rules, false, &suppressions);
        assert!(messages(&findings)
            .iter()
            .any(|m| m.contains("burst (5) must be >= limit (10)")));
    }
}
