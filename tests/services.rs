//! Seam 1: Services, their hierarchy (Parent Services) and their Variables,
//! through the Inventory module. All fixtures are synthetic.

mod common;

use common::{open, rows_referring_to, sql, temp_db};
use lager::inventory::{Error, Inventory, OwnVariable, Owner, Service, Source};

async fn service(inventory: &Inventory, name: &str) -> Service {
    inventory.create_service(name).await.unwrap().value
}

async fn nest(inventory: &Inventory, child: &Service, parent: &Service) {
    inventory
        .set_service_parent(child.id, Some(parent.id))
        .await
        .unwrap();
}

async fn set(inventory: &Inventory, owner: Owner, name: &str, value: &str) {
    inventory.set_variable(owner, name, value).await.unwrap();
}

fn own(name: &str, value: &str) -> OwnVariable {
    OwnVariable {
        name: name.to_string(),
        value: value.to_string(),
    }
}

fn names(services: &[Service]) -> Vec<String> {
    services.iter().map(|s| s.name.clone()).collect()
}

fn assert_invalid(err: Error, field: &str, says: &str) {
    assert!(
        matches!(err, Error::Invalid { field: f, ref message } if f == field && message.contains(says)),
        "expected {field} error saying {says:?}, got {err:?}"
    );
}

/// The Effective Variable called `name`, as "value from source".
async fn resolved(inventory: &Inventory, owner: Owner, name: &str) -> String {
    let v = inventory
        .effective_variables(owner)
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no Effective Variable {name}"));
    let source = match v.source {
        Source::Own => "own".to_string(),
        Source::Service { name, .. } => name,
        other => panic!("unexpected source {other:?}"),
    };
    format!("{} from {source}", v.value)
}

#[tokio::test]
async fn creates_renames_and_lists_services_by_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let nginx = service(&inventory, "nginx").await;
    service(&inventory, " mysql ").await;

    let renamed = inventory
        .rename_service(nginx.id, "haproxy")
        .await
        .unwrap()
        .value;

    assert_eq!((renamed.id, renamed.parent_id), (nginx.id, None));
    assert_eq!(inventory.service(nginx.id).await.unwrap(), Some(renamed));
    assert_eq!(
        names(&inventory.services().await.unwrap()),
        ["haproxy", "mysql"]
    );
}

#[tokio::test]
async fn service_names_must_be_non_empty_and_unique() {
    let db = temp_db();
    let inventory = open(&db).await;
    let mysql = service(&inventory, "mysql").await;
    let nginx = service(&inventory, "nginx").await;

    assert_invalid(
        inventory.create_service(" ").await.unwrap_err(),
        "name",
        "empty",
    );
    assert_invalid(
        inventory.create_service("mysql").await.unwrap_err(),
        "name",
        "mysql",
    );
    assert_invalid(
        inventory
            .rename_service(nginx.id, "mysql")
            .await
            .unwrap_err(),
        "name",
        "mysql",
    );
    assert_eq!(
        inventory
            .rename_service(mysql.id, "mysql")
            .await
            .unwrap()
            .value,
        mysql
    );
}

#[tokio::test]
async fn sets_and_clears_a_parent_service() {
    let db = temp_db();
    let inventory = open(&db).await;
    let mysql = service(&inventory, "mysql").await;
    let mariadb = service(&inventory, "mariadb").await;

    nest(&inventory, &mariadb, &mysql).await;
    assert_eq!(
        inventory
            .service(mariadb.id)
            .await
            .unwrap()
            .unwrap()
            .parent_id,
        Some(mysql.id)
    );
    assert_eq!(
        names(&inventory.child_services(mysql.id).await.unwrap()),
        ["mariadb"]
    );

    let cleared = inventory
        .set_service_parent(mariadb.id, None)
        .await
        .unwrap()
        .value;
    assert_eq!(cleared.parent_id, None);
}

#[tokio::test]
async fn rejects_a_parent_service_that_creates_a_cycle_naming_the_loop() {
    let db = temp_db();
    let inventory = open(&db).await;
    let a = service(&inventory, "a").await;
    let b = service(&inventory, "b").await;
    let c = service(&inventory, "c").await;
    nest(&inventory, &b, &a).await;
    nest(&inventory, &c, &b).await;

    let itself = inventory
        .set_service_parent(a.id, Some(a.id))
        .await
        .unwrap_err();
    let cycle = inventory
        .set_service_parent(a.id, Some(c.id))
        .await
        .unwrap_err();

    assert_invalid(itself, "parent", "a → a");
    assert_invalid(cycle, "parent", "a → c → b → a");
    assert_eq!(
        inventory.service(a.id).await.unwrap().unwrap().parent_id,
        None
    );
}

#[tokio::test]
async fn unknown_services_are_not_found() {
    let db = temp_db();
    let inventory = open(&db).await;
    let mysql = service(&inventory, "mysql").await;

    for err in [
        inventory.set_service_parent(999, None).await.unwrap_err(),
        inventory
            .set_service_parent(mysql.id, Some(999))
            .await
            .unwrap_err(),
        inventory.rename_service(999, "x").await.unwrap_err(),
        inventory
            .set_variable(Owner::Service(999), "port", "1")
            .await
            .unwrap_err(),
        inventory.service_deletion(999).await.unwrap_err(),
    ] {
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }
}

