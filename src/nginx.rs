use crate::parser::{Field, ParseError, Rule};
use std::collections::HashMap;

struct ZoneDef {
    limit: String,
    window: String,
    line: usize,
}

/// Parses the slice of nginx config syntax ratelint understands: `limit_req_zone`
/// directives (which define a rate) and `limit_req` directives inside `location`
/// blocks (which apply one). Everything else - `server`, `http`, `if`, `listen`,
/// and so on - is tracked only enough to know when a block closes; it isn't
/// otherwise inspected.
///
/// Each directive and each block opener/closer must be on its own line. A
/// `location /x { limit_req zone=y; }` packed onto one line is not recognized;
/// that matches how these files are written in practice, and keeps this parser
/// as simple as the native rule format's.
///
/// A zone's `path`/`limit`/`window` are read back at their own definition or
/// usage line, so lint findings and `--fix`-style diagnostics still point at
/// something meaningful in the source file.
pub fn parse(source: &str) -> Result<Vec<Rule>, ParseError> {
    let mut zones: HashMap<String, ZoneDef> = HashMap::new();
    let mut rules = Vec::new();
    let mut block_stack: Vec<Option<(String, usize)>> = Vec::new();

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(header) = line.strip_prefix("limit_req_zone") {
            if starts_with_boundary(header) {
                let (name, limit, window) = parse_zone_def(header, line_no)?;
                if let Some(existing) = zones.get(&name) {
                    return Err(ParseError {
                        line: line_no,
                        message: format!(
                            "zone '{}' already defined at line {}",
                            name, existing.line
                        ),
                    });
                }
                zones.insert(
                    name,
                    ZoneDef {
                        limit,
                        window,
                        line: line_no,
                    },
                );
                continue;
            }
        }

        if let Some(rest) = line.strip_prefix("limit_req") {
            if starts_with_boundary(rest) {
                let (zone_name, burst) = parse_limit_req(rest, line_no)?;
                let (path, path_line) = block_stack
                    .iter()
                    .rev()
                    .find_map(|entry| entry.clone())
                    .ok_or_else(|| ParseError {
                        line: line_no,
                        message: "'limit_req' directive outside of any 'location' block"
                            .to_string(),
                    })?;
                let zone = zones.get(&zone_name).ok_or_else(|| ParseError {
                    line: line_no,
                    message: format!(
                        "'limit_req' references zone '{}' with no matching 'limit_req_zone'",
                        zone_name
                    ),
                })?;

                let rule = Rule {
                    name: zone_name,
                    line: line_no,
                    path: Some(Field {
                        value: path,
                        line: path_line,
                    }),
                    limit: Some(Field {
                        value: zone.limit.clone(),
                        line: zone.line,
                    }),
                    window: Some(Field {
                        value: zone.window.clone(),
                        line: zone.line,
                    }),
                    burst: burst.map(|value| Field { value, line: line_no }),
                };
                rules.push(rule);
                continue;
            }
        }

        if line.ends_with('{') {
            let header = line[..line.len() - 1].trim();
            if let Some(rest) = header.strip_prefix("location") {
                if starts_with_boundary(rest) {
                    let path = rest.trim().rsplit(char::is_whitespace).next().unwrap_or("");
                    if path.is_empty() {
                        return Err(ParseError {
                            line: line_no,
                            message: "'location' block is missing a path".to_string(),
                        });
                    }
                    block_stack.push(Some((path.to_string(), line_no)));
                    continue;
                }
            }
            block_stack.push(None);
            continue;
        }

        if line == "}" {
            if block_stack.pop().is_none() {
                return Err(ParseError {
                    line: line_no,
                    message: "unmatched closing brace '}'".to_string(),
                });
            }
        }
    }

    if !block_stack.is_empty() {
        return Err(ParseError {
            line: source.lines().count(),
            message: "unclosed block: missing closing '}'".to_string(),
        });
    }

    Ok(rules)
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// True if `s` is empty or starts with whitespace, i.e. the directive that
/// was just stripped off ended there rather than continuing into a longer
/// word (`limit_req` matching inside `limit_req_status`).
fn starts_with_boundary(s: &str) -> bool {
    s.chars().next().map_or(true, |c| c.is_whitespace())
}

fn parse_zone_def(header: &str, line_no: usize) -> Result<(String, String, String), ParseError> {
    let mut name = None;
    let mut rate = None;
    for token in header.split_whitespace() {
        if let Some(z) = token.strip_prefix("zone=") {
            name = Some(z.split(':').next().unwrap_or(z).to_string());
        } else if let Some(r) = token.strip_prefix("rate=") {
            rate = Some(r.trim_end_matches(';').to_string());
        }
    }
    let name = name.ok_or_else(|| ParseError {
        line: line_no,
        message: "'limit_req_zone' is missing a 'zone=' argument".to_string(),
    })?;
    let rate = rate.ok_or_else(|| ParseError {
        line: line_no,
        message: "'limit_req_zone' is missing a 'rate=' argument".to_string(),
    })?;
    let (limit, window) = parse_rate(&rate, line_no)?;
    Ok((name, limit, window))
}

