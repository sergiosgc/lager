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

/// The Variable holding a Host's Server Index.
pub const SERVER_INDEX: &str = "server_index";

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

/// What deleting a Host removes: the Host and its Own Variables. While
/// `blockers` isn't empty, the Host can't be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostDeletion {
    pub host: Host,
    pub own_variables: Vec<OwnVariable>,
    /// Relations lager can't remove yet (Group memberships, Service
    /// Instances), each as a sentence for the operator.
    pub blockers: Vec<String>,
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

    /// Creates a Host with its Server Index. Its name is normalised (see
    /// [`normalize_host_name`]) and must then be non-empty and not already
    /// used by another Host.
    ///
    /// Without `server_index`, or with a blank one, the Host gets
    /// [`Inventory::next_server_index`].
    /// An override must parse as JSON and is stored as given; if another
    /// Host already has it, the Host is still created, with a warning.
    pub async fn create_host(
        &self,
        name: &str,
        server_index: Option<&str>,
    ) -> Result<Done<Host>, Error> {
        let name = normalized_host_name(name)?;
        let server_index = server_index.filter(|v| !v.trim().is_empty());
        if let Some(value) = server_index {
            valid_json(SERVER_INDEX, value)?;
        }
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        unused_host_name(&mut tx, &name, None).await?;
        let host = models::Host::create().name(name).exec(&mut tx).await?;
        let index = match server_index {
            Some(value) => value.to_string(),
            None => next_server_index(&mut tx).await?.to_string(),
        };
        write_host_variable(&mut tx, host.id, SERVER_INDEX, &index).await?;
        let warnings = host_warnings(&mut tx, host.id).await?;
        tx.commit().await?;
        Ok(Done {
            value: host.into(),
            warnings,
        })
    }

    /// Renames a Host, keeping its identity, so everything attached to it
    /// stays attached. The new name follows the same rules as on creation.
    pub async fn rename_host(&self, host_id: i64, name: &str) -> Result<Done<Host>, Error> {
        let name = normalized_host_name(name)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let mut host = existing_host(&mut tx, host_id).await?;
        unused_host_name(&mut tx, &name, Some(host_id)).await?;
        host.update().name(name).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(host.into()))
    }

    /// What deleting a Host would remove, or why it can't be deleted yet.
    /// [`Inventory::delete_host`] removes exactly this.
    pub async fn host_deletion(&self, host_id: i64) -> Result<HostDeletion, Error> {
        plan_host_deletion(&mut self.db.clone(), host_id).await
    }

    /// Deletes a Host and everything listed by [`Inventory::host_deletion`],
    /// in one transaction, returning what was removed. Variable names stay.
    /// Refused while the Host has relations lager can't remove yet.
    pub async fn delete_host(&self, host_id: i64) -> Result<Done<HostDeletion>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let plan = plan_host_deletion(&mut tx, host_id).await?;
        if !plan.blockers.is_empty() {
            return Err(invalid(
                "host",
                format!(
                    "{} can't be deleted yet. {}",
                    plan.host.name,
                    plan.blockers.join(" ")
                ),
            ));
        }
        models::HostVariable::filter(models::HostVariable::fields().host_id().eq(host_id))
            .delete()
            .exec(&mut tx)
            .await?;
        models::Host::filter_by_id(host_id)
            .delete()
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(Done::ok(plan))
    }

    /// The Server Index a new Host gets by default: one more than the
    /// highest `server_index` among existing Hosts, counting only positive
    /// JSON integers, or `1` when there is none. Gaps are never refilled.
    pub async fn next_server_index(&self) -> Result<u64, Error> {
        next_server_index(&mut self.db.clone()).await
    }

    /// Things about a Host the operator should notice, such as another Host
    /// sharing its Server Index.
    pub async fn host_warnings(&self, host_id: i64) -> Result<Vec<String>, Error> {
        host_warnings(&mut self.db.clone(), host_id).await
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
            Unnamed::Skip,
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
        valid_json("value", value)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        write_host_variable(&mut tx, host_id, name, value).await?;
        let warnings = host_warnings(&mut tx, host_id).await?;
        tx.commit().await?;
        Ok(Done {
            value: OwnVariable {
                name: name.to_string(),
                value: value.to_string(),
            },
            warnings,
        })
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

/// What to do with a value whose Variable row is missing (possible with
/// foreign keys off, ADR 0003).
#[derive(Clone, Copy)]
enum Unnamed {
    /// Leave it out, as `cli/si`'s joins do.
    Skip,
    /// List it under a placeholder name, e.g. for a delete preview, which
    /// must show everything the delete removes.
    Label,
}

/// Pairs each `(variable id, value)` with its Variable name, sorted by name.
async fn own_variables(
    db: &mut impl toasty::Executor,
    values: Vec<(i64, String)>,
    unnamed: Unnamed,
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
            let name = match (names.get(&id), unnamed) {
                (Some(name), _) => name.clone(),
                (None, Unnamed::Label) => format!("(Variable {id}, name missing)"),
                (None, Unnamed::Skip) => return None,
            };
            Some(OwnVariable { name, value })
        })
        .collect();
    own.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(own)
}

