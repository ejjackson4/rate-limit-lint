use crate::parser::{Field, ParseError, Rule};

struct PendingDescriptor {
    key: String,
    key_line: usize,
    value: Option<Field>,
    unit: Option<(String, usize)>,
    requests_per_unit: Option<Field>,
}

/// Parses the descriptor config format used by the envoyproxy/ratelimit
/// sidecar (https://github.com/envoyproxy/ratelimit#configuration):
///
/// ```text
/// domain: api-gateway
/// descriptors:
///   - key: path
///     value: /api/login
///     rate_limit:
///       unit: second
///       requests_per_unit: 5
/// ```
///
/// Only descriptors whose `key` is literally `path` become rules; other
/// descriptor kinds (`remote_address`, `header_match`, `generic_key`, ...)
/// are read past but otherwise ignored, since there's no path to check them
/// against. Nested `descriptors:` (per-key sub-limits) aren't supported -
/// every descriptor in the list is expected to carry its own `rate_limit`
/// directly, which is how a flat per-path config is written in practice.
///
/// This format has no `burst` concept, so every rule parses with `burst`
/// unset; the usual missing-burst check still applies in strict mode.
pub fn parse(source: &str) -> Result<Vec<Rule>, ParseError> {
    let mut rules = Vec::new();
    let mut pending: Option<PendingDescriptor> = None;

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("- ") {
            if let Some(p) = pending.take() {
                finish(p, &mut rules)?;
            }
            let key = rest.trim().strip_prefix("key:").ok_or_else(|| ParseError {
                line: line_no,
                message: "descriptor entry must start with 'key: <name>'".to_string(),
            })?;
            pending = Some(PendingDescriptor {
                key: unquote(key.trim()).to_string(),
                key_line: line_no,
                value: None,
                unit: None,
                requests_per_unit: None,
            });
            continue;
        }

        if line.strip_prefix("domain:").is_some() {
            if let Some(p) = pending.take() {
                finish(p, &mut rules)?;
            }
            continue;
        }

        if line == "descriptors:" || line == "rate_limit:" {
            continue;
        }

        if let Some(rest) = line.strip_prefix("value:") {
            let p = pending.as_mut().ok_or_else(|| ParseError {
                line: line_no,
                message: "'value' outside of any descriptor entry".to_string(),
            })?;
            p.value = Some(Field {
                value: unquote(rest.trim()).to_string(),
                line: line_no,
            });
            continue;
        }

        if let Some(rest) = line.strip_prefix("unit:") {
            let p = pending.as_mut().ok_or_else(|| ParseError {
                line: line_no,
                message: "'unit' outside of any descriptor entry".to_string(),
            })?;
            p.unit = Some((unquote(rest.trim()).to_string(), line_no));
            continue;
        }

        if let Some(rest) = line.strip_prefix("requests_per_unit:") {
            let p = pending.as_mut().ok_or_else(|| ParseError {
                line: line_no,
                message: "'requests_per_unit' outside of any descriptor entry".to_string(),
            })?;
            p.requests_per_unit = Some(Field {
                value: unquote(rest.trim()).to_string(),
                line: line_no,
            });
            continue;
        }

        // Any other line (other descriptor fields, control-plane metadata
        // ratelint has no opinion on) is read past rather than rejected.
    }

    if let Some(p) = pending.take() {
        finish(p, &mut rules)?;
    }

    Ok(rules)
}

fn finish(p: PendingDescriptor, rules: &mut Vec<Rule>) -> Result<(), ParseError> {
    if p.key != "path" {
        return Ok(());
    }

    let value = p.value.ok_or_else(|| ParseError {
        line: p.key_line,
        message: "descriptor with key 'path' is missing a 'value'".to_string(),
    })?;
    let (unit, unit_line) = p.unit.ok_or_else(|| ParseError {
        line: p.key_line,
        message: "descriptor with key 'path' is missing a 'rate_limit.unit'".to_string(),
    })?;
    let requests_per_unit = p.requests_per_unit.ok_or_else(|| ParseError {
        line: p.key_line,
        message: "descriptor with key 'path' is missing a 'rate_limit.requests_per_unit'"
            .to_string(),
    })?;
    let window = unit_seconds(&unit, unit_line)?;

    rules.push(Rule {
        name: value.value.clone(),
        line: p.key_line,
        path: Some(value),
        limit: Some(requests_per_unit),
        window: Some(Field {
            value: window.to_string(),
            line: unit_line,
        }),
        burst: None,
    });
    Ok(())
}

