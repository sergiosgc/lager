//! Seam 1: the Inventory module, against a fresh copy of the empty database.

mod common;

use common::{open, temp_db};
use lager::inventory::{Error, Inventory, OpenError, OwnVariable, SERVER_INDEX};

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
    open(&db).await.create_host("web1", None).await.unwrap();

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

    let done = inventory.create_host("web1", None).await.unwrap();
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
        inventory.create_host(name, None).await.unwrap();
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
        let err = inventory.create_host(name, None).await.unwrap_err();
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
    inventory.create_host("web1", None).await.unwrap();

    let err = inventory.create_host("web1", None).await.unwrap_err();

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
        let host = inventory.create_host(given, None).await.unwrap().value;
        assert_eq!(host.name, stored, "{given:?}");
    }
}

#[tokio::test]
async fn rejects_a_name_that_normalizes_to_an_existing_host() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory.create_host("web1", None).await.unwrap();

    for name in ["WEB1", " web1 ", "web.1"] {
        let err = inventory.create_host(name, None).await.unwrap_err();
        assert!(
            matches!(err, Error::Invalid { field: "name", ref message } if message.contains("web1")),
            "{name:?}: {err:?}"
        );
    }
    assert_eq!(inventory.hosts().await.unwrap().len(), 1);
}

// Host Own Variables

/// A Host's Own Variables, leaving out the `server_index` every new Host
/// gets, which the Server Index tests cover.
async fn variables_besides_server_index(inventory: &Inventory, host_id: i64) -> Vec<OwnVariable> {
    let mut variables = inventory.host_variables(host_id).await.unwrap();
    variables.retain(|v| v.name != SERVER_INDEX);
    variables
}

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
    let web1 = inventory.create_host("web1", None).await.unwrap().value;

    inventory
        .set_host_variable(web1.id, "ntp_server", r#""ntp.example.org""#)
        .await
        .unwrap();
    inventory
        .set_host_variable(web1.id, "dns", r#"["192.0.2.53", "192.0.2.54"]"#)
        .await
        .unwrap();

    assert_eq!(
        variables_besides_server_index(&inventory, web1.id).await,
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
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let value = "{ \"b\": 1,\n  \"a\": [1.50, true] }";

    inventory
        .set_host_variable(web1.id, "cfg", value)
        .await
        .unwrap();

    assert_eq!(
        variables_besides_server_index(&inventory, web1.id).await,
        [own("cfg", value)]
    );
}

#[tokio::test]
async fn overwrites_an_own_variable() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();

    inventory
        .set_host_variable(web1.id, "port", "8080")
        .await
        .unwrap();

    assert_eq!(
        variables_besides_server_index(&inventory, web1.id).await,
        [own("port", "8080")]
    );
}

#[tokio::test]
async fn own_variables_belong_to_their_host() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web2 = inventory.create_host("web2", None).await.unwrap().value;

    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();
    inventory
        .set_host_variable(web2.id, "port", "81")
        .await
        .unwrap();

    assert_eq!(
        variables_besides_server_index(&inventory, web1.id).await,
        [own("port", "80")]
    );
    assert_eq!(
        variables_besides_server_index(&inventory, web2.id).await,
        [own("port", "81")]
    );
}

#[tokio::test]
async fn removing_an_own_variable_leaves_it_on_other_hosts() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web2 = inventory.create_host("web2", None).await.unwrap().value;
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

    assert!(
        variables_besides_server_index(&inventory, web1.id)
            .await
            .is_empty()
    );
    assert_eq!(
        variables_besides_server_index(&inventory, web2.id).await,
        [own("port", "81")]
    );
}

#[tokio::test]
async fn removing_the_last_value_keeps_the_variable_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();

    inventory
        .remove_host_variable(web1.id, "port")
        .await
        .unwrap();

    assert_eq!(
        inventory.variable_names().await.unwrap(),
        ["port", "server_index"]
    );
}

#[tokio::test]
async fn setting_a_variable_reuses_its_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web2 = inventory.create_host("web2", None).await.unwrap().value;

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

    assert_eq!(
        inventory.variable_names().await.unwrap(),
        ["port", "server_index"]
    );
}

#[tokio::test]
async fn rejects_a_value_that_is_not_json() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
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
        variables_besides_server_index(&inventory, web1.id).await,
        [own("port", "80")]
    );
}

#[tokio::test]
async fn rejects_an_empty_variable_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;

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
    assert!(
        variables_besides_server_index(&inventory, web1.id)
            .await
            .is_empty()
    );
    assert_eq!(inventory.variable_names().await.unwrap(), ["server_index"]);
}