/// Sets a Host's value of the Variable `name`, adding the name if it is new.
/// The caller has checked the inputs and that the Host exists.
async fn write_host_variable(
    tx: &mut toasty::Transaction<'_>,
    host_id: i64,
    name: &str,
    value: &str,
) -> Result<(), Error> {
    let variable_id = match variable_by_name(&mut *tx, name).await? {
        Some(variable) => variable.id,
        None => {
            models::Variable::create()
                .name(name)
                .exec(&mut *tx)
                .await?
                .id
        }
    };
    match host_variable(variable_id, host_id)
        .first()
        .exec(&mut *tx)
        .await?
    {
        Some(mut row) => row.update().value(value).exec(&mut *tx).await?,
        None => {
            models::HostVariable::create()
                .variable_id(variable_id)
                .host_id(host_id)
                .value(value)
                .exec(&mut *tx)
                .await?;
        }
    }
    Ok(())
}

/// The `server_index` value of every existing Host, as stored. Values left
/// behind by deleted Hosts are not included.
async fn server_indexes(db: &mut impl toasty::Executor) -> Result<Vec<(Host, String)>, Error> {
    let Some(variable) = variable_by_name(&mut *db, SERVER_INDEX).await? else {
        return Ok(Vec::new());
    };
    let hosts: std::collections::HashMap<i64, Host> = models::Host::all()
        .exec(&mut *db)
        .await?
        .into_iter()
        .map(|h| (h.id, Host::from(h)))
        .collect();
    let values =
        models::HostVariable::filter(models::HostVariable::fields().variable_id().eq(variable.id))
            .exec(&mut *db)
            .await?;
    Ok(values
        .into_iter()
        .filter_map(|v| hosts.get(&v.host_id).map(|h| (h.clone(), v.value)))
        .collect())
}

async fn next_server_index(db: &mut impl toasty::Executor) -> Result<u64, Error> {
    let highest = server_indexes(db)
        .await?
        .iter()
        .filter_map(|(_, value)| parse_json(value)?.as_u64())
        .filter(|&n| n > 0)
        .max();
    // Saturating: an absurdly large stored value must not block new Hosts.
    Ok(highest.map_or(1, |n| n.saturating_add(1)))
}

async fn host_warnings(db: &mut impl toasty::Executor, host_id: i64) -> Result<Vec<String>, Error> {
    let indexes = server_indexes(db).await?;
    let Some((_, own)) = indexes.iter().find(|(h, _)| h.id == host_id) else {
        return Ok(Vec::new());
    };
    let Some(own_json) = parse_json(own) else {
        return Ok(Vec::new());
    };
    let mut others: Vec<&str> = indexes
        .iter()
        .filter(|(h, value)| h.id != host_id && parse_json(value).as_ref() == Some(&own_json))
        .map(|(h, _)| h.name.as_str())
        .collect();
    if others.is_empty() {
        return Ok(Vec::new());
    }
    others.sort();
    Ok(vec![format!(
        "Server Index {} is also used by {}.",
        own.trim(),
        others.join(", ")
    )])
}

