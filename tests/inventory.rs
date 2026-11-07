//! Seam 1: the Inventory module, against a fresh copy of the empty database.

mod common;

use common::{open, temp_db};
use lager::inventory::{Error, Inventory, OpenError, OwnVariable};

#[tokio::test]
async fn refuses_a_missing_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.sqlite");

    let err = Inventory::open(&missing).await.err().unwrap();

    assert!(
        matches!(err, OpenError::Missing(ref p) if p == &missing),
        "{err:?}"
    );
    assert!(err.to_string().contains("does not exist"), "{err}");
    assert!(!missing.exists(), "opening must not create the file");
}

#[tokio::test]
async fn opens_database_sqlite_inside_a_given_directory() {
    let db = temp_db();
    open(&db).await.create_host("web1").await.unwrap();

    let inventory = Inventory::open(db.dir.path()).await.unwrap();

    assert_eq!(inventory.hosts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn refuses_a_directory_without_database_sqlite() {
    let dir = tempfile::tempdir().unwrap();

    let err = Inventory::open(dir.path()).await.err().unwrap();

    let expected = dir.path().join("database.sqlite");
    assert!(
        matches!(err, OpenError::Missing(ref p) if p == &expected),
        "{err:?}"
    );
    assert!(!expected.exists(), "opening must not create the file");
}

#[tokio::test]
async fn refuses_a_database_with_another_db_version() {
    let db = temp_db();
    rusqlite::Connection::open(&db.path)
        .unwrap()
        .execute("UPDATE db_version SET version = 2", [])
        .unwrap();

    let err = Inventory::open(&db.path).await.err().unwrap();

    assert!(
        matches!(err, OpenError::UnsupportedVersion { found: Some(2) }),
        "{err:?}"
    );
    assert!(err.to_string().contains("db_version"), "{err}");
}

#[tokio::test]
async fn opens_the_empty_reference_database() {
    let db = temp_db();
    let inventory = open(&db).await;
    assert!(inventory.hosts().await.unwrap().is_empty());
}

#[tokio::test]
async fn creates_a_host_and_gets_it_back() {
    let db = temp_db();
    let inventory = open(&db).await;

    let done = inventory.create_host("web1").await.unwrap();
    assert!(done.warnings.is_empty());
    let created = done.value;
    assert_eq!(created.name, "web1");

    assert_eq!(
        inventory.host(created.id).await.unwrap(),
        Some(created.clone())
    );
    assert_eq!(inventory.hosts().await.unwrap(), [created]);
}

#[tokio::test]
async fn getting_an_unknown_host_finds_nothing() {
    let db = temp_db();
    let inventory = open(&db).await;
    assert_eq!(inventory.host(42).await.unwrap(), None);
}

#[tokio::test]
async fn lists_hosts_sorted_by_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    for name in ["web2", "db1", "web10", "app1"] {
        inventory.create_host(name).await.unwrap();
    }

    let names: Vec<String> = inventory
        .hosts()
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.name)
        .collect();

    assert_eq!(names, ["app1", "db1", "web10", "web2"]);
}

