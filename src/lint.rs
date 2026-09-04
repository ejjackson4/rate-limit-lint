use crate::parser::{Field, Rule};
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
    pub message: String,
}

// Beyond these, a rule is almost certainly a typo (e.g. a limit meant to be
// per-minute that ended up per-second) rather than an intentionally generous
// rate. --lenient exists for the rare case that's not true.
const MAX_SANE_LIMIT: u64 = 1_000_000;
const MAX_SANE_WINDOW_SECS: u64 = 86_400;

pub fn check(rules: &[Rule], lenient: bool) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut seen_paths: HashMap<String, (String, usize)> = HashMap::new();

    for rule in rules {
        let path = require_field(rule, &rule.path, "path", &mut findings);
        let limit = require_field(rule, &rule.limit, "limit", &mut findings)
            .and_then(|f| parse_positive(rule, f, "limit", &mut findings));
        let window = require_field(rule, &rule.window, "window", &mut findings)
            .and_then(|f| parse_positive(rule, f, "window", &mut findings));

        if let Some(f) = path {
            if !f.value.starts_with('/') {
                findings.push(Finding {
                    line: f.line,
                    severity: Severity::Error,
                    message: format!("path '{}' must start with '/'", f.value),
                });
            } else if let Some((other_rule, other_line)) = seen_paths.get(&f.value) {
                findings.push(Finding {
                    line: f.line,
                    severity: Severity::Error,
                    message: format!(
                        "path '{}' already defined at line {} by rule '{}'",
                        f.value, other_line, other_rule
                    ),
                });
            } else {
                seen_paths.insert(f.value.clone(), (rule.name.clone(), f.line));
            }
        }

        match &rule.burst {
            Some(f) => {
                if let Some(burst) = parse_positive(rule, f, "burst", &mut findings) {
                    if let Some(limit) = limit {
                        if burst < limit {
                            findings.push(Finding {
                                line: f.line,
                                severity: Severity::Error,
                                message: format!(
                                    "burst ({}) must be >= limit ({})",
                                    burst, limit
                                ),
                            });
                        }
                    }
                }
            }
            None if !lenient => findings.push(Finding {
                line: rule.line,
                severity: Severity::Error,
                message: format!(
                    "rule '{}' has no explicit 'burst'; add one or pass --lenient to default it to 'limit'",
                    rule.name
                ),
            }),
            None => {}
        }

        if !lenient {
            if let (Some(limit), Some(f)) = (limit, &rule.limit) {
                if limit > MAX_SANE_LIMIT {
                    findings.push(Finding {
                        line: f.line,
                        severity: Severity::Error,
                        message: format!(
                            "limit {} exceeds the sane maximum of {}; use --lenient to allow it",
                            limit, MAX_SANE_LIMIT
                        ),
                    });
                }
            }
            if let (Some(window), Some(f)) = (window, &rule.window) {
                if window > MAX_SANE_WINDOW_SECS {
                    findings.push(Finding {
                        line: f.line,
                        severity: Severity::Error,
                        message: format!(
                            "window {}s exceeds the sane maximum of {}s; use --lenient to allow it",
                            window, MAX_SANE_WINDOW_SECS
                        ),
                    });
                }
            }
        }
    }

    findings.sort_by_key(|f| f.line);
    findings
}

fn require_field<'a>(
    rule: &Rule,
    field: &'a Option<Field>,
    name: &str,
    findings: &mut Vec<Finding>,
) -> Option<&'a Field> {
    match field {
        Some(f) => Some(f),
        None => {
            findings.push(Finding {
                line: rule.line,
                severity: Severity::Error,
                message: format!("rule '{}' is missing required field '{}'", rule.name, name),
            });
            None
        }
    }
}

fn parse_positive(rule: &Rule, field: &Field, name: &str, findings: &mut Vec<Finding>) -> Option<u64> {
    match field.value.parse::<u64>() {
        Ok(0) => {
            findings.push(Finding {
                line: field.line,
                severity: Severity::Error,
                message: format!(
                    "rule '{}' field '{}' must be a positive integer, got 0",
                    rule.name, name
                ),
            });
            None
        }
        Ok(v) => Some(v),
        Err(_) => {
            findings.push(Finding {
                line: field.line,
                severity: Severity::Error,
                message: format!(
                    "rule '{}' field '{}' must be a positive integer, got '{}'",
                    rule.name, name, field.value
                ),
            });
            None
        }
    }
}