#[tokio::test]
async fn keeps_variable_names_exactly_as_given() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
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
        variables_besides_server_index(&inventory, web1.id).await,
        [own(" port", "8080"), own("port", "81")]
    );

    inventory
        .remove_host_variable(web1.id, " port")
        .await
        .unwrap();
    assert_eq!(
        variables_besides_server_index(&inventory, web1.id).await,
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

// Server Index

/// The Host's `server_index` Own Variable, as stored.
async fn server_index(inventory: &Inventory, host_id: i64) -> Option<String> {
    inventory
        .host_variables(host_id)
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.name == SERVER_INDEX)
        .map(|v| v.value)
}

/// Arranges data lager itself wouldn't write, the way cli/si or a deleted
/// Host might have left it.
fn sql(db: &common::TempDb, statement: &str) {
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn.execute_batch(statement).unwrap();
}

#[tokio::test]
async fn the_first_host_gets_server_index_1_and_the_next_2() {
    let db = temp_db();
    let inventory = open(&db).await;
    assert_eq!(inventory.next_server_index().await.unwrap(), 1);

    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web2 = inventory.create_host("web2", None).await.unwrap().value;

    assert_eq!(
        server_index(&inventory, web1.id).await.as_deref(),
        Some("1")
    );
    assert_eq!(
        server_index(&inventory, web2.id).await.as_deref(),
        Some("2")
    );
    assert_eq!(inventory.next_server_index().await.unwrap(), 3);
}

#[tokio::test]
async fn ignores_server_indexes_that_are_not_positive_json_integers() {
    let db = temp_db();
    let inventory = open(&db).await;
    let mut hosts = Vec::new();
    for name in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        hosts.push(inventory.create_host(name, None).await.unwrap().value);
    }
    for (host, value) in hosts
        .iter()
        .zip(["7", "0", "-40", "12.5", "\"99\"", "null", "[100]", "1e3"])
    {
        inventory
            .set_host_variable(host.id, SERVER_INDEX, value)
            .await
            .unwrap();
    }

    assert_eq!(inventory.next_server_index().await.unwrap(), 8);
}

#[tokio::test]
async fn saves_an_override_as_given() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory.create_host("web1", None).await.unwrap();

    let web2 = inventory.create_host("web2", Some("40")).await.unwrap();
    let web3 = inventory
        .create_host("web3", Some(r#""spare""#))
        .await
        .unwrap();

    assert!(web2.warnings.is_empty(), "{:?}", web2.warnings);
    assert_eq!(
        server_index(&inventory, web2.value.id).await.as_deref(),
        Some("40")
    );
    assert_eq!(
        server_index(&inventory, web3.value.id).await.as_deref(),
        Some(r#""spare""#)
    );
    assert_eq!(inventory.next_server_index().await.unwrap(), 41);
}

#[tokio::test]
async fn rejects_an_override_that_is_not_json() {
    let db = temp_db();
    let inventory = open(&db).await;

    let err = inventory
        .create_host("web1", Some("abc"))
        .await
        .unwrap_err();

    assert!(
        matches!(err, Error::Invalid { field: "server_index", ref message } if message.contains("JSON")),
        "{err:?}"
    );
    assert!(
        inventory.hosts().await.unwrap().is_empty(),
        "no half-created Host"
    );
}

#[tokio::test]
async fn a_duplicate_override_warns_naming_the_other_host_and_still_creates() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;

    let done = inventory.create_host("web2", Some("1")).await.unwrap();

    assert_eq!(
        server_index(&inventory, done.value.id).await.as_deref(),
        Some("1")
    );
    assert_eq!(done.warnings, ["Server Index 1 is also used by web1."]);
    assert_eq!(
        inventory.host_warnings(web1.id).await.unwrap(),
        ["Server Index 1 is also used by web2."]
    );
}

#[tokio::test]
async fn names_every_other_host_sharing_the_server_index() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory.create_host("web2", Some("5")).await.unwrap();
    inventory.create_host("web1", Some("5")).await.unwrap();

    let done = inventory.create_host("web3", Some("5")).await.unwrap();

    assert_eq!(
        done.warnings,
        ["Server Index 5 is also used by web1, web2."]
    );
}

