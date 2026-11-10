//! Contract regression test for db-opened-for-all-commands-9kr:
//! install/uninstall/completion/config must NOT create or require the DB.

mod contract_support;

use contract_support::ContractEnv;
use std::path::Path;

#[test]
fn db_free_commands_do_not_create_database() {
    let env = ContractEnv::new();
    let db = env.home.path().join("store.db");

    for args in [
        vec!["completion", "--shell=bash"],
        vec!["config", "list"],
        vec!["uninstall", "--personal"],
        vec!["install", "--personal"],
    ] {
        let (_, _, code) = env.triple(&args);
        assert!(
            !db.exists(),
            "{args:?} must not create store.db (rc={code})"
        );
    }
}

#[test]
fn uninstall_works_with_corrupt_database() {
    let env = ContractEnv::new();
    // Poison the DB file; DB-backed commands fail, DB-free ones must not.
    std::fs::write(env.home.path().join("store.db"), b"not a sqlite db").unwrap();

    let (_, _, code) = env.triple(&["uninstall", "--personal"]);
    assert_eq!(code, 0, "uninstall must not need a working DB");

    let (_, _, code) = env.triple(&["completion", "--shell=zsh"]);
    assert_eq!(code, 0);

    // Sanity: a DB command does fail on the corrupt file.
    let (_, _, code) = env.triple(&["list"]);
    assert_eq!(code, 1);
}

#[test]
fn config_does_not_touch_db_even_when_it_exists() {
    let env = ContractEnv::new();
    env.seed_prompt("x", "X", "c");
    assert!(Path::new(&env.home.path().join("store.db")).exists());
    let (_, _, code) = env.triple(&["config", "get", "output.color"]);
    assert_eq!(code, 0);
}
