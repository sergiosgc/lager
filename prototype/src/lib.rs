//! Throwaway spike for #5: Toasty models mapped onto lager's frozen schema
//! (ADR 0002, ADR 0003). Nothing here creates or alters tables.

use toasty::Deferred;

#[derive(Debug, toasty::Model)]
#[table = "host"]
pub struct Host {
    #[key]
    #[auto(increment)]
    pub id: i64,

    pub name: String,

    #[has_many(pair = host)]
    pub service_instances: Deferred<Vec<ServiceInstance>>,
}

#[derive(Debug, toasty::Model)]
#[table = "host_group"]
pub struct Group {
    #[key]
    #[auto(increment)]
    pub id: i64,

    #[index]
    #[column("parent")]
    pub parent_id: Option<i64>,

    #[belongs_to(key = parent_id, references = id)]
    pub parent: Deferred<Option<Group>>,

    #[has_many(pair = parent)]
    pub children: Deferred<Vec<Group>>,

    pub name: String,
}

#[derive(Debug, toasty::Model)]
#[table = "host_group_membership"]
#[key(host_id, group_id)]
pub struct Membership {
    #[column("host")]
    pub host_id: i64,

    #[column("group")]
    pub group_id: i64,

    #[belongs_to(key = host_id, references = id)]
    pub host: Deferred<Host>,

    #[belongs_to(key = group_id, references = id)]
    pub group: Deferred<Group>,
}

#[derive(Debug, toasty::Model)]
#[table = "variable"]
pub struct Variable {
    #[key]
    #[auto(increment)]
    pub id: i64,

    pub name: String,
}

#[derive(Debug, toasty::Model)]
#[table = "host_variable"]
#[key(variable_id, host_id)]
pub struct HostVariable {
    #[column("variable")]
    pub variable_id: i64,

    #[column("host")]
    pub host_id: i64,

    #[belongs_to(key = variable_id, references = id)]
    pub variable: Deferred<Variable>,

    pub value: String,
}

#[derive(Debug, toasty::Model)]
#[table = "host_group_variable"]
#[key(variable_id, group_id)]
pub struct GroupVariable {
    #[column("variable")]
    pub variable_id: i64,

    #[column("group")]
    pub group_id: i64,

    #[belongs_to(key = group_id, references = id)]
    pub group: Deferred<Group>,

    pub value: String,
}

#[derive(Debug, toasty::Model)]
#[table = "service"]
pub struct Service {
    #[key]
    #[auto(increment)]
    pub id: i64,

    pub name: String,

    #[index]
    #[column("parent")]
    pub parent_id: Option<i64>,

    #[belongs_to(key = parent_id, references = id)]
    pub parent: Deferred<Option<Service>>,

    #[has_many(pair = parent)]
    pub children: Deferred<Vec<Service>>,
}

#[derive(Debug, toasty::Model)]
#[table = "service_variable"]
#[key(variable_id, service_id)]
pub struct ServiceVariable {
    #[column("variable")]
    pub variable_id: i64,

    #[column("service")]
    pub service_id: i64,

    pub value: String,
}

#[derive(Debug, toasty::Model)]
#[table = "service_instance"]
pub struct ServiceInstance {
    #[key]
    #[auto(increment)]
    pub id: i64,

    #[index]
    #[column("service")]
    pub service_id: i64,

    #[belongs_to(key = service_id, references = id)]
    pub service: Deferred<Service>,

    #[index]
    #[column("host")]
    pub host_id: i64,

    #[belongs_to(key = host_id, references = id)]
    pub host: Deferred<Host>,

    pub ip: String,

    pub name: String,
}

#[derive(Debug, toasty::Model)]
#[table = "service_instance_variable"]
#[key(variable_id, instance_id)]
pub struct ServiceInstanceVariable {
    #[column("variable")]
    pub variable_id: i64,

    #[column("instance")]
    pub instance_id: i64,

    pub value: String,
}

#[derive(Debug, toasty::Model)]
#[table = "db_version"]
pub struct DbVersion {
    #[key]
    pub key: String,

    pub version: Option<i64>,
}

/// Opens an existing database file with foreign-key enforcement off (ADR 0002).
///
/// toasty-driver-sqlite 0.11.0 has no pragma API, and the SQLite that
/// rusqlite bundles is compiled with `SQLITE_DEFAULT_FOREIGN_KEYS=1`, so every
/// new connection enforces foreign keys. `PRAGMA foreign_keys` is
/// per-connection, so the pool is pinned to one connection that is never
/// recycled (lifetime and idle caps default to none), and the pragma is set
/// once on it.
///
/// TEMPORARY: replace with the driver's `.foreign_keys(false)` once a
/// published Toasty release has it. See
/// https://github.com/sergiosgc/lager/issues/17
pub async fn open(path: &std::path::Path) -> toasty::Result<toasty::Db> {
    let driver = toasty_driver_sqlite::Sqlite::open(path);
    let mut db = toasty::Db::builder()
        .models(toasty::models!(crate::*))
        .max_pool_size(1)
        .build(driver)
        .await?;
    toasty::sql::statement("PRAGMA foreign_keys = OFF")
        .exec(&mut db)
        .await?;
    Ok(db)
}
