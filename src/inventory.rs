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

/// The file opened when the database path given is a directory.
pub const DATABASE_FILE_NAME: &str = "database.sqlite";

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

/// A Variable set directly on an entity, with its value as JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnVariable {
    pub name: String,
    pub value: String,
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
    /// The entity the change was aimed at doesn't exist.
    NotFound(String),
    Database(toasty::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid { field, message } => write!(f, "{field}: {message}"),
            Error::NotFound(what) => write!(f, "{what} not found"),
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

    /// Creates a Host. Its name is normalised (see [`normalize_host_name`])
    /// and must then be non-empty and not already used by another Host.
    pub async fn create_host(&self, name: &str) -> Result<Done<Host>, Error> {
        let name = normalize_host_name(name);
        if name.is_empty() {
            return Err(invalid(
                "name",
                "A Host name can't be empty: it needs at least one letter, digit, \"-\" or \"_\".",
            ));
        }
        let name = name.as_str();
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

    /// A Host's Own Variables, sorted by name.
    pub async fn host_variables(&self, host_id: i64) -> Result<Vec<OwnVariable>, Error> {
        let mut db = self.db.clone();
        let values =
            models::HostVariable::filter(models::HostVariable::fields().host_id().eq(host_id))
                .exec(&mut db)
                .await?;
        own_variables(
            &mut db,
            values
                .into_iter()
                .map(|v| (v.variable_id, v.value))
                .collect(),
        )
        .await
    }

    /// Sets a Host's Own Variable, adding the Variable name if it is new.
    ///
    /// The name must not be blank, and is used exactly as given. The value
    /// must parse as JSON, and is stored exactly as given, as `cli/si` does.
    pub async fn set_host_variable(
        &self,
        host_id: i64,
        name: &str,
        value: &str,
    ) -> Result<Done<OwnVariable>, Error> {
        valid_variable_name(name)?;
        valid_json(value)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        let variable_id = match variable_by_name(&mut tx, name).await? {
            Some(variable) => variable.id,
            None => {
                models::Variable::create()
                    .name(name)
                    .exec(&mut tx)
                    .await?
                    .id
            }
        };
        match host_variable(variable_id, host_id)
            .first()
            .exec(&mut tx)
            .await?
        {
            Some(mut row) => row.update().value(value).exec(&mut tx).await?,
            None => {
                models::HostVariable::create()
                    .variable_id(variable_id)
                    .host_id(host_id)
                    .value(value)
                    .exec(&mut tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(Done::ok(OwnVariable {
            name: name.to_string(),
            value: value.to_string(),
        }))
    }

    /// Removes a Host's Own Variable. The Variable name stays, for any other
    /// entity using it. Removing a Variable the Host doesn't have does nothing.
    pub async fn remove_host_variable(&self, host_id: i64, name: &str) -> Result<Done<()>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        if let Some(variable) = variable_by_name(&mut tx, name).await? {
            host_variable(variable.id, host_id)
                .delete()
                .exec(&mut tx)
                .await?;
        }
        tx.commit().await?;
        Ok(Done::ok(()))
    }

    /// Every Variable name, sorted, whether or not any entity has a value
    /// for it.
    pub async fn variable_names(&self) -> Result<Vec<String>, Error> {
        let mut db = self.db.clone();
        let mut names: Vec<String> = models::Variable::all()
            .exec(&mut db)
            .await?
            .into_iter()
            .map(|v| v.name)
            .collect();
        names.sort();
        Ok(names)
    }
}

/// Pairs each `(variable id, value)` with its Variable name, sorted by name.
/// A value whose Variable row is missing is left out, as `cli/si`'s joins do.
async fn own_variables(
    db: &mut toasty::Db,
    values: Vec<(i64, String)>,
) -> Result<Vec<OwnVariable>, Error> {
    let ids: Vec<i64> = values.iter().map(|(id, _)| *id).collect();
    let names: std::collections::HashMap<i64, String> =
        models::Variable::filter(toasty::stmt::in_list(models::Variable::fields().id(), ids))
            .exec(db)
            .await?
            .into_iter()
            .map(|v| (v.id, v.name))
            .collect();
    let mut own: Vec<OwnVariable> = values
        .into_iter()
        .filter_map(|(id, value)| {
            names.get(&id).map(|name| OwnVariable {
                name: name.clone(),
                value,
            })
        })
        .collect();
    own.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(own)
}

/// Fails with [`Error::NotFound`] unless the Host exists.
async fn existing_host(tx: &mut toasty::Transaction<'_>, host_id: i64) -> Result<(), Error> {
    match models::Host::filter_by_id(host_id).first().exec(tx).await? {
        Some(_) => Ok(()),
        None => Err(Error::NotFound(format!("Host {host_id}"))),
    }
}

/// The Variable called exactly `name`, if there is one.
async fn variable_by_name(
    tx: &mut toasty::Transaction<'_>,
    name: &str,
) -> Result<Option<models::Variable>, Error> {
    Ok(
        models::Variable::filter(models::Variable::fields().name().eq(name))
            .first()
            .exec(tx)
            .await?,
    )
}

/// The query for one Host's value of one Variable.
fn host_variable(
    variable_id: i64,
    host_id: i64,
) -> toasty::schema::QueryMany<models::HostVariable> {
    models::HostVariable::filter(
        models::HostVariable::fields()
            .variable_id()
            .eq(variable_id)
            .and(models::HostVariable::fields().host_id().eq(host_id)),
    )
}

/// Rejects a blank Variable name.
fn valid_variable_name(name: &str) -> Result<(), Error> {
    if name.trim().is_empty() {
        return Err(invalid("name", "A Variable name can't be empty."));
    }
    Ok(())
}

/// Checks that a Variable value is a JSON document.
fn valid_json(value: &str) -> Result<(), Error> {
    serde_json::from_str::<serde_json::Value>(value)
        .map(|_| ())
        .map_err(|e| invalid("value", format!("This is not valid JSON: {e}.")))
}

/// The name a Host is stored under: lowercased, keeping only `a`-`z`,
/// `0`-`9`, `-` and `_`. Anything else, including spaces, dots and
/// non-ASCII letters, is dropped.
pub fn normalize_host_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| matches!(c, 'a'..='z' | '0'..='9' | '-' | '_'))
        .collect()
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
