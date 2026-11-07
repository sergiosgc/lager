//! Spike #5: does Toasty 0.11 work against lager's frozen schema?
//!
//! Every test runs on a fresh copy of the repo's empty `database.sqlite`.
//! Data is synthetic (documentation IP ranges).

use std::path::{Path, PathBuf};

use toasty::stmt::Value;
use toasty_schema_spike::*;

fn reference_db() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../database.sqlite")
}

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    db: toasty::Db,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("database.sqlite");
    std::fs::copy(reference_db(), &path).unwrap();
    let db = open(&path).await.unwrap();
    Fixture { _dir: dir, path, db }
}

/// The schema as SQLite records it: every table and index with its DDL.
fn schema_of(path: &Path) -> Vec<(String, String, Option<String>)> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn rows(values: Vec<Value>) -> Vec<Vec<Value>> {
    values
        .into_iter()
        .map(|v| match v {
            Value::Record(r) => r.into_iter().collect(),
            other => panic!("expected a record row, got {other:?}"),
        })
        .collect()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("expected a string, got {v:?}"))
}

#[tokio::test]
async fn foreign_key_enforcement_is_off() {
    let mut f = fixture().await;
    let r = rows(toasty::sql::query("PRAGMA foreign_keys").exec(&mut f.db).await.unwrap());
    assert_eq!(r[0][0], Value::I64(0));
}

#[tokio::test]
async fn reads_db_version_through_a_model() {
    let mut f = fixture().await;
    let v = DbVersion::get_by_key(&mut f.db, "single-row").await.unwrap();
    assert_eq!(v.version, Some(1));
}

#[tokio::test]
async fn sqlite_assigns_rowid_keys_on_insert() {
    let mut f = fixture().await;
    let a = Host::create().name("web1").exec(&mut f.db).await.unwrap();
    let b = Host::create().name("web2").exec(&mut f.db).await.unwrap();
    assert_eq!((a.id, b.id), (1, 2));

    let back = Host::get_by_id(&mut f.db, &b.id).await.unwrap();
    assert_eq!(back.name, "web2");
}

