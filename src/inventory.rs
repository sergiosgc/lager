//! The Inventory module: the one place that knows the rules of the inventory.
//!
//! It opens the database (refusing files it doesn't understand), reads
//! entities, and performs every change in a single transaction. A change
//! either succeeds, possibly with warnings, or fails with an [`Error`] that
//! says which field was wrong and why.

mod hosts;
mod models;
mod server_index;
mod variables;

pub use hosts::{Host, HostDeletion, normalize_host_name};
pub use variables::OwnVariable;

use hosts::*;
use server_index::*;
use variables::*;

use std::fmt;
use std::path::{Path, PathBuf};

use toasty::stmt::Value;

/// The file opened when the database path given is a directory.
pub const DATABASE_FILE_NAME: &str = "database.sqlite";

/// The Variable holding a Host's Server Index.
pub const SERVER_INDEX: &str = "server_index";

/// The only `db_version` Lager knows how to read and write.
pub const SUPPORTED_DB_VERSION: i64 = 1;

/// The result of a successful change, with anything the operator should
/// notice but that didn't stop it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done<T> {
    pub value: T,
    pub warnings: Vec<String>,
}

impl<T> Done<T> {
    fn ok(value: T) -> Self {
        Done {
            value,
            warnings: Vec::new(),
        }
    }
}