#[tokio::test]
async fn sets_and_removes_service_own_variables() {
    let db = temp_db();
    let inventory = open(&db).await;
    let mysql = service(&inventory, "mysql").await;
    let owner = Owner::Service(mysql.id);

    set(&inventory, owner, "port", "3306").await;
    set(&inventory, owner, "port", "3307").await;
    set(&inventory, owner, "engine", r#""innodb""#).await;
    inventory.remove_variable(owner, "engine").await.unwrap();

    assert_eq!(
        inventory.own_variables(owner).await.unwrap(),
        [own("port", "3307")]
    );
    let err = inventory
        .set_variable(owner, "port", "not json")
        .await
        .unwrap_err();
    assert_invalid(err, "value", "JSON");
}

#[tokio::test]
async fn a_child_service_inherits_from_its_parent_services_nearest_first() {
    let db = temp_db();
    let inventory = open(&db).await;
    let sql_db = service(&inventory, "sql").await;
    let mysql = service(&inventory, "mysql").await;
    let mariadb = service(&inventory, "mariadb").await;
    nest(&inventory, &mysql, &sql_db).await;
    nest(&inventory, &mariadb, &mysql).await;
    set(&inventory, Owner::Service(sql_db.id), "port", "1").await;
    set(&inventory, Owner::Service(sql_db.id), "backup", "true").await;
    set(&inventory, Owner::Service(mysql.id), "port", "3306").await;
    set(
        &inventory,
        Owner::Service(mariadb.id),
        "engine",
        r#""aria""#,
    )
    .await;

    let owner = Owner::Service(mariadb.id);
    assert_eq!(resolved(&inventory, owner, "port").await, "3306 from mysql");
    assert_eq!(resolved(&inventory, owner, "backup").await, "true from sql");
    assert_eq!(
        resolved(&inventory, owner, "engine").await,
        r#""aria" from own"#
    );

    set(&inventory, owner, "port", "3307").await;
    let port = inventory
        .effective_variables(owner)
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.name == "port")
        .unwrap();
    assert_eq!((port.value.as_str(), &port.source), ("3307", &Source::Own));
    assert_eq!(
        port.overrides,
        Some(Source::Service {
            id: mysql.id,
            name: "mysql".to_string()
        })
    );
}

#[tokio::test]
async fn service_inheritance_survives_a_cycle_already_in_the_data() {
    let db = temp_db();
    let inventory = open(&db).await;
    let x = service(&inventory, "x").await;
    let y = service(&inventory, "y").await;
    let top = service(&inventory, "top").await;
    set(&inventory, Owner::Service(y.id), "port", "80").await;
    // cli/si doesn't reject cycles (#2).
    sql(
        &db,
        &format!(
            "UPDATE service SET parent = {y} WHERE id = {x};
             UPDATE service SET parent = {x} WHERE id = {y};",
            x = x.id,
            y = y.id
        ),
    );

    assert_eq!(
        resolved(&inventory, Owner::Service(x.id), "port").await,
        "80 from y"
    );
    // A walk up through the existing cycle still terminates.
    assert!(
        inventory
            .set_service_parent(top.id, Some(x.id))
            .await
            .is_ok()
    );
    assert_eq!(
        resolved(&inventory, Owner::Service(top.id), "port").await,
        "80 from y"
    );
}

#[tokio::test]
async fn deleting_a_service_removes_its_variables_and_un_nests_its_children() {
    let db = temp_db();
    let inventory = open(&db).await;
    let mysql = service(&inventory, "mysql").await;
    let mariadb = service(&inventory, "mariadb").await;
    let percona = service(&inventory, "percona").await;
    nest(&inventory, &mariadb, &mysql).await;
    nest(&inventory, &percona, &mysql).await;
    set(&inventory, Owner::Service(mysql.id), "port", "3306").await;
    set(&inventory, Owner::Service(mariadb.id), "port", "3307").await;

    let preview = inventory.service_deletion(mysql.id).await.unwrap();
    let done = inventory.delete_service(mysql.id).await.unwrap();

    assert_eq!(preview.service.name, "mysql");
    assert_eq!(preview.own_variables, [own("port", "3306")]);
    assert_eq!(names(&preview.child_services), ["mariadb", "percona"]);
    assert_eq!(done.value, preview);

    assert_eq!(inventory.service(mysql.id).await.unwrap(), None);
    assert_eq!(
        rows_referring_to(&db, "service_variable", "service", mysql.id),
        0
    );
    for child in [mariadb.id, percona.id] {
        assert_eq!(
            inventory.service(child).await.unwrap().unwrap().parent_id,
            None
        );
    }
    assert_eq!(
        inventory
            .own_variables(Owner::Service(mariadb.id))
            .await
            .unwrap(),
        [own("port", "3307")]
    );
    assert!(
        inventory
            .variable_names()
            .await
            .unwrap()
            .contains(&"port".to_string())
    );
}