/// The Host with this id, or [`Error::NotFound`].
async fn existing_host(
    db: &mut impl toasty::Executor,
    host_id: i64,
) -> Result<models::Host, Error> {
    models::Host::filter_by_id(host_id)
        .first()
        .exec(db)
        .await?
        .ok_or_else(|| Error::NotFound(format!("Host {host_id}")))
}

/// A Host name after [`normalize_host_name`], if it isn't empty.
fn normalized_host_name(name: &str) -> Result<String, Error> {
    let name = normalize_host_name(name);
    if name.is_empty() {
        return Err(invalid(
            "name",
            "A Host name can't be empty: it needs at least one letter, digit, \"-\" or \"_\".",
        ));
    }
    Ok(name)
}

/// Fails unless no Host other than `except` is called `name`.
async fn unused_host_name(
    db: &mut impl toasty::Executor,
    name: &str,
    except: Option<i64>,
) -> Result<(), Error> {
    // Every match, not just the first: cli/si may have left duplicate names.
    let taken = models::Host::filter(models::Host::fields().name().eq(name))
        .exec(db)
        .await?
        .iter()
        .any(|other| Some(other.id) != except);
    if taken {
        return Err(invalid(
            "name",
            format!("A Host named {name} already exists."),
        ));
    }
    Ok(())
}

/// The shared plan behind [`Inventory::host_deletion`] and
/// [`Inventory::delete_host`], so the preview and the delete can't disagree.
async fn plan_host_deletion(
    db: &mut impl toasty::Executor,
    host_id: i64,
) -> Result<HostDeletion, Error> {
    let host = existing_host(&mut *db, host_id).await?;
    let values = models::HostVariable::filter(models::HostVariable::fields().host_id().eq(host_id))
        .exec(&mut *db)
        .await?;
    let own_variables = own_variables(
        &mut *db,
        values
            .into_iter()
            .map(|v| (v.variable_id, v.value))
            .collect(),
        Unnamed::Label,
    )
    .await?;
    // TEMPORARY until lager manages Group membership (#10) and Service
    // Instances (#13): their tables aren't modelled yet, so they're counted
    // with raw SQL and block the delete instead of cascading.
    let groups = count_rows_for_host(
        &mut *db,
        "SELECT count(*) FROM host_group_membership WHERE host = ?1",
        host_id,
    )
    .await?;
    let instances = count_rows_for_host(
        &mut *db,
        "SELECT count(*) FROM service_instance WHERE host = ?1",
        host_id,
    )
    .await?;
    let mut blockers = Vec::new();
    if groups > 0 {
        blockers.push(format!(
            "It is a member of {}, and lager can't remove Hosts from Groups yet.",
            plural(groups, "Group")
        ));
    }
    if instances > 0 {
        blockers.push(format!(
            "It has {}, and lager can't delete Service Instances yet.",
            plural(instances, "Service Instance")
        ));
    }
    Ok(HostDeletion {
        host: host.into(),
        own_variables,
        blockers,
    })
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

/// The Variable called exactly `name`, if there is one.
async fn variable_by_name(
    db: &mut impl toasty::Executor,
    name: &str,
) -> Result<Option<models::Variable>, Error> {
    Ok(
        models::Variable::filter(models::Variable::fields().name().eq(name))
            .first()
            .exec(db)
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

/// The JSON document in a stored Variable value, if it is one.
fn parse_json(value: &str) -> Option<serde_json::Value> {
    serde_json::from_str(value).ok()
}

/// Checks that a Variable value, submitted as `field`, is a JSON document.
fn valid_json(field: &'static str, value: &str) -> Result<(), Error> {
    serde_json::from_str::<serde_json::Value>(value)
        .map(|_| ())
        .map_err(|e| invalid(field, format!("This is not valid JSON: {e}.")))
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
