use std::fmt;

/// A single `key = value` line, keeping the line number for diagnostics.
#[derive(Debug)]
pub struct Field {
    pub value: String,
    pub line: usize,
}

/// One `[name]` section from a rules file.
#[derive(Debug)]
pub struct Rule {
    pub name: String,
    pub line: usize,
    pub path: Option<Field>,
    pub limit: Option<Field>,
    pub window: Option<Field>,
    pub burst: Option<Field>,
}

impl Rule {
    fn new(name: String, line: usize) -> Self {
        Rule {
            name,
            line,
            path: None,
            limit: None,
            window: None,
            burst: None,
        }
    }
}

#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// Parses the small INI-style dialect ratelint reads:
///
/// ```text
/// [rule_name]
/// path = /api/login
/// limit = 5
/// window = 60
/// burst = 10
/// ```
///
/// Comments start with `#`, blank lines are ignored, and every key must
/// live inside a `[section]`.
pub fn parse(source: &str) -> Result<Vec<Rule>, ParseError> {
    let mut rules = Vec::new();
    let mut current: Option<Rule> = None;

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err(ParseError {
                    line: line_no,
                    message: "unterminated section header, expected closing ']'".to_string(),
                });
            }
            let name = line[1..line.len() - 1].trim();
            if name.is_empty() {
                return Err(ParseError {
                    line: line_no,
                    message: "section header must not be empty".to_string(),
                });
            }
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
            current = Some(Rule::new(name.to_string(), line_no));
            continue;
        }

        let rule = match current.as_mut() {
            Some(r) => r,
            None => {
                return Err(ParseError {
                    line: line_no,
                    message: "key/value pair outside of any rule section".to_string(),
                })
            }
        };

        let (key, value) = match line.split_once('=') {
            Some(pair) => pair,
            None => {
                return Err(ParseError {
                    line: line_no,
                    message: format!("expected 'key = value', found '{}'", line),
                })
            }
        };
        let key = key.trim();
        let value = value.trim();
        let field = Field {
            value: value.to_string(),
            line: line_no,
        };

        match key {
            "path" => rule.path = Some(field),
            "limit" => rule.limit = Some(field),
            "window" => rule.window = Some(field),
            "burst" => rule.burst = Some(field),
            other => {
                return Err(ParseError {
                    line: line_no,
                    message: format!("unknown field '{}'", other),
                })
            }
        }
    }

    if let Some(rule) = current.take() {
        rules.push(rule);
    }

    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_complete_rule() {
        let rules = parse(
            "[login]\npath = /api/login\nlimit = 5\nwindow = 60\nburst = 10\n",
        )
        .unwrap();

        assert_eq!(rules.len(), 1);
        let rule = &rules[0];
        assert_eq!(rule.name, "login");
        assert_eq!(rule.line, 1);
        assert_eq!(rule.path.as_ref().unwrap().value, "/api/login");
        assert_eq!(rule.path.as_ref().unwrap().line, 2);
        assert_eq!(rule.limit.as_ref().unwrap().value, "5");
        assert_eq!(rule.window.as_ref().unwrap().value, "60");
        assert_eq!(rule.burst.as_ref().unwrap().value, "10");
    }

    #[test]
    fn ignores_blank_lines_and_comments() {
        let rules = parse(
            "# a rules file\n\n[login]\n# path comes first\npath = /api/login\n\nlimit = 5\nwindow = 60\n",
        )
        .unwrap();

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].path.as_ref().unwrap().value, "/api/login");
    }

    #[test]
    fn parses_multiple_sections() {
        let rules = parse(
            "[login]\npath = /api/login\nlimit = 5\nwindow = 60\n\n[search]\npath = /api/search\nlimit = 100\nwindow = 60\n",
        )
        .unwrap();

        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].name, "login");
        assert_eq!(rules[1].name, "search");
        assert_eq!(rules[1].line, 6);
    }

    #[test]
    fn trims_whitespace_around_keys_and_values() {
        let rules = parse("[login]\n  path   =   /api/login  \n").unwrap();
        assert_eq!(rules[0].path.as_ref().unwrap().value, "/api/login");
    }

    #[test]
    fn rejects_unterminated_section_header() {
        let err = parse("[login\npath = /api/login\n").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("unterminated"));
    }

    #[test]
    fn rejects_empty_section_header() {
        let err = parse("[]\n").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("must not be empty"));
    }

    #[test]
    fn rejects_key_value_outside_section() {
        let err = parse("path = /api/login\n").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("outside of any rule section"));
    }

    #[test]
    fn rejects_line_without_equals() {
        let err = parse("[login]\njust some text\n").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("expected 'key = value'"));
    }

    #[test]
    fn rejects_unknown_field() {
        let err = parse("[login]\nmethod = POST\n").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("unknown field 'method'"));
    }

    #[test]
    fn empty_source_yields_no_rules() {
        let rules = parse("").unwrap();
        assert!(rules.is_empty());
    }
}