/// Why a change was not made.
#[derive(Debug)]
pub enum Error {
    /// The input broke a rule. `field` names the input that was wrong.
    Invalid {
        field: &'static str,
        message: String,
    },
    /// The entity the change was aimed at doesn't exist.
    NotFound(String),
    /// The database answered in a shape lager didn't expect.
    Unexpected(String),
    Database(toasty::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid { field, message } => write!(f, "{field}: {message}"),
            Error::NotFound(what) => write!(f, "{what} not found"),
            Error::Unexpected(what) => write!(f, "unexpected database result: {what}"),
            Error::Database(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<toasty::Error> for Error {
    fn from(e: toasty::Error) -> Self {
        Error::Database(e)
    }
}

/// Why the database could not be opened.
#[derive(Debug)]
pub enum OpenError {
    Missing(PathBuf),
    UnsupportedVersion { found: Option<i64> },
    Database(toasty::Error),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::Missing(path) => {
                write!(f, "database file {} does not exist", path.display())
            }
            OpenError::UnsupportedVersion { found: Some(v) } => write!(
                f,
                "db_version is {v}, but lager only supports db_version {SUPPORTED_DB_VERSION}"
            ),
            OpenError::UnsupportedVersion { found: None } => write!(
                f,
                "db_version holds no version, but lager only supports db_version {SUPPORTED_DB_VERSION}"
            ),
            OpenError::Database(e) => write!(f, "cannot read the database: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

impl From<toasty::Error> for OpenError {
    fn from(e: toasty::Error) -> Self {
        OpenError::Database(e)
    }
}

/// An open inventory database.
#[derive(Clone)]
pub struct Inventory {
    db: toasty::Db,
}

impl Inventory {
    /// Opens an existing inventory database, refusing a missing file or one
    /// whose `db_version` isn't [`SUPPORTED_DB_VERSION`] (ADR 0002).
    ///
    /// A directory stands for the [`DATABASE_FILE_NAME`] inside it.
    pub async fn open(path: &Path) -> Result<Self, OpenError> {
        let path = if path.is_dir() {
            path.join(DATABASE_FILE_NAME)
        } else {
            path.to_path_buf()
        };
        // Checked first: the SQLite driver would otherwise create the file.
        if !path.is_file() {
            return Err(OpenError::Missing(path));
        }
        let mut db = open_without_foreign_keys(&path).await?;
        let found = read_db_version(&mut db).await?;
        if found != Some(SUPPORTED_DB_VERSION) {
            return Err(OpenError::UnsupportedVersion { found });
        }
        Ok(Inventory { db })
    }
}

/// `"1 Group"`, `"2 Groups"`.
fn plural(n: i64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Runs a raw `SELECT count(*) ... WHERE host = ?1` query for one Host.
///
/// A result in any other shape is an error rather than 0: a wrong 0 would
/// let a delete through and orphan rows (ADR 0002, ADR 0003).
async fn count_rows_for_host(
    db: &mut impl toasty::Executor,
    sql: &str,
    host_id: i64,
) -> Result<i64, Error> {
    let rows = toasty::sql::query(sql).bind(host_id).exec(db).await?;
    let mut rows = rows.into_iter();
    match (rows.next(), rows.next()) {
        (Some(Value::Record(r)), None) => match r.into_iter().collect::<Vec<_>>().as_slice() {
            [Value::I64(n)] => Ok(*n),
            other => Err(Error::Unexpected(format!("{sql} returned {other:?}"))),
        },
        other => Err(Error::Unexpected(format!("{sql} returned {other:?}"))),
    }
}
fn invalid(field: &'static str, message: impl Into<String>) -> Error {
    Error::Invalid {
        field,
        message: message.into(),
    }
}

/// Opens the database with SQLite foreign-key enforcement off, because two
/// references in the frozen schema point at tables that don't exist
/// (ADR 0002, ADR 0003).
///
/// TEMPORARY workaround, see https://github.com/sergiosgc/lager/issues/17:
/// toasty-driver-sqlite 0.11.0 has no pragma setting, and the SQLite that
/// rusqlite bundles enforces foreign keys on every new connection.
/// `PRAGMA foreign_keys` is per-connection, so the pool is pinned to one
/// connection (never recycled: lifetime and idle caps default to none) and
/// the pragma is set once on it. Replace with the driver's
/// `.foreign_keys(false)` once a published Toasty release has it.
async fn open_without_foreign_keys(path: &Path) -> toasty::Result<toasty::Db> {
    let driver = toasty_driver_sqlite::Sqlite::open(path);
    let mut db = toasty::Db::builder()
        .models(toasty::models!(
            models::Host,
            models::Variable,
            models::HostVariable
        ))
        .max_pool_size(1)
        .build(driver)
        .await?;
    toasty::sql::statement("PRAGMA foreign_keys = OFF")
        .exec(&mut db)
        .await?;
    Ok(db)
}

async fn read_db_version(db: &mut toasty::Db) -> Result<Option<i64>, OpenError> {
    let rows = toasty::sql::query("SELECT version FROM db_version")
        .exec(db)
        .await?;
    Ok(rows.into_iter().find_map(|row| match row {
        Value::Record(r) => r.into_iter().next().and_then(|v| match v {
            Value::I64(n) => Some(n),
            _ => None,
        }),
        _ => None,
    }))
}

#[cfg(test)]
mod tests {
    //! Unit-level check of the foreign-key workaround (#17). It needs the raw
    //! connection state, which the public interface rightly doesn't expose.

    use super::*;

    async fn foreign_keys(db: &mut toasty::Db) -> Value {
        let rows = toasty::sql::query("PRAGMA foreign_keys")
            .exec(db)
            .await
            .unwrap();
        match rows.into_iter().next() {
            Some(Value::Record(r)) => r.into_iter().next().unwrap(),
            other => panic!("unexpected PRAGMA result {other:?}"),
        }
    }

    fn temp_copy() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("database.sqlite");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("database.sqlite"),
            &path,
        )
        .unwrap();
        (dir, path)
    }

    /// Catches a Toasty upgrade or pool change that silently turns
    /// enforcement back on, e.g. by handing out a fresh connection after a
    /// failed statement or a transaction.
    #[tokio::test]
    async fn foreign_keys_stay_off_after_errors_and_transactions() {
        let (_dir, path) = temp_copy();
        let inventory = Inventory::open(&path).await.unwrap();
        let mut db = inventory.db.clone();
        assert_eq!(foreign_keys(&mut db).await, Value::I64(0));

        assert!(
            toasty::sql::statement("NOT VALID SQL")
                .exec(&mut db)
                .await
                .is_err()
        );
        assert_eq!(foreign_keys(&mut db).await, Value::I64(0));

        {
            let mut tx = db.transaction().await.unwrap();
            models::Host::create()
                .name("web1")
                .exec(&mut tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        {
            let mut tx = db.transaction().await.unwrap();
            assert!(
                toasty::sql::statement("NOT VALID SQL")
                    .exec(&mut tx)
                    .await
                    .is_err()
            );
            tx.rollback().await.unwrap();
        }
        assert_eq!(foreign_keys(&mut db).await, Value::I64(0));

        // A row whose reference points at a missing table: accepted only with
        // enforcement off (host_group.parent references "host_groups").
        toasty::sql::statement("INSERT INTO host_group(name, parent) VALUES ('orphan', 999)")
            .exec(&mut db)
            .await
            .unwrap();
    }
}