#[tokio::test]
async fn rejects_an_empty_host_name() {
    let db = temp_db();
    let inventory = open(&db).await;

    for name in ["", "   ", "!!!", "ÉÉ."] {
        let err = inventory.create_host(name).await.unwrap_err();
        assert!(
            matches!(err, Error::Invalid { field: "name", ref message } if message.contains("empty")),
            "{name:?}: {err:?}"
        );
    }
    assert!(inventory.hosts().await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_a_duplicate_host_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory.create_host("web1").await.unwrap();

    let err = inventory.create_host("web1").await.unwrap_err();

    assert!(
        matches!(err, Error::Invalid { field: "name", ref message } if message.contains("web1")),
        "{err:?}"
    );
    assert_eq!(inventory.hosts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn normalizes_host_names_to_lowercase_letters_digits_dashes_and_underscores() {
    let db = temp_db();
    let inventory = open(&db).await;

    for (given, stored) in [
        (" DB1 ", "db1"),
        ("Web-01.Example_X", "web-01example_x"),
        ("mail server #2", "mailserver2"),
        ("Ça-va", "a-va"),
    ] {
        let host = inventory.create_host(given).await.unwrap().value;
        assert_eq!(host.name, stored, "{given:?}");
    }
}

#[tokio::test]
async fn rejects_a_name_that_normalizes_to_an_existing_host() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory.create_host("web1").await.unwrap();

    for name in ["WEB1", " web1 ", "web.1"] {
        let err = inventory.create_host(name).await.unwrap_err();
        assert!(
            matches!(err, Error::Invalid { field: "name", ref message } if message.contains("web1")),
            "{name:?}: {err:?}"
        );
    }
    assert_eq!(inventory.hosts().await.unwrap().len(), 1);
}

// Host Own Variables

fn own(name: &str, value: &str) -> OwnVariable {
    OwnVariable {
        name: name.to_string(),
        value: value.to_string(),
    }
}

#[tokio::test]
async fn sets_host_own_variables_listed_by_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;

    inventory
        .set_host_variable(web1.id, "ntp_server", r#""ntp.example.org""#)
        .await
        .unwrap();
    inventory
        .set_host_variable(web1.id, "dns", r#"["192.0.2.53", "192.0.2.54"]"#)
        .await
        .unwrap();

    assert_eq!(
        inventory.host_variables(web1.id).await.unwrap(),
        [
            own("dns", r#"["192.0.2.53", "192.0.2.54"]"#),
            own("ntp_server", r#""ntp.example.org""#),
        ]
    );
}

#[tokio::test]
async fn stores_the_json_text_exactly_as_submitted() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;
    let value = "{ \"b\": 1,\n  \"a\": [1.50, true] }";

    inventory
        .set_host_variable(web1.id, "cfg", value)
        .await
        .unwrap();

    assert_eq!(
        inventory.host_variables(web1.id).await.unwrap(),
        [own("cfg", value)]
    );
}

#[tokio::test]
async fn overwrites_an_own_variable() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();

    inventory
        .set_host_variable(web1.id, "port", "8080")
        .await
        .unwrap();

    assert_eq!(
        inventory.host_variables(web1.id).await.unwrap(),
        [own("port", "8080")]
    );
}

#[tokio::test]
async fn own_variables_belong_to_their_host() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;
    let web2 = inventory.create_host("web2").await.unwrap().value;

    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();
    inventory
        .set_host_variable(web2.id, "port", "81")
        .await
        .unwrap();

    assert_eq!(
        inventory.host_variables(web1.id).await.unwrap(),
        [own("port", "80")]
    );
    assert_eq!(
        inventory.host_variables(web2.id).await.unwrap(),
        [own("port", "81")]
    );
}

#[tokio::test]
async fn removing_an_own_variable_leaves_it_on_other_hosts() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;
    let web2 = inventory.create_host("web2").await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();
    inventory
        .set_host_variable(web2.id, "port", "81")
        .await
        .unwrap();

    inventory
        .remove_host_variable(web1.id, "port")
        .await
        .unwrap();

    assert!(inventory.host_variables(web1.id).await.unwrap().is_empty());
    assert_eq!(
        inventory.host_variables(web2.id).await.unwrap(),
        [own("port", "81")]
    );
}

#[tokio::test]
async fn removing_the_last_value_keeps_the_variable_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();

    inventory
        .remove_host_variable(web1.id, "port")
        .await
        .unwrap();

    assert_eq!(inventory.variable_names().await.unwrap(), ["port"]);
}

#[tokio::test]
async fn setting_a_variable_reuses_its_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;
    let web2 = inventory.create_host("web2").await.unwrap().value;

    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();
    inventory
        .set_host_variable(web2.id, "port", "81")
        .await
        .unwrap();
    inventory
        .remove_host_variable(web1.id, "port")
        .await
        .unwrap();
    inventory
        .set_host_variable(web1.id, "port", "82")
        .await
        .unwrap();

    assert_eq!(inventory.variable_names().await.unwrap(), ["port"]);
}

#[tokio::test]
async fn rejects_a_value_that_is_not_json() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();

    for value in ["", "ntp.example.org", "{\"a\": 1", "[1, 2,]"] {
        let err = inventory
            .set_host_variable(web1.id, "port", value)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::Invalid { field: "value", ref message } if message.contains("JSON")),
            "{value:?}: {err:?}"
        );
    }
    assert_eq!(
        inventory.host_variables(web1.id).await.unwrap(),
        [own("port", "80")]
    );
}

#[tokio::test]
async fn rejects_an_empty_variable_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;

    for name in ["", "  "] {
        let err = inventory
            .set_host_variable(web1.id, name, "1")
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::Invalid { field: "name", ref message } if message.contains("empty")),
            "{name:?}: {err:?}"
        );
    }
    assert!(inventory.host_variables(web1.id).await.unwrap().is_empty());
    assert!(inventory.variable_names().await.unwrap().is_empty());
}

#[tokio::test]
async fn keeps_variable_names_exactly_as_given() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1").await.unwrap().value;
    // As cli/si may have written it: a name with surrounding whitespace.
    inventory
        .set_host_variable(web1.id, " port", "80")
        .await
        .unwrap();
    inventory
        .set_host_variable(web1.id, "port", "81")
        .await
        .unwrap();

    inventory
        .set_host_variable(web1.id, " port", "8080")
        .await
        .unwrap();
    assert_eq!(
        inventory.host_variables(web1.id).await.unwrap(),
        [own(" port", "8080"), own("port", "81")]
    );

    inventory
        .remove_host_variable(web1.id, " port")
        .await
        .unwrap();
    assert_eq!(
        inventory.host_variables(web1.id).await.unwrap(),
        [own("port", "81")]
    );
}

#[tokio::test]
async fn variables_of_an_unknown_host_are_not_found() {
    let db = temp_db();
    let inventory = open(&db).await;

    let err = inventory
        .set_host_variable(42, "port", "80")
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    assert!(inventory.variable_names().await.unwrap().is_empty());
}

#[tokio::test]
async fn removing_a_variable_of_an_unknown_host_is_not_found() {
    let db = temp_db();
    let inventory = open(&db).await;

    let err = inventory
        .remove_host_variable(42, "port")
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
}
