//! Render engine (bead P3.1) — `{{VAR}}` substitution per plan §7.
//!
//! Behavioral spec:
//! - Placeholders `{{VAR_NAME}}`, regex `\{\{\s*([a-zA-Z0-9_]+)\s*\}\}`,
//!   case-sensitive exact-name matching.
//! - Unfilled placeholders pass through untouched.
//! - Defaults precedence: explicit > dynamic (CWD, PROJECT_NAME) > declared.
//! - `file` type reads file contents capped at 100KB with truncation notice;
//!   `path` passes the path string through.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};

use crate::model::Prompt;

/// Max bytes read for a `file`-type variable (plan §7).
pub const MAX_FILE_VAR_BYTES: usize = 102_400;

pub const TRUNCATION_SUFFIX_TEMPLATE: &str = "[File truncated to 102400 bytes from {} bytes]";

/// A resolved value ready for substitution.
#[derive(Debug, Clone)]
pub enum ValueSource {
    /// Explicitly provided via --VAR= or context file/stdin.
    Explicit,
    /// Derived from CWD/PROJECT_NAME at call time.
    Dynamic,
    /// Prompt-declared default_value.
    Declared,
}

/// Pure render: substitute placeholders in content using the given values.
/// Values already have defaults/dynamic applied by the caller. Unfilled
/// placeholders remain verbatim. Returns rendered string + names filled.
pub fn render_content(content: &str, values: &HashMap<String, String>) -> (String, Vec<String>) {
    let mut filled = Vec::new();
    // Single pass over placeholder occurrences: user-provided values are
    // inserted literally and never re-scanned for {{...}} patterns.
    let mut result = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'{'
            && parse_placeholder(&content[i..]).is_some()
        {
            let (name, end) = parse_placeholder(&content[i..]).expect("checked above");
            match values.get(name) {
                Some(v) => {
                    result.push_str(v);
                    filled.push(name.to_string());
                }
                None => result.push_str(&content[i..i + end]),
            }
            i += end;
            continue;
        }
        // Advance one UTF-8 char.
        let ch_len = utf8_char_len(bytes[i]);
        result.push_str(&content[i..i + ch_len]);
        i += ch_len;
    }
    (result, filled)
}

/// Parse `{{NAME}}` from the start of s; returns (name, consumed_bytes).
fn parse_placeholder(s: &str) -> Option<(&str, usize)> {
    let close = s.find("}}")?;
    let inner = &s[2..close];
    let name = inner.trim();
    if name.is_empty() {
        return None;
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((name, close + 2))
}

fn utf8_char_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Extract placeholder names from content (order of first appearance, deduped).
pub fn extract_variables(content: &str) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'{'
            && parse_placeholder(&content[i..]).is_some()
        {
            let (name, end) = parse_placeholder(&content[i..]).expect("checked");
            if seen.insert(name.to_string()) {
                out.push(name.to_string());
            }
            i += end;
            continue;
        }
        let ch_len = utf8_char_len(bytes[i]);
        i += ch_len;
    }
    out
}

/// Build the value map for a prompt: explicit > declared-default > nothing.
/// Dynamic defaults (CWD, PROJECT_NAME) are injected here too.
pub fn build_values(
    prompt: &Prompt,
    explicit: &[(String, String)],
    cwd: &Path,
) -> Result<HashMap<String, String>> {
    let mut map: HashMap<String, String> = HashMap::new();

    // Declared defaults first (lowest priority below explicit).
    for var in &prompt.variables {
        if let Some(d) = &var.default {
            map.insert(var.name.clone(), d.clone());
        }
    }

    // Dynamic defaults from cwd.
    map.insert("CWD".into(), cwd.display().to_string());
    if let Some(project) = project_name(cwd) {
        map.insert("PROJECT_NAME".into(), project);
    }

    // Explicit wins last; process special types.
    for (name, value) in explicit {
        if let Some(var) = prompt.variables.iter().find(|v| v.name == *name) {
            let resolved = match var.var_type {
                crate::model::VariableType::File => read_file_var(value)?,
                _ => value.clone(),
            };
            map.insert(name.clone(), resolved);
        } else {
            map.insert(name.clone(), value.clone());
        }
    }
    Ok(map)
}

