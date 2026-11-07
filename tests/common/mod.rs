//! Shared test fixture: a fresh temporary copy of the repo's empty
//! `database.sqlite`. All data the tests write is synthetic.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn reference_db() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("database.sqlite")
}

/// A temporary directory holding `database.sqlite`, copied from the reference
/// database. The directory is removed when the fixture is dropped.
pub struct TempDb {
    pub dir: tempfile::TempDir,
    pub path: PathBuf,
}

pub fn temp_db() -> TempDb {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("database.sqlite");
    std::fs::copy(reference_db(), &path).unwrap();
    TempDb { dir, path }
}

pub async fn open(db: &TempDb) -> lager::inventory::Inventory {
    lager::inventory::Inventory::open(&db.path).await.unwrap()
}
