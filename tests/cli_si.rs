//! Seam 3: `cli/si` reads what lager writes.
//!
//! Needs Python 3 with PyYAML (`cli/si` imports it); each test is skipped
//! with a message when they aren't available.

mod common;

use common::{open, run_si, temp_db};
use lager::inventory::Owner;

#[tokio::test]
async fn lists_a_host_lager_created() {
    if common::skip_without_python("lists_a_host_lager_created") {
        return;
    }
    let db = temp_db();
    open(&db).await.create_host("web1", None).await.unwrap();

    let listing = run_si(&db.path, &["--list"]);

    let hostvars = listing["_meta"]["hostvars"]
        .as_object()
        .unwrap_or_else(|| panic!("no _meta.hostvars in cli/si --list output:\n{listing:#}"));
    assert_eq!(hostvars.keys().collect::<Vec<_>>(), ["web1"], "{listing:#}");
}

#[tokio::test]
async fn returns_the_host_variables_lager_set() {
    if common::skip_without_python("returns_the_host_variables_lager_set") {
        return;
    }
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web2 = inventory.create_host("web2", None).await.unwrap().value;
    for (name, value) in [
        ("ntp_server", r#""ntp.example.org""#),
        ("port", "8080"),
        ("dns", r#"["192.0.2.53", "192.0.2.54"]"#),
        ("tls", r#"{"enabled": true, "min_version": 1.2}"#),
    ] {
        inventory
            .set_variable(Owner::Host(web1.id), name, value)
            .await
            .unwrap();
    }
    inventory
        .set_variable(Owner::Host(web1.id), "port", "8443")
        .await
        .unwrap();
    inventory
        .set_variable(Owner::Host(web2.id), "gone", "1")
        .await
        .unwrap();
    inventory
        .remove_variable(Owner::Host(web2.id), "gone")
        .await
        .unwrap();

    let web1_vars = run_si(&db.path, &["--host", "web1"]);
    let web2_vars = run_si(&db.path, &["--host", "web2"]);

    assert_eq!(
        web2_vars["server_index"],
        serde_json::json!(2),
        "{web2_vars:#}"
    );
    for (name, expected) in [
        ("server_index", serde_json::json!(1)),
        ("ntp_server", serde_json::json!("ntp.example.org")),
        ("port", serde_json::json!(8443)),
        ("dns", serde_json::json!(["192.0.2.53", "192.0.2.54"])),
        (
            "tls",
            serde_json::json!({"enabled": true, "min_version": 1.2}),
        ),
    ] {
        assert_eq!(web1_vars[name], expected, "{name} in:\n{web1_vars:#}");
    }
    assert!(web2_vars.get("gone").is_none(), "{web2_vars:#}");
}

#[tokio::test]
async fn lists_the_groups_lager_wrote_with_hosts_and_children() {
    if common::skip_without_python("lists_the_groups_lager_wrote_with_hosts_and_children") {
        return;
    }
    let db = temp_db();
    let inventory = open(&db).await;
    let all = inventory.create_group("all_sites").await.unwrap().value;
    let eu = inventory.create_group("eu").await.unwrap().value;
    let us = inventory.create_group("us").await.unwrap().value;
    let gone = inventory.create_group("gone").await.unwrap().value;
    inventory
        .set_group_parent(eu.id, Some(all.id))
        .await
        .unwrap();
    inventory
        .set_group_parent(us.id, Some(all.id))
        .await
        .unwrap();
    inventory
        .set_group_parent(gone.id, Some(eu.id))
        .await
        .unwrap();
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web2 = inventory.create_host("web2", None).await.unwrap().value;
    inventory.add_host_to_group(web1.id, eu.id).await.unwrap();
    inventory
        .set_host_groups(web2.id, &[eu.id, us.id])
        .await
        .unwrap();
    inventory.add_host_to_group(web2.id, gone.id).await.unwrap();
    inventory.delete_group(gone.id).await.unwrap();

    let listing = run_si(&db.path, &["--list"]);

    let group = |name: &str| (&listing[name]["hosts"], &listing[name]["children"]);
    assert_eq!(
        group("all_sites"),
        (&serde_json::json!([]), &serde_json::json!(["eu", "us"])),
        "{listing:#}"
    );
    assert_eq!(
        group("eu"),
        (&serde_json::json!(["web1", "web2"]), &serde_json::json!([])),
        "{listing:#}"
    );
    assert_eq!(
        group("us"),
        (&serde_json::json!(["web2"]), &serde_json::json!([])),
        "{listing:#}"
    );
    assert!(listing.get("gone").is_none(), "{listing:#}");
}

#[tokio::test]
async fn lists_the_group_variables_lager_set() {
    if common::skip_without_python("lists_the_group_variables_lager_set") {
        return;
    }
    let db = temp_db();
    let inventory = open(&db).await;
    let web = inventory.create_group("web").await.unwrap().value;
    let eu = inventory.create_group("eu").await.unwrap().value;
    for (name, value) in [
        ("ntp_server", r#""ntp.example.org""#),
        ("ansible_group_priority", "10"),
        ("dns", r#"["192.0.2.53"]"#),
        ("gone", "1"),
    ] {
        inventory
            .set_variable(Owner::Group(web.id), name, value)
            .await
            .unwrap();
    }
    inventory
        .remove_variable(Owner::Group(web.id), "gone")
        .await
        .unwrap();
    inventory
        .set_variable(Owner::Group(eu.id), "dns", r#"["198.51.100.53"]"#)
        .await
        .unwrap();

    let listing = run_si(&db.path, &["--list"]);

    assert_eq!(
        listing["web"]["vars"],
        serde_json::json!({
            "ntp_server": "ntp.example.org",
            "ansible_group_priority": 10,
            "dns": ["192.0.2.53"],
        }),
        "{listing:#}"
    );
    assert_eq!(
        listing["eu"]["vars"],
        serde_json::json!({"dns": ["198.51.100.53"]}),
        "{listing:#}"
    );
}

#[tokio::test]
async fn shows_the_service_instances_lager_wrote_with_hierarchy_and_vars() {
    if common::skip_without_python(
        "shows_the_service_instances_lager_wrote_with_hierarchy_and_vars",
    ) {
        return;
    }
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web = inventory.create_service("web").await.unwrap().value;
    let nginx = inventory.create_service("nginx").await.unwrap().value;
    inventory
        .set_service_parent(nginx.id, Some(web.id))
        .await
        .unwrap();
    let main = inventory
        .create_service_instance(web1.id, nginx.id, "main", "192.0.2.10")
        .await
        .unwrap()
        .value;
    let admin = inventory
        .create_service_instance(web1.id, nginx.id, "admin", "2001:db8::10")
        .await
        .unwrap()
        .value;
    for (owner, name, value) in [
        (Owner::Service(web.id), "port", "80"),
        (Owner::Service(web.id), "tls", "false"),
        (Owner::Service(nginx.id), "port", "8080"),
        (Owner::ServiceInstance(main.id), "tls", "true"),
        (Owner::ServiceInstance(main.id), "workers", "4"),
    ] {
        inventory.set_variable(owner, name, value).await.unwrap();
    }
    inventory
        .update_service_instance(admin.id, "admin", "2001:db8::11")
        .await
        .unwrap();

    let host = run_si(&db.path, &["--host", "web1"]);
    let listing = run_si(&db.path, &["--list"]);

    let expected = serde_json::json!({
        "main": {
            "service": "nginx",
            "ip": "192.0.2.10",
            "hierarchy": ["nginx", "web"],
            "vars": {"port": 8080, "tls": true, "workers": 4},
        },
        "admin": {
            "service": "nginx",
            "ip": "2001:db8::11",
            "hierarchy": ["nginx", "web"],
            "vars": {"port": 8080, "tls": false},
        },
    });
    assert_eq!(host["service_instances"]["nginx"], expected, "{host:#}");
    assert_eq!(
        listing["_meta"]["hostvars"]["web1"]["service_instances"]["nginx"], expected,
        "{listing:#}"
    );
}
