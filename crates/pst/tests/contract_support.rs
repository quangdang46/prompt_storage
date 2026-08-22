//! Contract harness (bead P0.1): spawn the real `pst` binary with an
//! isolated PST_HOME and capture byte-exact stdout/stderr/exit triples.
//!
//! The contract — not the code — is the product API for AI agents.

use std::process::{Command, Output};
use tempfile::TempDir;

static BIN: std::sync::LazyLock<std::path::PathBuf> =
    std::sync::LazyLock::new(|| std::path::PathBuf::from(env!("CARGO_BIN_EXE_pst")));

/// Locate the compiled binary once per test process (avoids rebuild races).
pub fn bin() -> &'static std::path::Path {
    &BIN
}

/// One isolated environment: temp PST_HOME + a command builder.
pub struct ContractEnv {
    pub home: TempDir,
}

impl Default for ContractEnv {
    fn default() -> Self {
        Self {
            home: TempDir::new().expect("temp dir"),
        }
    }
}

impl ContractEnv {
    /// Alias for `Default::default()` — reads nicer in tests.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a Command with PST_HOME pointed at this env.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(bin());
        cmd.env("PST_HOME", self.home.path())
            .env("NO_COLOR", "1")
            .args(args);
        cmd
    }

    /// Run and capture the full output triple. stdout/stderr are piped so
    /// is_terminal() is false — exactly what an AI agent sees.
    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("spawn pst")
    }

    /// Assert helper: returns (stdout, stderr, exit_code).
    pub fn triple(&self, args: &[&str]) -> (String, String, i32) {
        let out = self.run(args);
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        )
    }

    /// Seed the library by writing directly through the lib API.
    /// (Contract tests exercise the BINARY; seeding uses the same storage
    /// code path the binary itself would use.)
    pub fn seed_prompt(&self, id: &str, title: &str, content: &str) {
        let root = self.home.path().to_path_buf();
        let db = pst::storage::database::Database::open(&root).expect("open seed db");
        let p = pst::model::Prompt::new(id, title, content);
        db.upsert_prompt(&p).expect("seed upsert");
    }
}
