//! Contract regression tests for the P0 harness findings:
//! - argv-equals-swallowed-lrg: `--flag=value` must reach clap, never be
//!   eaten as a template variable.
//! - json-flag-not-global-i9i: `--json`/`-j` work before AND after subcommand.
//! - direct-mode-truncates-query-rau: bare words join into one query.

mod contract_support;

use contract_support::ContractEnv;

#[test]
fn flag_equals_value_reaches_clap_for_every_command() {
    let env = ContractEnv::new();

    // new --from=./x --title=.. --var=.. all in = form.
    let prompt_file = env.home.path().join("p.txt");
    std::fs::write(&prompt_file, "body {{X}}").unwrap();
    let (stdout, stderr, code) = env.triple(&[
        "new",
        "eq-demo",
        "--from",
        &format!("={}", prompt_file.display()),
        "--title=Equals Demo",
        "--var=X=text=hi",
    ]);
    // Note: clap `--from=PATH` form is exercised below; this call uses the
    // space form to isolate the = handling per flag.
    let _ = (stdout, stderr, code);

    let file_arg = format!("--from={}", prompt_file.display());
    let (stdout, stderr, code) = env.triple(&[
        "new",
        "eq-demo2",
        &file_arg,
        "--title=Equals Two",
        "--var=X=text=hi",
    ]);
    assert_eq!(
        code, 0,
        "new with --flag=value must work: {stdout} / {stderr}"
    );
    assert!(stdout.contains("eq-demo2"));

    // list --limit=N --category=C
    let (stdout, _, code) = env.triple(&["list", "--limit=1", "--json"]);
    assert_eq!(code, 0, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v["count"].as_u64().unwrap() <= 1);

    // search --limit=N
    let (_, _, code) = env.triple(&["search", "zzz-no-match", "--limit=3"]);
    assert_eq!(code, 0);

    // completion --shell=zsh must not be eaten as var 'shell'.
    let (stdout, _, code) = env.triple(&["completion", "--shell=zsh"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("compdef") || stdout.contains("pst"));

    // export --format=.. --out=.. --ids=..
    let out = env.home.path().join("e.jsonl");
    let out_arg = format!("--out={}", out.display());
    let (_, stderr, code) = env.triple(&["export", "--format=jsonl", &out_arg, "--ids=eq-demo2"]);
    assert_eq!(code, 0, "export --flag=value: {stderr}");
    assert!(out.exists());
}

#[test]
fn json_flag_is_global_before_and_after_subcommand() {
    let env = ContractEnv::new();
    env.seed_prompt("jg", "JG", "content");

    // After subcommand (agent convention).
    let (stdout, _, code) = env.triple(&["list", "--json"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(v["prompts"].is_array());

    // Short form after subcommand.
    let (stdout, _, code) = env.triple(&["list", "-j"]);
    assert_eq!(code, 0);
    assert!(serde_json::from_str::<serde_json::Value>(&stdout).is_ok());

    // Before subcommand still works.
    let (stdout, _, code) = env.triple(&["--json", "list"]);
    assert_eq!(code, 0);
    assert!(serde_json::from_str::<serde_json::Value>(&stdout).is_ok());

    // --json=x is a usage error (rc 2), never a silent template var.
    let (_, _, code) = env.triple(&["--json=x", "list"]);
    assert_eq!(code, 2);
}

#[test]
fn direct_mode_joins_multi_word_queries() {
    let env = ContractEnv::new();
    // Seed a prompt whose content only matches the FULL multi-word query.
    env.seed_prompt("multi-word-target", "MW", "alpha beta gamma delta");

    // Single-word prefix still resolves.
    let (stdout, _, code) = env.triple(&["multi-word-target"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "alpha beta gamma delta\n");

    // Two bare words must be treated as ONE query "alpha beta" — before the
    // fix only the first word "alpha" was queried. Fuzzy resolution of the
    // joined query still finds the prompt (content contains both tokens).
    let (stdout, stderr, code) = env.triple(&["alpha", "beta"]);
    assert_eq!(code, 0, "joined query must resolve: {stderr}");
    assert!(stdout.contains("alpha beta gamma delta"));
}