/// Writes one row into every table through Toasty, reads it back through
/// Toasty, and checks the bytes landed in the right columns with plain SQLite.
#[tokio::test]
async fn maps_every_table_including_renamed_relation_keys() {
    let mut f = fixture().await;
    let db = &mut f.db;

    let host = Host::create().name("web1").exec(db).await.unwrap();
    let all = Group::create().name("all").exec(db).await.unwrap();
    let eu = Group::create().name("eu").parent(&all).exec(db).await.unwrap();
    Membership::create().host_id(host.id).group_id(eu.id).exec(db).await.unwrap();

    let ntp = Variable::create().name("ntp_server").exec(db).await.unwrap();
    let port = Variable::create().name("port").exec(db).await.unwrap();
    HostVariable::create()
        .variable_id(ntp.id)
        .host_id(host.id)
        .value("\"ntp.host.example\"")
        .exec(db)
        .await
        .unwrap();
    GroupVariable::create()
        .variable_id(ntp.id)
        .group_id(eu.id)
        .value("\"ntp.eu.example\"")
        .exec(db)
        .await
        .unwrap();

    let mysql = Service::create().name("mysql").exec(db).await.unwrap();
    let mariadb = Service::create().name("mariadb").parent(&mysql).exec(db).await.unwrap();
    ServiceVariable::create()
        .variable_id(port.id)
        .service_id(mysql.id)
        .value("3306")
        .exec(db)
        .await
        .unwrap();

    let si = ServiceInstance::create()
        .service(&mariadb)
        .host(&host)
        .name("main")
        .ip("192.0.2.10")
        .exec(db)
        .await
        .unwrap();
    ServiceInstanceVariable::create()
        .variable_id(port.id)
        .instance_id(si.id)
        .value("3307")
        .exec(db)
        .await
        .unwrap();

    // Read back through Toasty: composite-key lookups and relation traversal.
    let m = Membership::get_by_host_id_and_group_id(db, &host.id, &eu.id).await.unwrap();
    assert_eq!(m.group().exec(db).await.unwrap().name, "eu");
    let hv = HostVariable::get_by_variable_id_and_host_id(db, &ntp.id, &host.id).await.unwrap();
    assert_eq!(hv.variable().exec(db).await.unwrap().name, "ntp_server");
    let gv = GroupVariable::get_by_variable_id_and_group_id(db, &ntp.id, &eu.id).await.unwrap();
    assert_eq!(gv.value, "\"ntp.eu.example\"");
    let children = all.children().exec(db).await.unwrap();
    assert_eq!(children.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["eu"]);
    assert_eq!(eu.parent().exec(db).await.unwrap().unwrap().name, "all");
    assert!(all.parent().exec(db).await.unwrap().is_none());
    assert_eq!(mariadb.parent().exec(db).await.unwrap().unwrap().name, "mysql");
    let instances = host.service_instances().exec(db).await.unwrap();
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0].service().exec(db).await.unwrap().name, "mariadb");
    let siv = ServiceInstanceVariable::get_by_variable_id_and_instance_id(db, &port.id, &si.id)
        .await
        .unwrap();
    assert_eq!(siv.value, "3307");
    let sv = ServiceVariable::get_by_variable_id_and_service_id(db, &port.id, &mysql.id)
        .await
        .unwrap();
    assert_eq!(sv.value, "3306");

    // Check the real columns with plain SQLite, bypassing Toasty.
    let conn = rusqlite::Connection::open(&f.path).unwrap();
    let q = |sql: &str| -> String { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    let eu_id = eu.id;
    assert_eq!(
        q(r#"SELECT h.name || '/' || g.name FROM host_group_membership m
             JOIN host h ON h.id = m.host JOIN host_group g ON g.id = m."group""#),
        "web1/eu"
    );
    assert_eq!(
        q(r#"SELECT c.name || '<' || p.name FROM host_group c JOIN host_group p ON p.id = c.parent"#),
        "eu<all"
    );
    assert_eq!(
        q(r#"SELECT s.name || '@' || h.name || ':' || i.name || '=' || i.ip FROM service_instance i
             JOIN service s ON s.id = i."service" JOIN host h ON h.id = i.host"#),
        "mariadb@web1:main=192.0.2.10"
    );
    assert_eq!(
        conn.query_row(
            r#"SELECT v.name || '=' || gv.value FROM host_group_variable gv
               JOIN variable v ON v.id = gv.variable WHERE gv."group" = ?1"#,
            [eu_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
        "ntp_server=\"ntp.eu.example\""
    );
    drop(conn);
    assert_eq!(schema_of(&f.path), schema_of(&reference_db()));
}

/// The Effective Variables of a Host through its Groups and Parent Groups,
/// nearest first, via raw SQL with a recursive CTE.
#[tokio::test]
async fn resolves_host_effective_variables_with_a_recursive_query() {
    let mut f = fixture().await;
    let db = &mut f.db;

    let host = Host::create().name("web1").exec(db).await.unwrap();
    let all = Group::create().name("all").exec(db).await.unwrap();
    let eu = Group::create().name("eu").parent(&all).exec(db).await.unwrap();
    Membership::create().host_id(host.id).group_id(eu.id).exec(db).await.unwrap();

    let mut var = async |name: &str| Variable::create().name(name).exec(db).await.unwrap().id;
    let (ntp, dns, tz) = (var("ntp_server").await, var("dns").await, var("timezone").await);

    let set_group = |v, g, value: &str| {
        GroupVariable::create().variable_id(v).group_id(g).value(value.to_string())
    };
    set_group(ntp, all.id, "\"ntp.all.example\"").exec(db).await.unwrap();
    set_group(dns, all.id, "\"192.0.2.53\"").exec(db).await.unwrap();
    set_group(ntp, eu.id, "\"ntp.eu.example\"").exec(db).await.unwrap();
    HostVariable::create()
        .variable_id(tz)
        .host_id(host.id)
        .value("\"UTC\"")
        .exec(db)
        .await
        .unwrap();

    let effective = rows(
        toasty::sql::query(
            r#"
WITH RECURSIVE
  ancestry(id, name, parent, depth) AS (
    SELECT g.id, g.name, g.parent, 1
      FROM host_group g JOIN host_group_membership m ON m."group" = g.id
     WHERE m.host = ?1
    UNION ALL
    SELECT g.id, g.name, g.parent, a.depth + 1
      FROM host_group g JOIN ancestry a ON g.id = a.parent
  ),
  candidate(variable, value, source, depth) AS (
    SELECT hv.variable, hv.value, 'host', 0 FROM host_variable hv WHERE hv.host = ?1
    UNION ALL
    SELECT gv.variable, gv.value, 'group ' || a.name, a.depth
      FROM host_group_variable gv JOIN ancestry a ON gv."group" = a.id
  ),
  ranked AS (
    SELECT c.*, row_number() OVER (PARTITION BY c.variable ORDER BY c.depth) AS rn
      FROM candidate c
  )
SELECT v.name, r.value, r.source FROM ranked r JOIN variable v ON v.id = r.variable
 WHERE r.rn = 1 ORDER BY v.name
"#,
        )
        .bind(host.id)
        .exec(db)
        .await
        .unwrap(),
    );

    let got: Vec<[&str; 3]> = effective.iter().map(|r| [s(&r[0]), s(&r[1]), s(&r[2])]).collect();
    assert_eq!(
        got,
        [
            ["dns", "\"192.0.2.53\"", "group all"],
            ["ntp_server", "\"ntp.eu.example\"", "group eu"],
            ["timezone", "\"UTC\"", "host"],
        ]
    );
}

#[tokio::test]
async fn never_changes_the_schema() {
    let before = schema_of(&reference_db());
    let f = fixture().await;
    let mut db = f.db;
    Host::create().name("web1").exec(&mut db).await.unwrap();
    drop(db);
    assert_eq!(schema_of(&f.path), before);
}

/// The pragma is per-connection: make sure a failed statement or a
/// transaction doesn't leave us on a fresh connection with enforcement on.
#[tokio::test]
async fn foreign_keys_stay_off_after_errors_and_transactions() {
    let mut f = fixture().await;
    assert!(toasty::sql::statement("NOT VALID SQL").exec(&mut f.db).await.is_err());
    {
        let mut tx = f.db.transaction().await.unwrap();
        let all = Group::create().name("all").exec(&mut tx).await.unwrap();
        Group::create().name("eu").parent(&all).exec(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    let r = rows(toasty::sql::query("PRAGMA foreign_keys").exec(&mut f.db).await.unwrap());
    assert_eq!(r[0][0], Value::I64(0));
    assert_eq!(Group::all().exec(&mut f.db).await.unwrap().len(), 2);
    assert_eq!(schema_of(&f.path), schema_of(&reference_db()));
}

/// Finding: the schema allows NULL names (ADR 0003), but a name mapped as
/// `String` makes any query that loads such a row fail, not just that row.
#[tokio::test]
async fn reading_a_null_name() {
    let mut f = fixture().await;
    rusqlite::Connection::open(&f.path)
        .unwrap()
        .execute("INSERT INTO host(name) VALUES (NULL)", [])
        .unwrap();
    let err = Host::all().exec(&mut f.db).await.unwrap_err();
    assert!(err.to_string().contains("cannot convert Null to String"), "{err}");
}
