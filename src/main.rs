mod envoy;
mod fix;
mod lint;
mod nginx;
mod parser;

use std::env;
use std::fs;
use std::process::ExitCode;

#[derive(PartialEq, Eq)]
enum Format {
    Ini,
    Nginx,
    Envoy,
}

fn main() -> ExitCode {
    let mut path: Option<String> = None;
    let mut lenient = false;
    let mut json = false;
    let mut fix_mode = false;
    let mut format = Format::Ini;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--lenient" => lenient = true,
            "--json" => json = true,
            "--fix" => fix_mode = true,
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other if other.starts_with("--format=") => {
                format = match &other["--format=".len()..] {
                    "ini" => Format::Ini,
                    "nginx" => Format::Nginx,
                    "envoy" => Format::Envoy,
                    other => {
                        eprintln!(
                            "unknown format '{}': expected 'ini', 'nginx', or 'envoy'",
                            other
                        );
                        print_usage();
                        return ExitCode::from(2);
                    }
                };
            }
            other if path.is_none() => path = Some(other.to_string()),
            other => {
                eprintln!("unexpected argument: {}", other);
                print_usage();
                return ExitCode::from(2);
            }
        }
    }

    let path = match path {
        Some(p) => p,
        None => {
            print_usage();
            return ExitCode::from(2);
        }
    };

    if fix_mode && format != Format::Ini {
        eprintln!("--fix is only supported for the default ini format");
        return ExitCode::from(2);
    }

    let source = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {}", path, e);
            return ExitCode::from(2);
        }
    };

    let parse_result = match format {
        Format::Ini => parser::parse(&source),
        Format::Nginx => nginx::parse(&source),
        Format::Envoy => envoy::parse(&source),
    };

    let mut rules = match parse_result {
        Ok(r) => r,
        Err(e) => {
            if json {
                println!(
                    "{{\"file\":{},\"parse_error\":{{\"line\":{},\"message\":{}}}}}",
                    json_string(&path),
                    e.line,
                    json_string(&e.message)
                );
            } else {
                eprintln!("{}:{}: {}", path, e.line, e.message);
            }
            return ExitCode::from(2);
        }
    };

    let mut fixes = Vec::new();
    if fix_mode {
        let (fixed_source, applied) = fix::apply(&source, &rules);
        if !applied.is_empty() {
            if let Err(e) = fs::write(&path, &fixed_source) {
                eprintln!("{}: {}", path, e);
                return ExitCode::from(2);
            }
            rules = match parser::parse(&fixed_source) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{}:{}: {}", path, e.line, e.message);
                    return ExitCode::from(2);
                }
            };
            fixes = applied;
        }
    }

    let findings = lint::check(&rules, lenient);
    let has_error = findings
        .iter()
        .any(|f| matches!(f.severity, lint::Severity::Error));

    if json {
        print_json(&path, rules.len(), &findings, &fixes);
    } else {
        for applied in &fixes {
            println!(
                "{}:{}: fix: added 'burst = {}' to rule '{}'",
                path, applied.line, applied.value, applied.rule
            );
        }

        for finding in &findings {
            println!(
                "{}:{}: {}: {}",
                path,
                finding.line,
                finding.severity.label(),
                finding.message
            );
        }

        if findings.is_empty() {
            println!(
                "{}: no findings ({} rule{})",
                path,
                rules.len(),
                if rules.len() == 1 { "" } else { "s" }
            );
        }
    }

    if has_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_json(path: &str, rule_count: usize, findings: &[lint::Finding], fixes: &[fix::Fix]) {
    let mut out = String::new();
    out.push_str("{\"file\":");
    out.push_str(&json_string(path));
    out.push_str(",\"rules\":");
    out.push_str(&rule_count.to_string());
    out.push_str(",\"fixed\":[");
    for (i, applied) in fixes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"line\":");
        out.push_str(&applied.line.to_string());
        out.push_str(",\"rule\":");
        out.push_str(&json_string(&applied.rule));
        out.push_str(",\"field\":\"burst\",\"value\":");
        out.push_str(&applied.value.to_string());
        out.push('}');
    }
    out.push_str("],\"findings\":[");
    for (i, finding) in findings.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"line\":");
        out.push_str(&finding.line.to_string());
        out.push_str(",\"severity\":");
        out.push_str(&json_string(finding.severity.label()));
        out.push_str(",\"message\":");
        out.push_str(&json_string(&finding.message));
        out.push('}');
    }
    out.push_str("]}");
    println!("{}", out);
}

/// Encodes a string as a JSON string literal, including the surrounding quotes.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn print_usage() {
    eprintln!("usage: ratelint [--lenient] [--json] [--fix] [--format=ini|nginx|envoy] <rules-file>");
    eprintln!();
    eprintln!("checks a rate-limit rule file for missing fields, bad values,");
    eprintln!("duplicate paths, and burst/limit inconsistencies.");
    eprintln!();
    eprintln!("--lenient      relax strict-only checks (missing burst, absurd limits)");
    eprintln!("--json         print findings as a single JSON object on stdout");
    eprintln!("--fix          fill in auto-fillable issues in place (currently: default");
    eprintln!("               a missing 'burst' to 'limit') before linting; ini format only");
    eprintln!("--format=FMT   input format: 'ini' (default), 'nginx' (limit_req config), or");
    eprintln!("               'envoy' (ratelimit descriptor config)");
}

#[cfg(test)]
mod tests {
    use super::json_string;

    #[test]
    fn escapes_quotes_and_backslashes() {
        assert_eq!(json_string("say \"hi\"\\now"), "\"say \\\"hi\\\"\\\\now\"");
    }

    #[test]
    fn escapes_newlines_tabs_and_carriage_returns() {
        assert_eq!(json_string("a\nb\tc\rd"), "\"a\\nb\\tc\\rd\"");
    }

    #[test]
    fn escapes_other_control_characters_as_unicode_points() {
        let bell = char::from_u32(7).unwrap();
        assert_eq!(json_string(&bell.to_string()), "\"\\u0007\"");
    }

    #[test]
    fn leaves_plain_text_untouched() {
        assert_eq!(json_string("/api/login"), "\"/api/login\"");
    }
}
