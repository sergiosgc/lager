//! Shared test fixture: a fresh temporary copy of the repo's `database.sqlite`,
//! emptied so every test starts from an empty inventory. All data the tests
//! write is synthetic.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

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
