//! Contract regression tests for alias-rm-resolution-63t:
//! - `alias` resolves its target id like `show` (aliases/prefix work)
//! - `rm` reports ambiguous/not_found faithfully
//! - `rm` without --force off-TTY refuses (destructive op needs consent)
//! - alias batches are atomic; unalias with no args is a usage error

mod contract_support;

use contract_support::ContractEnv;

#[test]
fn alias_accepts_resolvable_target_not_just_canonical_id() {
    let env = ContractEnv::new();
    env.seed_prompt("real-target", "RT", "body");
    env.triple(&["alias", "real-target", "first"]);

    // 'first' is an alias, not a canonical id — must still resolve.
    let (stdout, stderr, code) = env.triple(&["alias", "first", "second"]);
    assert_eq!(code, 0, "{stdout} / {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["to"], "real-target");

    // 'second' now resolves to the same prompt.
    let (stdout, _, code) = env.triple(&["second"]);
    assert_eq!(code, 0);
    assert_eq!(stdout, "body\n");
}

#[test]
fn alias_batch_is_atomic() {
    let env = ContractEnv::new();
    env.seed_prompt("p-one", "P1", "1");
    env.seed_prompt("p-two", "P2", "2");
    // 'p-two' is a canonical id → id_conflict on the SECOND alias; the
    // first alias in the batch must be rolled back too.
    let (_, stderr, code) = env.triple(&["alias", "p-one", "ok-alias", "p-two"]);
    assert_eq!(code, 1, "{stderr}");
    // 'ok-alias' must NOT exist: batch rolled back. Prove it via unalias
    // (a bare query could still hit via prefix/fuzzy, so don't use that).
    let (_, stderr, ucode) = env.triple(&["unalias", "ok-alias"]);
    assert_eq!(ucode, 1, "alias must not exist after rollback: {stderr}");
}

#[test]
fn rm_reports_ambiguous_and_not_found_faithfully() {
    let env = ContractEnv::new();
    env.seed_prompt("amb-a", "A", "1");
    env.seed_prompt("amb-b", "B", "2");

    let (_, stderr, code) = env.triple(&["rm", "amb", "--force"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(v["error"], "ambiguous", "stderr={stderr}");

    let (_, stderr, code) = env.triple(&["rm", "zzz-nothing", "--force"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(v["error"], "not_found", "stderr={stderr}");
}

#[test]
fn rm_without_force_off_tty_requires_confirmation() {
    let env = ContractEnv::new();
    env.seed_prompt("doomed2", "D", "bye");

    // Piped stdin (non-TTY): must refuse without --force.
    let (_, stderr, code) = env.triple(&["rm", "doomed2"]);
    assert_eq!(code, 1);
    let v: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(v["error"], "confirmation_required", "stderr={stderr}");

    // Prompt still exists.
    let (_, _, code) = env.triple(&["doomed2"]);
    assert_eq!(code, 0);

    // --force deletes.
    let (stdout, _, code) = env.triple(&["rm", "doomed2", "--force"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("\"deleted\":true"));
}

#[test]
fn unalias_and_alias_with_no_args_are_usage_errors() {
    let env = ContractEnv::new();
    let (_, _, code) = env.triple(&["unalias"]);
    assert_eq!(code, 1);
    let (_, _, code) = env.triple(&["alias", "whatever"]);
    assert_eq!(code, 1);
}
