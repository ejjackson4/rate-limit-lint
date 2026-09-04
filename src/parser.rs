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
