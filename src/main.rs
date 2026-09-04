mod lint;
mod parser;

use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut path: Option<String> = None;
    let mut lenient = false;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--lenient" => lenient = true,
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
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

    let source = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {}", path, e);
            return ExitCode::from(2);
        }
    };

    let rules = match parser::parse(&source) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}:{}: {}", path, e.line, e.message);
            return ExitCode::from(2);
        }
    };

    let findings = lint::check(&rules, lenient);
    let mut has_error = false;

    for finding in &findings {
        if matches!(finding.severity, lint::Severity::Error) {
            has_error = true;
        }
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

    if has_error {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_usage() {
    eprintln!("usage: ratelint [--lenient] <rules-file>");
    eprintln!();
    eprintln!("checks a rate-limit rule file for missing fields, bad values,");
    eprintln!("duplicate paths, and burst/limit inconsistencies.");
    eprintln!();
    eprintln!("--lenient   relax strict-only checks (missing burst, absurd limits)");
}