fn unit_seconds(unit: &str, line: usize) -> Result<u64, ParseError> {
    match unit {
        "second" => Ok(1),
        "minute" => Ok(60),
        "hour" => Ok(3600),
        "day" => Ok(86400),
        other => Err(ParseError {
            line,
            message: format!(
                "unsupported rate_limit unit '{}', expected 'second', 'minute', 'hour', or 'day'",
                other
            ),
        }),
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn unquote(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_path_descriptor_into_a_rule() {
        let source = "\
domain: api-gateway
descriptors:
  - key: path
    value: /api/login
    rate_limit:
      unit: second
      requests_per_unit: 5
";
        let rules = parse(source).unwrap();
        assert_eq!(rules.len(), 1);
        let rule = &rules[0];
        assert_eq!(rule.path.as_ref().unwrap().value, "/api/login");
        assert_eq!(rule.limit.as_ref().unwrap().value, "5");
        assert_eq!(rule.window.as_ref().unwrap().value, "1");
        assert!(rule.burst.is_none());
    }

    #[test]
    fn converts_minute_hour_and_day_units_to_seconds() {
        let source = "\
descriptors:
  - key: path
    value: /a
    rate_limit:
      unit: minute
      requests_per_unit: 10
  - key: path
    value: /b
    rate_limit:
      unit: hour
      requests_per_unit: 100
  - key: path
    value: /c
    rate_limit:
      unit: day
      requests_per_unit: 1000
";
        let rules = parse(source).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].window.as_ref().unwrap().value, "60");
        assert_eq!(rules[1].window.as_ref().unwrap().value, "3600");
        assert_eq!(rules[2].window.as_ref().unwrap().value, "86400");
    }

    #[test]
    fn ignores_descriptors_with_a_different_key() {
        let source = "\
descriptors:
  - key: remote_address
    rate_limit:
      unit: second
      requests_per_unit: 1000
  - key: path
    value: /api/login
    rate_limit:
      unit: second
      requests_per_unit: 5
";
        let rules = parse(source).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].path.as_ref().unwrap().value, "/api/login");
    }

    #[test]
    fn rejects_a_path_descriptor_missing_value() {
        let source = "\
descriptors:
  - key: path
    rate_limit:
      unit: second
      requests_per_unit: 5
";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("missing a 'value'"));
    }

    #[test]
    fn rejects_a_path_descriptor_missing_rate_limit() {
        let source = "\
descriptors:
  - key: path
    value: /api/login
";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("missing a 'rate_limit.unit'"));
    }

    #[test]
    fn rejects_an_unsupported_unit() {
        let source = "\
descriptors:
  - key: path
    value: /api/login
    rate_limit:
      unit: fortnight
      requests_per_unit: 5
";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("unsupported rate_limit unit 'fortnight'"));
    }

    #[test]
    fn rejects_fields_outside_any_descriptor() {
        let source = "value: /api/login\n";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("outside of any descriptor entry"));
    }

    #[test]
    fn strips_comments_and_quoted_values() {
        let source = "\
descriptors:
  - key: path # match on the request path
    value: \"/api/login\"
    rate_limit:
      unit: 'second'
      requests_per_unit: 5
";
        let rules = parse(source).unwrap();
        assert_eq!(rules[0].path.as_ref().unwrap().value, "/api/login");
        assert_eq!(rules[0].window.as_ref().unwrap().value, "1");
    }

    #[test]
    fn a_domain_line_closes_a_pending_descriptor() {
        let source = "\
descriptors:
  - key: path
    value: /api/login
    rate_limit:
      unit: second
      requests_per_unit: 5
domain: api-gateway
";
        let rules = parse(source).unwrap();
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn empty_source_yields_no_rules() {
        let rules = parse("").unwrap();
        assert!(rules.is_empty());
    }
}