fn parse_rate(rate: &str, line_no: usize) -> Result<(String, String), ParseError> {
    let digit_end = rate
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rate.len());
    if digit_end == 0 {
        return Err(ParseError {
            line: line_no,
            message: format!("invalid rate '{}', expected e.g. '5r/s' or '30r/m'", rate),
        });
    }
    let (num, unit) = rate.split_at(digit_end);
    let window = match unit {
        "r/s" => "1",
        "r/m" => "60",
        other => {
            return Err(ParseError {
                line: line_no,
                message: format!(
                    "unsupported rate unit '{}', expected 'r/s' or 'r/m'",
                    other
                ),
            })
        }
    };
    Ok((num.to_string(), window.to_string()))
}

fn parse_limit_req(rest: &str, line_no: usize) -> Result<(String, Option<String>), ParseError> {
    let mut zone = None;
    let mut burst = None;
    for token in rest.split_whitespace() {
        if let Some(z) = token.strip_prefix("zone=") {
            zone = Some(z.trim_end_matches(';').to_string());
        } else if let Some(b) = token.strip_prefix("burst=") {
            burst = Some(b.trim_end_matches(';').to_string());
        }
    }
    let zone = zone.ok_or_else(|| ParseError {
        line: line_no,
        message: "'limit_req' is missing a 'zone=' argument".to_string(),
    })?;
    Ok((zone, burst))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zone_and_location_into_a_rule() {
        let source = "\
http {
    limit_req_zone $binary_remote_addr zone=login:10m rate=5r/s;

    server {
        location /api/login {
            limit_req zone=login burst=10 nodelay;
        }
    }
}
";
        let rules = parse(source).unwrap();
        assert_eq!(rules.len(), 1);
        let rule = &rules[0];
        assert_eq!(rule.name, "login");
        assert_eq!(rule.path.as_ref().unwrap().value, "/api/login");
        assert_eq!(rule.limit.as_ref().unwrap().value, "5");
        assert_eq!(rule.window.as_ref().unwrap().value, "1");
        assert_eq!(rule.burst.as_ref().unwrap().value, "10");
    }

    #[test]
    fn converts_requests_per_minute_to_a_60s_window() {
        let source = "\
limit_req_zone $binary_remote_addr zone=search:10m rate=300r/m;
location /api/search {
    limit_req zone=search;
}
";
        let rules = parse(source).unwrap();
        assert_eq!(rules[0].limit.as_ref().unwrap().value, "300");
        assert_eq!(rules[0].window.as_ref().unwrap().value, "60");
        assert!(rules[0].burst.is_none());
    }

    #[test]
    fn rejects_limit_req_outside_a_location_block() {
        let source = "\
limit_req_zone $binary_remote_addr zone=login:10m rate=5r/s;
limit_req zone=login;
";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("outside of any 'location' block"));
    }

    #[test]
    fn rejects_limit_req_for_an_undefined_zone() {
        let source = "\
location /api/login {
    limit_req zone=login;
}
";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("no matching 'limit_req_zone'"));
    }

    #[test]
    fn rejects_duplicate_zone_definitions() {
        let source = "\
limit_req_zone $binary_remote_addr zone=login:10m rate=5r/s;
limit_req_zone $binary_remote_addr zone=login:10m rate=10r/s;
";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("already defined at line 1"));
    }

    #[test]
    fn rejects_unclosed_blocks() {
        let source = "server {\n    location /x {\n";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("unclosed block"));
    }

    #[test]
    fn rejects_unmatched_closing_brace() {
        let source = "}\n";
        let err = parse(source).unwrap_err();
        assert!(err.message.contains("unmatched closing brace"));
    }

    #[test]
    fn ignores_unrelated_directives_and_similarly_named_ones() {
        let source = "\
worker_processes 1;
limit_req_status 429;
limit_req_zone $binary_remote_addr zone=login:10m rate=5r/s;
server {
    listen 80;
    server_name example.com;
    location /api/login {
        limit_req zone=login burst=10;
    }
}
";
        let rules = parse(source).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "login");
    }

    #[test]
    fn strips_comments_before_matching_directives() {
        let source = "\
limit_req_zone $binary_remote_addr zone=login:10m rate=5r/s; # five per second
location /api/login { # the login endpoint
    limit_req zone=login; # no burst configured
}
";
        let rules = parse(source).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].path.as_ref().unwrap().value, "/api/login");
    }

    #[test]
    fn a_zone_used_by_multiple_locations_yields_multiple_rules() {
        let source = "\
limit_req_zone $binary_remote_addr zone=api:10m rate=5r/s;
location /api/a {
    limit_req zone=api;
}
location /api/b {
    limit_req zone=api;
}
";
        let rules = parse(source).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].path.as_ref().unwrap().value, "/api/a");
        assert_eq!(rules[1].path.as_ref().unwrap().value, "/api/b");
    }
}
