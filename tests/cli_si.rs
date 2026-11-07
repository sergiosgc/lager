//! Seam 3: `cli/si` reads what lager writes.
//!
//! Needs Python 3 with PyYAML (`cli/si` imports it); each test is skipped
//! with a message when they aren't available.

mod common;

use std::path::Path;
use std::process::Command;

use common::{open, temp_db};

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
            .set_host_variable(web1.id, name, value)
            .await
            .unwrap();
    }
    inventory
        .set_host_variable(web1.id, "port", "8443")
        .await
        .unwrap();
    inventory
        .set_host_variable(web2.id, "gone", "1")
        .await
        .unwrap();
    inventory
        .remove_host_variable(web2.id, "gone")
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
