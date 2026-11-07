//! The Inventory module: the one place that knows the rules of the inventory.
//!
//! It opens the database (refusing files it doesn't understand), reads
//! entities, and performs every change in a single transaction. A change
//! either succeeds, possibly with warnings, or fails with an [`Error`] that
//! says which field was wrong and why.

mod models;

use std::fmt;
use std::path::{Path, PathBuf};

use toasty::stmt::Value;

/// The only `db_version` Lager knows how to read and write.
pub const SUPPORTED_DB_VERSION: i64 = 1;

/// A machine Ansible manages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: i64,
    pub name: String,
}

impl From<models::Host> for Host {
    fn from(h: models::Host) -> Self {
        Host {
            id: h.id,
            name: h.name,
        }
    }
}

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
    Database(toasty::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid { field, message } => write!(f, "{field}: {message}"),
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
    pub async fn open(path: &Path) -> Result<Self, OpenError> {
        // Checked first: the SQLite driver would otherwise create the file.
        if !path.is_file() {
            return Err(OpenError::Missing(path.to_path_buf()));
        }
        let mut db = open_without_foreign_keys(path).await?;
        let found = read_db_version(&mut db).await?;
        if found != Some(SUPPORTED_DB_VERSION) {
            return Err(OpenError::UnsupportedVersion { found });
        }
        Ok(Inventory { db })
    }

    /// All Hosts, sorted by name.
    pub async fn hosts(&self) -> Result<Vec<Host>, Error> {
        let mut db = self.db.clone();
        let mut hosts: Vec<Host> = models::Host::all()
            .exec(&mut db)
            .await?
            .into_iter()
            .map(Host::from)
            .collect();
        hosts.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(hosts)
    }

    /// The Host with this id, if there is one.
    pub async fn host(&self, id: i64) -> Result<Option<Host>, Error> {
        let mut db = self.db.clone();
        let host = models::Host::filter_by_id(id).first().exec(&mut db).await?;
        Ok(host.map(Host::from))
    }

    /// Creates a Host. Its name, with surrounding whitespace removed, must be
    /// non-empty and not already used by another Host.
    pub async fn create_host(&self, name: &str) -> Result<Done<Host>, Error> {
        let name = name.trim();
        if name.is_empty() {
            return Err(invalid("name", "A Host name can't be empty."));
        }
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let existing = models::Host::filter(models::Host::fields().name().eq(name))
            .first()
            .exec(&mut tx)
            .await?;
        if existing.is_some() {
            return Err(invalid(
                "name",
                format!("A Host named {name} already exists."),
            ));
        }
        let host = models::Host::create().name(name).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(host.into()))
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
        .models(toasty::models!(models::Host))
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