#[tokio::test]
async fn never_refills_a_gap() {
    let db = temp_db();
    let inventory = open(&db).await;
    for name in ["web1", "web2", "web3"] {
        inventory.create_host(name, None).await.unwrap();
    }
    // Deleting a Host (as cli/si does) leaves a gap at 2.
    sql(
        &db,
        "DELETE FROM host_variable WHERE host = (SELECT id FROM host WHERE name = 'web2');
              DELETE FROM host WHERE name = 'web2';",
    );

    let web4 = inventory.create_host("web4", None).await.unwrap().value;

    assert_eq!(
        server_index(&inventory, web4.id).await.as_deref(),
        Some("4")
    );
}

#[tokio::test]
async fn the_highest_index_comes_back_only_while_it_is_one_past_the_max() {
    let db = temp_db();
    let inventory = open(&db).await;
    for name in ["web1", "web2", "web3"] {
        inventory.create_host(name, None).await.unwrap();
    }
    sql(
        &db,
        "DELETE FROM host_variable WHERE host = (SELECT id FROM host WHERE name = 'web3');
              DELETE FROM host WHERE name = 'web3';",
    );

    assert_eq!(inventory.next_server_index().await.unwrap(), 3);
}

#[tokio::test]
async fn ignores_server_indexes_of_deleted_hosts_left_behind() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory.create_host("web1", None).await.unwrap();
    inventory.create_host("web2", None).await.unwrap();
    // cli/si's host delete leaves the Host's Variables behind (ADR 0003).
    sql(&db, "DELETE FROM host WHERE name = 'web2';");

    assert_eq!(inventory.next_server_index().await.unwrap(), 2);
    let web3 = inventory.create_host("web3", Some("2")).await.unwrap();
    assert!(web3.warnings.is_empty(), "{:?}", web3.warnings);
}

#[tokio::test]
async fn a_blank_override_assigns_the_next_server_index() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory.create_host("web1", None).await.unwrap();

    let web2 = inventory
        .create_host("web2", Some("  "))
        .await
        .unwrap()
        .value;

    assert_eq!(
        server_index(&inventory, web2.id).await.as_deref(),
        Some("2")
    );
}

#[tokio::test]
async fn the_largest_possible_server_index_does_not_block_new_hosts() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory
        .create_host("web1", Some("18446744073709551615"))
        .await
        .unwrap();

    let web2 = inventory.create_host("web2", None).await;

    assert!(web2.is_ok(), "{web2:?}");
}

#[tokio::test]
async fn setting_a_duplicate_server_index_warns_naming_the_other_host() {
    let db = temp_db();
    let inventory = open(&db).await;
    inventory.create_host("web1", None).await.unwrap();
    let web2 = inventory.create_host("web2", None).await.unwrap().value;

    let done = inventory
        .set_host_variable(web2.id, SERVER_INDEX, "1")
        .await
        .unwrap();

    assert_eq!(done.warnings, ["Server Index 1 is also used by web1."]);
}

// Renaming and deleting Hosts

/// How many rows in `table` refer to the Host through `column`.
fn rows_referring_to(db: &common::TempDb, table: &str, column: &str, host_id: i64) -> i64 {
    rusqlite::Connection::open(&db.path)
        .unwrap()
        .query_row(
            &format!("SELECT count(*) FROM {table} WHERE {column} = ?1"),
            [host_id],
            |r| r.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn renaming_a_host_keeps_its_identity_and_own_variables() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();
    let before = inventory.host_variables(web1.id).await.unwrap();

    let renamed = inventory
        .rename_host(web1.id, "Web-01")
        .await
        .unwrap()
        .value;

    assert_eq!(renamed.id, web1.id);
    assert_eq!(renamed.name, "web-01");
    assert_eq!(inventory.host(web1.id).await.unwrap(), Some(renamed));
    assert_eq!(inventory.host_variables(web1.id).await.unwrap(), before);
}

#[tokio::test]
async fn renaming_a_host_follows_the_name_rules() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory.create_host("web2", None).await.unwrap();

    for (name, says) in [("", "empty"), ("!!!", "empty"), ("WEB2", "web2")] {
        let err = inventory.rename_host(web1.id, name).await.unwrap_err();
        assert!(
            matches!(err, Error::Invalid { field: "name", ref message } if message.contains(says)),
            "{name:?}: {err:?}"
        );
    }
    assert_eq!(inventory.host(web1.id).await.unwrap().unwrap().name, "web1");
}

#[tokio::test]
async fn renaming_a_host_to_its_own_name_is_allowed() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;

    assert_eq!(
        inventory
            .rename_host(web1.id, " WEB1 ")
            .await
            .unwrap()
            .value,
        web1
    );
}

