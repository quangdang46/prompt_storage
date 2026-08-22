//! Argv pre-scan for dynamic `--VAR=value` tokens (plan §7).
//!
//! clap cannot declare arbitrary flags, so `main.rs` extracts variable
//! assignments BEFORE handing argv to clap.
//!
//! Token grammar: `^--[A-Za-z_][A-Za-z0-9_]*=(.*)$` — split on FIRST `=` only.
//! Reserved-flag precedence: names matching real flags are never variables;
//! `--json=x` is a hard error.

/// Result of the pre-scan: cleaned argv + extracted variables.
#[derive(Debug, Default)]
pub struct Prescan {
    /// Remaining argv (program name preserved at index 0 if given).
    pub argv: Vec<String>,
    /// Extracted (name, value) pairs in order of appearance.
    pub vars: Vec<(String, String)>,
    /// Names that collided with reserved flags (`--json=x` etc.).
    pub reserved_errors: Vec<String>,
}

/// Flags pst itself defines. A `--NAME=value` token whose NAME matches one
/// of these (case-sensitive) is NOT a variable; it is an error because bare
/// flags never take inline values in our CLI grammar.
pub const RESERVED_FLAGS: &[&str] = &["json", "no-color", "help", "version"];

/// Check whether a token looks like `--NAME=value`.
fn parse_var_token(token: &str) -> Option<(&str, &str)> {
    let rest = token.strip_prefix("--")?;
    let eq = rest.find('=')?;
    let name = &rest[..eq];
    // Grammar: first char alpha or _, rest alnum or _.
    if name.is_empty() {
        return None;
    }
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((name, &rest[eq + 1..]))
}

/// Pre-scan raw args (excluding program name). Returns cleaned args and vars.
pub fn prescan(args: &[String]) -> Prescan {
    let mut out = Prescan::default();
    let mut after_double_dash = false;

    for arg in args {
        if after_double_dash {
            out.argv.push(arg.clone());
            continue;
        }
        if arg == "--" {
            after_double_dash = true;
            out.argv.push(arg.clone());
            continue;
        }
        if let Some((name, value)) = parse_var_token(arg) {
            if RESERVED_FLAGS.contains(&name) {
                out.reserved_errors.push(arg.clone());
            } else {
                out.vars.push((name.to_string(), value.to_string()));
            }
            continue;
        }
        out.argv.push(arg.clone());
    }
    out
}

/// Case-insensitive near-miss check for typo warnings: does any declared
/// variable differ from `provided` only by case?
pub fn case_mismatch<'a>(
    provided: &str,
    declared: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    declared
        .into_iter()
        .find(|d| d.eq_ignore_ascii_case(provided))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn extracts_basic_assignment() {
        let p = prescan(&v(&["render", "--CODE=x.rs", "demo"]));
        assert_eq!(p.vars, vec![("CODE".to_string(), "x.rs".to_string())]);
        assert_eq!(p.argv, vec!["render", "demo"]);
    }

    #[test]
    fn empty_value_is_ok() {
        let p = prescan(&v(&["--FOO="]));
        assert_eq!(p.vars, vec![("FOO".to_string(), String::new())]);
    }

    #[test]
    fn second_equals_stays_in_value() {
        let p = prescan(&v(&["--FOO=a=b"]));
        assert_eq!(p.vars, vec![("FOO".to_string(), "a=b".to_string())]);
    }

    #[test]
    fn lowercase_and_underscore_names_accepted() {
        let p = prescan(&v(&["--foo=bar", "--A_B=123", "--_hidden=1"]));
        assert_eq!(p.vars.len(), 3);
        assert_eq!(p.vars[0], ("foo".to_string(), "bar".to_string()));
        assert_eq!(p.vars[1], ("A_B".to_string(), "123".to_string()));
        assert_eq!(p.vars[2], ("_hidden".to_string(), "1".to_string()));
    }

    #[test]
    fn digits_leading_name_rejected() {
        // --2fast has digit-first name → not a variable, stays in argv
        let p = prescan(&v(&["--2fast=x"]));
        assert!(p.vars.is_empty());
        assert_eq!(p.argv, vec!["--2fast=x"]);
    }

    #[test]
    fn dash_in_name_rejected() {
        // --no-color=x has a dash in the NAME portion → not var grammar,
        // but no-color IS reserved... as a whole-token it fails the grammar
        // (dash not allowed), so it stays in argv for clap to reject.
        let p = prescan(&v(&["--no-color=x"]));
        assert!(p.vars.is_empty());
        assert!(p.reserved_errors.is_empty());
        assert_eq!(p.argv, vec!["--no-color=x"]);
    }

    #[test]
    fn reserved_flag_with_value_is_hard_error() {
        // --json=x and --no-color=true are reserved errors.
        // --version=3: "version" is reserved BUT the token fails var grammar
        // only if name invalid — "version" is valid, so it IS a reserved error.
        let p = prescan(&v(&["--json=x", "--no-color=true", "--version=3"]));
        assert_eq!(
            p.reserved_errors.len(),
            2,
            "no-color has dash → not var grammar"
        );
    }

    #[test]
    fn double_dash_ends_variable_extraction() {
        let p = prescan(&v(&["render", "--", "--AFTER=still-positional"]));
        assert!(p.vars.is_empty());
        assert_eq!(p.argv, vec!["render", "--", "--AFTER=still-positional"]);
    }

    #[test]
    fn multiple_vars_preserve_order() {
        let p = prescan(&v(&["--B=2", "--A=1", "--C=3"]));
        let names: Vec<_> = p.vars.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["B", "A", "C"]);
    }

    #[test]
    fn plain_flags_pass_through() {
        let p = prescan(&v(&["list", "--json", "-j", "search", "--limit", "5"]));
        assert!(p.vars.is_empty());
        assert_eq!(p.argv.len(), 6);
    }

    #[test]
    fn values_with_spaces_arrive_intact() {
        // Shell already unquoted this into ONE argv entry:
        let p = prescan(&v(&["--MSG=hello world  with spaces"]));
        assert_eq!(
            p.vars,
            vec![("MSG".to_string(), "hello world  with spaces".to_string())]
        );
    }

    #[test]
    fn case_mismatch_detection() {
        let declared = ["CODE", "LANGUAGE"];
        assert_eq!(case_mismatch("code", declared), Some("CODE".to_string()));
        assert_eq!(
            case_mismatch("language", declared),
            Some("LANGUAGE".to_string())
        );
        assert_eq!(case_mismatch("unknown", declared), None);
    }
}
