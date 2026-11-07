//! Seam 3: `cli/si` reads what lager writes.
//!
//! Needs Python 3 with PyYAML (`cli/si` imports it); each test is skipped
//! with a message when they aren't available.

mod common;

use std::path::Path;
use std::process::Command;

use common::{open, temp_db};
use lager::inventory::Owner;

/// Why `cli/si` can't run here, if it can't.
fn missing_python() -> Option<String> {
    match Command::new("python3").args(["-c", "import yaml"]).output() {
        Err(e) => Some(format!("python3 is not available ({e})")),
        Ok(out) if !out.status.success() => Some(format!(
            "python3 can't import PyYAML: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Ok(_) => None,
    }
}

/// Runs `cli/si --database <database> <args>` and parses its JSON output.
fn run_si(database: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("cli/si"))
        .arg("--database")
        .arg(database)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cli/si {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "cli/si {args:?} printed invalid JSON ({e}):\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

#[tokio::test]
async fn lists_a_host_lager_created() {
    if let Some(reason) = missing_python() {
        eprintln!("SKIPPED lists_a_host_lager_created: {reason}");
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
    if let Some(reason) = missing_python() {
        eprintln!("SKIPPED returns_the_host_variables_lager_set: {reason}");
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
    if let Some(reason) = missing_python() {
        eprintln!("SKIPPED lists_the_groups_lager_wrote_with_hosts_and_children: {reason}");
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
    if let Some(reason) = missing_python() {
        eprintln!("SKIPPED lists_the_group_variables_lager_set: {reason}");
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
