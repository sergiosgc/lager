//! Shared test fixture: a fresh temporary copy of the repo's `database.sqlite`,
//! emptied so every test starts from an empty inventory. All data the tests
//! write is synthetic.

#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn reference_db() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("database.sqlite")
}

/// Every table except `db_version`. The committed `database.sqlite` is empty,
/// but the working copy is also what `lager` edits by default during
/// development, so the copy is emptied rather than trusted to be.
const DATA_TABLES: [&str; 10] = [
    "host",
    "host_group",
    "host_group_membership",
    "service",
    "service_instance",
    "variable",
    "host_variable",
    "host_group_variable",
    "service_variable",
    "service_instance_variable",
];

/// A temporary directory holding an empty `database.sqlite` with the
/// reference schema. The directory is removed when the fixture is dropped.
pub struct TempDb {
    pub dir: tempfile::TempDir,
    pub path: PathBuf,
}

pub fn temp_db() -> TempDb {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("database.sqlite");
    std::fs::copy(reference_db(), &path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    // The schema's broken foreign keys (ADR 0002) make deletes fail otherwise.
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    for table in DATA_TABLES {
        conn.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    TempDb { dir, path }
}

pub async fn open(db: &TempDb) -> lager::inventory::Inventory {
    lager::inventory::Inventory::open(&db.path).await.unwrap()
}

/// Arranges data lager itself wouldn't write, the way cli/si or a deleted
/// Host might have left it.
pub fn sql(db: &TempDb, statement: &str) {
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn.execute_batch(statement).unwrap();
}

/// How many rows in `table` refer to `id` through `column`.
pub fn rows_referring_to(db: &TempDb, table: &str, column: &str, id: i64) -> i64 {
    rusqlite::Connection::open(&db.path)
        .unwrap()
        .query_row(
            &format!("SELECT count(*) FROM {table} WHERE {column} = ?1"),
            [id],
            |r| r.get(0),
        )
        .unwrap()
}

/// Why `cli/si` can't run here, if it can't: it needs Python 3 with PyYAML.
pub fn missing_python() -> Option<String> {
    match Command::new("python3").args(["-c", "import yaml"]).output() {
        Err(e) => Some(format!("python3 is not available ({e})")),
        Ok(out) if !out.status.success() => Some(format!(
            "python3 can't import PyYAML: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Ok(_) => None,
    }
}

/// True, after saying so, if `test` must be skipped because `cli/si` can't
/// run. Writes straight to stderr: the test harness only captures the print
/// macros, so the message shows even when the test passes.
pub fn skip_without_python(test: &str) -> bool {
    let Some(reason) = missing_python() else {
        return false;
    };
    let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {reason}");
    true
}

/// Runs `cli/si --database <database> <args>` and parses its JSON output.
pub fn run_si(database: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("cli/si"))
        .arg("--database")
        .arg(database)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cli/si {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "cli/si {args:?} printed invalid JSON ({e}):\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}
