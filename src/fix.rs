use crate::parser::Rule;

/// A single auto-applied fix, recorded so the CLI can report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub line: usize,
    pub rule: String,
    pub value: u64,
}

struct Pending {
    after: usize,
    rule: String,
    value: u64,
}

/// Fills in auto-fillable issues in `source` and returns the rewritten
/// source along with a record of what changed.
///
/// The only auto-fillable issue today is a missing `burst`: if a rule has a
/// valid positive `limit` but no `burst`, one is added with `burst` set to
/// `limit`, matching the default `--lenient` already assumes at lint time.
/// A rule with no `limit`, or a `limit` that doesn't parse as a positive
/// integer, is left alone; those are structural problems `--fix` can't
/// guess its way out of.
pub fn apply(source: &str, rules: &[Rule]) -> (String, Vec<Fix>) {
    let mut pending: Vec<Pending> = Vec::new();

    for rule in rules {
        if rule.burst.is_some() {
            continue;
        }
        let limit_field = match &rule.limit {
            Some(f) => f,
            None => continue,
        };
        let value: u64 = match limit_field.value.parse() {
            Ok(v) if v > 0 => v,
            _ => continue,
        };
        let after = [&rule.path, &rule.limit, &rule.window]
            .iter()
            .filter_map(|f| f.as_ref().map(|f| f.line))
            .max()
            .unwrap_or(rule.line);
        pending.push(Pending {
            after,
            rule: rule.name.clone(),
            value,
        });
    }

    if pending.is_empty() {
        return (source.to_string(), Vec::new());
    }

    // Insert from the bottom of the file up so an earlier insertion doesn't
    // shift the line index a later (in file order, earlier here) fix was
    // computed against.
    pending.sort_by(|a, b| b.after.cmp(&a.after));

    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    for p in &pending {
        lines.insert(p.after, format!("burst = {}", p.value));
    }

    let mut fixes: Vec<Fix> = pending
        .iter()
        .map(|p| Fix {
            line: p.after + 1,
            rule: p.rule.clone(),
            value: p.value,
        })
        .collect();
    fixes.sort_by_key(|f| f.line);

    let mut new_source = lines.join("\n");
    if source.ends_with('\n') {
        new_source.push('\n');
    }

    (new_source, fixes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    #[test]
    fn fills_missing_burst_from_limit() {
        let source = "[search]\npath = /api/search\nlimit = 100\nwindow = 60\n";
        let rules = parser::parse(source).unwrap();
        let (new_source, fixes) = apply(source, &rules);

        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].rule, "search");
        assert_eq!(fixes[0].value, 100);
        assert_eq!(fixes[0].line, 5);
        assert_eq!(
            new_source,
            "[search]\npath = /api/search\nlimit = 100\nwindow = 60\nburst = 100\n"
        );
    }

    #[test]
    fn leaves_rules_with_burst_untouched() {
        let source = "[login]\npath = /api/login\nlimit = 5\nwindow = 60\nburst = 10\n";
        let rules = parser::parse(source).unwrap();
        let (new_source, fixes) = apply(source, &rules);

        assert!(fixes.is_empty());
        assert_eq!(new_source, source);
    }

    #[test]
    fn skips_rules_missing_limit() {
        let source = "[login]\npath = /api/login\nwindow = 60\n";
        let rules = parser::parse(source).unwrap();
        let (new_source, fixes) = apply(source, &rules);

        assert!(fixes.is_empty());
        assert_eq!(new_source, source);
    }

    #[test]
    fn skips_rules_with_unparseable_limit() {
        let source = "[login]\npath = /api/login\nlimit = abc\nwindow = 60\n";
        let rules = parser::parse(source).unwrap();
        let (new_source, fixes) = apply(source, &rules);

        assert!(fixes.is_empty());
        assert_eq!(new_source, source);
    }

    #[test]
    fn fixes_multiple_rules_with_correct_line_numbers() {
        let source = "[login]\npath = /api/login\nlimit = 5\nwindow = 60\n\n[search]\npath = /api/search\nlimit = 100\nwindow = 60\n";
        let rules = parser::parse(source).unwrap();
        let (new_source, fixes) = apply(source, &rules);

        assert_eq!(fixes.len(), 2);
        assert_eq!(fixes[0].rule, "login");
        assert_eq!(fixes[0].line, 5);
        assert_eq!(fixes[1].rule, "search");
        assert_eq!(fixes[1].line, 10);

        let reparsed = parser::parse(&new_source).unwrap();
        assert_eq!(reparsed[0].burst.as_ref().unwrap().value, "5");
        assert_eq!(reparsed[1].burst.as_ref().unwrap().value, "100");
    }

    #[test]
    fn preserves_lack_of_trailing_newline() {
        let source = "[search]\npath = /api/search\nlimit = 100\nwindow = 60";
        let rules = parser::parse(source).unwrap();
        let (new_source, _) = apply(source, &rules);

        assert!(!new_source.ends_with('\n'));
        assert!(new_source.ends_with("burst = 100"));
    }
}