fn project_name(cwd: &Path) -> Option<String> {
    cwd.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Read a `file`-type variable value with the 100KB cap and notice suffix.
fn read_file_var(path_str: &str) -> Result<String> {
    let path = Path::new(path_str);
    let meta = std::fs::metadata(path)
        .with_context(|| format!("file variable points to unreadable path '{path_str}'"))?;
    let total = meta.len() as usize;
    if total <= MAX_FILE_VAR_BYTES {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading file variable '{path_str}'"))?;
        return Ok(content);
    }
    // Truncated read: first MAX bytes.
    let f =
        std::fs::File::open(path).with_context(|| format!("opening file variable '{path_str}'"))?;
    use std::io::Read;
    let mut handle = std::io::BufReader::new(f);
    let mut buf = vec![0u8; MAX_FILE_VAR_BYTES];
    handle.read_exact(&mut buf)?;
    let mut content = String::from_utf8_lossy(&buf).into_owned();
    content.push('\n');
    content.push_str(&format!(
        "[File truncated to {MAX_FILE_VAR_BYTES} bytes from {total} bytes]"
    ));
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PromptVariable, VariableType};

    #[test]
    fn substitutes_declared_placeholders() {
        let (out, filled) = render_content(
            "Review this {{LANGUAGE}} code:\n{{CODE}}",
            &HashMap::from([
                ("LANGUAGE".to_string(), "Rust".to_string()),
                ("CODE".to_string(), "fn main() {}".to_string()),
            ]),
        );
        assert!(out.contains("Review this Rust code:"));
        assert!(out.contains("fn main() {}"));
        assert_eq!(filled.len(), 2);
    }

    #[test]
    fn unfilled_placeholders_pass_through_verbatim() {
        let (out, filled) = render_content("keep {{MISSING}} intact", &HashMap::new());
        assert_eq!(out, "keep {{MISSING}} intact");
        assert!(filled.is_empty());
    }

    #[test]
    fn whitespace_inside_braces_tolerated() {
        let (out, _) = render_content(
            "{{  NAME  }}",
            &HashMap::from([("NAME".to_string(), "v".to_string())]),
        );
        assert_eq!(out, "v");
    }

    #[test]
    fn case_sensitive_matching() {
        let (out, filled) = render_content(
            "{{FOO}} vs {{foo}}",
            &HashMap::from([("foo".to_string(), "lower".to_string())]),
        );
        assert_eq!(out, "{{FOO}} vs lower");
        assert_eq!(filled, vec!["foo"]);
    }

    #[test]
    fn values_containing_braces_not_rescanned() {
        let (out, _) = render_content(
            "{{A}}",
            &HashMap::from([("A".to_string(), "{{INJECTED}}".to_string())]),
        );
        assert_eq!(out, "{{INJECTED}}", "value inserted literally once");
    }

    #[test]
    fn extraction_dedupes_and_orders() {
        let vars = extract_variables("{{B}} {{A}} {{B}} tail {{C_1}}");
        assert_eq!(
            vars,
            vec!["B".to_string(), "A".to_string(), "C_1".to_string()]
        );
    }

    #[test]
    fn invalid_placeholder_names_ignored() {
        let (out, _) = render_content("{{has-dash}} {{has space}} {{}}", &HashMap::new());
        assert_eq!(out, "{{has-dash}} {{has space}} {{}}");
    }

    fn var(name: &str, t: VariableType) -> PromptVariable {
        PromptVariable {
            name: name.into(),
            var_type: t,
            required: false,
            description: None,
            default: None,
        }
    }

    #[test]
    fn build_values_precedence_explicit_over_dynamic_over_default() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Prompt::new("x", "X", "{{CWD}} {{PROJECT_NAME}}");
        let vals = build_values(&p, &[], tmp.path()).unwrap();
        assert_eq!(vals["CWD"], tmp.path().display().to_string());
        assert_eq!(
            vals["PROJECT_NAME"],
            tmp.path().file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn file_var_reads_content() {
        let tmp = tempfile::tempdir().unwrap();
        let fp = tmp.path().join("f.txt");
        std::fs::write(&fp, "FILE CONTENT").unwrap();
        let mut p = Prompt::new("x", "X", "{{F}}");
        p.variables = vec![var("F", VariableType::File)];
        let vals = build_values(
            &p,
            &[("F".into(), fp.display().to_string())],
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(vals["F"], "FILE CONTENT");
    }

    #[test]
    fn file_var_truncates_over_cap_with_notice() {
        let tmp = tempfile::tempdir().unwrap();
        let fp = tmp.path().join("big.bin");
        let big = vec![b'a'; MAX_FILE_VAR_BYTES + 500];
        std::fs::write(&fp, &big).unwrap();
        let mut p = Prompt::new("x", "X", "{{F}}");
        p.variables = vec![var("F", VariableType::File)];
        let vals = build_values(
            &p,
            &[("F".into(), fp.display().to_string())],
            Path::new("/"),
        )
        .unwrap();
        let v = &vals["F"];
        assert!(v.len() < big.len());
        assert!(
            v.ends_with("[File truncated to 102400 bytes from 102900 bytes]"),
            "got suffix: {:?}",
            &v[v.len().saturating_sub(60)..]
        );
    }

    #[test]
    fn missing_file_var_is_error() {
        let mut p = Prompt::new("x", "X", "{{F}}");
        p.variables = vec![var("F", VariableType::File)];
        let e = build_values(
            &p,
            &[("F".into(), "/nonexistent/xx".into())],
            Path::new("/"),
        )
        .unwrap_err();
        assert!(e.to_string().contains("unreadable"));
    }

    #[test]
    fn path_var_passes_through_without_reading() {
        let mut p = Prompt::new("x", "X", "{{P}}");
        p.variables = vec![var("P", VariableType::Path)];
        let vals = build_values(
            &p,
            &[("P".into(), "/does/not/exist".into())],
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(vals["P"], "/does/not/exist");
    }
}
