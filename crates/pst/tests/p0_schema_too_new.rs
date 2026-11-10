//! Contract regression test for schema-too-new-wrong-error-v0c:
//! a DB written by a newer pst must emit the pinned
//! {"error":"schema_too_new","db_version":N,"supported":M} payload.

mod contract_support;

use contract_support::ContractEnv;

#[test]
fn newer_schema_emits_schema_too_new_contract() {
    let env = ContractEnv::new();
    // Create a real v1 DB, then bump user_version to simulate a newer pst.
    env.seed_prompt("x", "X", "c");
    let db_path = env.home.path().join("store.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }

    let (stdout, stderr, code) = env.triple(&["list"]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty(), "error payload must not pollute stdout");
    let v: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|e| panic!("stderr must be contract JSON: {e}; raw={stderr:?}"));
    assert_eq!(v["error"], "schema_too_new");
    assert_eq!(v["db_version"], 99);
    assert_eq!(v["supported"], 1);
}
