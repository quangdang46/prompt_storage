//! Contract regression tests for import validation beads:
//! - import-id-validation-bypass-8nn: unsafe ids rejected like `new`.
//! - import-validation-gaps-whg: merge is atomic; replace validates
//!   aliases; new/import reject ids that shadow existing aliases.

mod contract_support;

use contract_support::ContractEnv;

fn write_jsonl(env: &ContractEnv, name: &str, lines: &[&str]) -> String {
    let path = env.home.path().join(name);
    let mut text =
        String::from(r#"{"_meta":{"version":"0","count":0,"exported_at":"x","schema_version":1}}"#);
    text.push('\n');
    for l in lines {
        text.push_str(l);
        text.push('\n');
    }
    std::fs::write(&path, text).unwrap();
    path.display().to_string()
}

fn prompt_ids(env: &ContractEnv) -> Vec<String> {
    let (stdout, _, code) = env.triple(&["list", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let mut ids: Vec<String> = v["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

#[test]
fn import_rejects_noncanonical_ids() {
    let env = ContractEnv::new();
    for bad in ["../evil", "a/b", "x y", "UP", "a.b", "-lead", "trail-"] {
        let file = write_jsonl(
            &env,
            "bad.jsonl",
            &[&format!(r#"{{"id":"{bad}","title":"T","content":"c"}}"#)],
        );
        let (_, stderr, code) = env.triple(&["import", &file, "--merge"]);
        assert_eq!(code, 1, "id '{bad}' must be rejected: {stderr}");
        assert!(stderr.contains("invalid prompt"), "{stderr}");
        assert!(prompt_ids(&env).is_empty());
    }
}

#[test]
fn merge_import_is_atomic_on_mid_file_failure() {
    let env = ContractEnv::new();
    env.seed_prompt("pre", "Pre", "p");
    // 'taken' alias belongs to 'pre'; 'newprompt' tries to claim it.
    env.triple(&["alias", "pre", "taken"]);
    let file = write_jsonl(
        &env,
        "conflict.jsonl",
        &[
            r#"{"id":"newprompt","title":"N","content":"x","aliases":["taken"]}"#,
            r#"{"id":"lateone","title":"L","content":"y"}"#,
        ],
    );
    let (_, stderr, code) = env.triple(&["import", &file, "--merge"]);
    assert_eq!(code, 1, "{stderr}");
    // NOTHING from the file may be committed — not even 'lateone'.
    assert_eq!(prompt_ids(&env), vec!["pre".to_string()]);
}

#[test]
fn replace_import_validates_aliases_like_add_alias() {
    let env = ContractEnv::new();
    env.seed_prompt("owner", "O", "x");
    env.seed_prompt("other", "T", "y");
    // Alias 'OTHER' collides (NOCASE) with canonical id 'other'.
    let file = write_jsonl(
        &env,
        "r.jsonl",
        &[
            r#"{"id":"owner","title":"O","content":"x","aliases":["OTHER"]}"#,
            r#"{"id":"other","title":"T","content":"y"}"#,
        ],
    );
    let (_, stderr, code) = env.triple(&["import", &file, "--replace"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("id_conflict"), "{stderr}");
    // Replace is atomic: pre-existing prompts survive a failed import.
    assert_eq!(
        prompt_ids(&env),
        vec!["other".to_string(), "owner".to_string()]
    );
}

#[test]
fn new_rejects_id_that_shadows_existing_alias() {
    let env = ContractEnv::new();
    env.seed_prompt("owner", "O", "x");
    env.triple(&["alias", "owner", "taken-name"]);
    // Creating a prompt whose id equals the alias would shadow it forever.
    let prompt_file = env.home.path().join("p.txt");
    std::fs::write(&prompt_file, "body").unwrap();
    let file_arg = format!("--from={}", prompt_file.display());
    let (_, stderr, code) = env.triple(&["new", "taken-name", &file_arg]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(v["error"], "alias_conflict");
}

#[test]
fn merge_import_rejects_id_shadowing_existing_alias() {
    let env = ContractEnv::new();
    env.seed_prompt("owner", "O", "x");
    env.triple(&["alias", "owner", "shadowed"]);
    let file = write_jsonl(
        &env,
        "s.jsonl",
        &[r#"{"id":"shadowed","title":"S","content":"z"}"#],
    );
    let (_, stderr, code) = env.triple(&["import", &file, "--merge"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("alias_conflict"), "{stderr}");
    assert_eq!(prompt_ids(&env), vec!["owner".to_string()]);
}

#[test]
fn import_rejects_nul_bytes() {
    let env = ContractEnv::new();
    // \u0000 inside JSON string decodes to a real NUL char.
    let file = write_jsonl(
        &env,
        "nul.jsonl",
        &[r#"{"id":"nul-id","title":"T","content":"a\u0000b"}"#],
    );
    let (_, stderr, code) = env.triple(&["import", &file, "--merge"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(prompt_ids(&env).is_empty());
}