#[tokio::test]
async fn renaming_an_unknown_host_is_not_found() {
    let db = temp_db();
    let inventory = open(&db).await;

    let err = inventory.rename_host(42, "web1").await.unwrap_err();

    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn the_delete_preview_lists_the_host_and_its_own_variables() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();

    let preview = inventory.host_deletion(web1.id).await.unwrap();

    assert_eq!(preview.host, web1);
    assert_eq!(
        preview.own_variables,
        [own("port", "80"), own(SERVER_INDEX, "1")]
    );
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
}

#[tokio::test]
async fn deleting_a_host_removes_exactly_what_the_preview_listed() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web2 = inventory.create_host("web2", None).await.unwrap().value;
    inventory
        .set_host_variable(web1.id, "port", "80")
        .await
        .unwrap();
    inventory
        .set_host_variable(web2.id, "port", "81")
        .await
        .unwrap();
    let preview = inventory.host_deletion(web1.id).await.unwrap();

    let done = inventory.delete_host(web1.id).await.unwrap();

    assert_eq!(done.value, preview);
    assert_eq!(inventory.host(web1.id).await.unwrap(), None);
    assert_eq!(
        inventory.hosts().await.unwrap(),
        std::slice::from_ref(&web2)
    );
    for table in ["host_variable", "host_group_membership", "service_instance"] {
        assert_eq!(rows_referring_to(&db, table, "host", web1.id), 0, "{table}");
    }
    assert_eq!(
        variables_besides_server_index(&inventory, web2.id).await,
        [own("port", "81")]
    );
    assert_eq!(
        inventory.variable_names().await.unwrap(),
        ["port", SERVER_INDEX],
        "Variable names stay"
    );
}

#[tokio::test]
async fn deleting_an_unknown_host_is_not_found() {
    let db = temp_db();
    let inventory = open(&db).await;

    assert!(matches!(
        inventory.host_deletion(42).await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        inventory.delete_host(42).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn a_host_in_groups_or_with_service_instances_cannot_be_deleted_yet() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    // Written by cli/si: lager doesn't manage these relations yet.
    sql(
        &db,
        &format!(
            "INSERT INTO host_group(id, name) VALUES (1, 'eu'), (2, 'web');
             INSERT INTO host_group_membership(host, \"group\") VALUES ({id}, 1), ({id}, 2);
             INSERT INTO service(id, name) VALUES (1, 'nginx');
             INSERT INTO service_instance(service, host, ip, name) VALUES (1, {id}, '192.0.2.10', 'main');",
            id = web1.id
        ),
    );

    let preview = inventory.host_deletion(web1.id).await.unwrap();
    let err = inventory.delete_host(web1.id).await.unwrap_err();

    assert_eq!(
        preview.blockers,
        [
            "It is a member of 2 Groups, and lager can't remove Hosts from Groups yet.",
            "It has 1 Service Instance, and lager can't delete Service Instances yet.",
        ]
    );
    assert!(
        matches!(err, Error::Invalid { field: "host", .. }),
        "{err:?}"
    );
    assert_eq!(inventory.host(web1.id).await.unwrap(), Some(web1.clone()));
    assert_eq!(rows_referring_to(&db, "host_variable", "host", web1.id), 1);
}

#[tokio::test]
async fn the_delete_preview_lists_values_whose_variable_name_is_missing() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    // A value whose Variable row is gone, as foreign keys off allow.
    sql(
        &db,
        &format!(
            "INSERT INTO host_variable(variable, host, value) VALUES (999, {}, '7')",
            web1.id
        ),
    );
    let preview = inventory.host_deletion(web1.id).await.unwrap();

    let done = inventory.delete_host(web1.id).await.unwrap();

    assert!(
        preview
            .own_variables
            .contains(&own("(Variable 999, name missing)", "7")),
        "{:?}",
        preview.own_variables
    );
    assert_eq!(done.value, preview);
    assert_eq!(rows_referring_to(&db, "host_variable", "host", web1.id), 0);
}

#[tokio::test]
async fn renaming_rejects_a_name_another_host_already_shares() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    // cli/si allows duplicate names: make web1 and another Host both "dup".
    sql(
        &db,
        &format!(
            "UPDATE host SET name = 'dup' WHERE id = {};
             INSERT INTO host(name) VALUES ('dup');",
            web1.id
        ),
    );

    let err = inventory.rename_host(web1.id, "dup").await.unwrap_err();

    assert!(
        matches!(err, Error::Invalid { field: "name", ref message } if message.contains("dup")),
        "{err:?}"
    );
}
