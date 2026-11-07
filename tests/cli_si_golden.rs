//! Seam 3, in full: a synthetic inventory built entirely through lager,
//! using its main features together, reads in `cli/si` exactly as recorded in
//! the golden files under `tests/golden/`. This is what keeps Ansible
//! unaffected by which tool the operator used.
//!
//! The comparison is on parsed JSON, so object key order (Python's dict
//! order) doesn't matter; array order does.
//!
//! When a change to lager or `cli/si` is meant to change this output,
//! regenerate the golden files and review the diff before committing:
//!
//! ```sh
//! LAGER_BLESS_GOLDEN=1 cargo test --test cli_si_golden
//! git diff tests/golden/
//! ```
//!
//! Needs Python 3 with PyYAML (`cli/si` imports it); skipped with a message
//! when they aren't available. All data is synthetic, and every IP is from
//! a documentation range (192.0.2.0/24, 198.51.100.0/24, 2001:db8::/32).

mod common;

use std::path::{Path, PathBuf};

use common::{open, run_si, temp_db};
use lager::inventory::{Inventory, Owner};

fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.json"))
}

/// Compares `actual` with the golden file `name`, or rewrites the file when
/// `LAGER_BLESS_GOLDEN=1`.
fn check_golden(name: &str, actual: &serde_json::Value) {
    let path = golden_path(name);
    if std::env::var("LAGER_BLESS_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut text = serde_json::to_string_pretty(actual).unwrap();
        text.push('\n');
        std::fs::write(&path, text).unwrap();
        eprintln!("wrote {}", path.display());
        return;
    }
    let golden = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "can't read golden file {} ({e}); run with LAGER_BLESS_GOLDEN=1 to create it",
            path.display()
        )
    });
    let expected: serde_json::Value = serde_json::from_str(&golden).unwrap();
    assert!(
        actual == &expected,
        "cli/si output differs from {}. If the change is intended, run \
         `LAGER_BLESS_GOLDEN=1 cargo test --test cli_si_golden` and review \
         `git diff tests/golden/`.\nactual:\n{}",
        path.display(),
        serde_json::to_string_pretty(actual).unwrap()
    );
}

async fn set(inventory: &Inventory, owner: Owner, name: &str, value: &str) {
    inventory.set_variable(owner, name, value).await.unwrap();
}

/// Builds the synthetic inventory, using lager's main features together.
async fn build(inventory: &Inventory) {
    // Groups, nested, with Group Variables and ansible_group_priority.
    let group =
        |name: &'static str| async move { inventory.create_group(name).await.unwrap().value };
    let all_sites = group("all_sites").await;
    let eu = group("eu").await;
    let us = group("us").await;
    let lisbon = group("lisbon").await;
    let web = group("web").await;
    let databases = group("db").await;
    for (child, parent) in [(&eu, &all_sites), (&us, &all_sites), (&lisbon, &eu)] {
        inventory
            .set_group_parent(child.id, Some(parent.id))
            .await
            .unwrap();
    }
    set(
        inventory,
        Owner::Group(all_sites.id),
        "ntp_server",
        r#""ntp.example.org""#,
    )
    .await;
    set(
        inventory,
        Owner::Group(all_sites.id),
        "dns",
        r#"["192.0.2.53", "2001:db8::53"]"#,
    )
    .await;
    set(
        inventory,
        Owner::Group(eu.id),
        "ntp_server",
        r#""ntp.eu.example.org""#,
    )
    .await;
    set(
        inventory,
        Owner::Group(eu.id),
        "timezone",
        r#""Europe/Lisbon""#,
    )
    .await;
    set(
        inventory,
        Owner::Group(us.id),
        "timezone",
        r#""America/New_York""#,
    )
    .await;
    set(
        inventory,
        Owner::Group(web.id),
        "ansible_group_priority",
        "10",
    )
    .await;
    set(inventory, Owner::Group(web.id), "timezone", r#""UTC""#).await;
    set(
        inventory,
        Owner::Group(databases.id),
        "backup",
        r#"{"enabled": true, "keep_days": 14}"#,
    )
    .await;

    // Hosts in several Groups, with Host Variables and Server Indexes (one
    // overridden).
    let host = |name: &'static str, index: Option<&'static str>| async move {
        inventory.create_host(name, index).await.unwrap().value
    };
    let web1 = host("web1", None).await;
    let web2 = host("web2", None).await;
    let db1 = host("db1", Some("10")).await;
    inventory
        .set_host_groups(web1.id, &[lisbon.id, web.id])
        .await
        .unwrap();
    inventory
        .set_host_groups(web2.id, &[us.id, web.id])
        .await
        .unwrap();
    inventory.add_host_to_group(db1.id, eu.id).await.unwrap();
    inventory
        .add_host_to_group(db1.id, databases.id)
        .await
        .unwrap();
    set(
        inventory,
        Owner::Host(web1.id),
        "ntp_server",
        r#""ntp.local.example.org""#,
    )
    .await;
    set(inventory, Owner::Host(db1.id), "disk_gb", "500").await;

    // Services with Parent Services and Service Variables.
    let service =
        |name: &'static str| async move { inventory.create_service(name).await.unwrap().value };
    let http = service("http").await;
    let nginx = service("nginx").await;
    let sql = service("sql").await;
    let mariadb = service("mariadb").await;
    inventory
        .set_service_parent(nginx.id, Some(http.id))
        .await
        .unwrap();
    inventory
        .set_service_parent(mariadb.id, Some(sql.id))
        .await
        .unwrap();
    set(inventory, Owner::Service(http.id), "port", "80").await;
    set(inventory, Owner::Service(http.id), "tls", "false").await;
    set(inventory, Owner::Service(nginx.id), "port", "8080").await;
    set(inventory, Owner::Service(sql.id), "port", "3306").await;
    set(inventory, Owner::Service(mariadb.id), "engine", r#""aria""#).await;

    // Service Instances with Variables, including a clone, on IPv4 and
    // IPv6 documentation addresses.
    let instance = |host: i64, service: i64, name: &'static str, ip: &'static str| async move {
        inventory
            .create_service_instance(host, service, name, ip)
            .await
            .unwrap()
            .value
    };
    let web1_nginx = instance(web1.id, nginx.id, "main", "192.0.2.10").await;
    set(
        inventory,
        Owner::ServiceInstance(web1_nginx.id),
        "tls",
        "true",
    )
    .await;
    set(
        inventory,
        Owner::ServiceInstance(web1_nginx.id),
        "workers",
        "4",
    )
    .await;
    let web2_nginx = inventory
        .clone_service_instance(web1_nginx.id, web2.id, "2001:db8::20")
        .await
        .unwrap()
        .value;
    set(
        inventory,
        Owner::ServiceInstance(web2_nginx.id),
        "workers",
        "8",
    )
    .await;
    let db1_main = instance(db1.id, mariadb.id, "main", "198.51.100.5").await;
    set(
        inventory,
        Owner::ServiceInstance(db1_main.id),
        "port",
        "3307",
    )
    .await;
    instance(db1.id, mariadb.id, "replica", "2001:db8::5").await;
}

#[tokio::test]
async fn cli_si_reads_a_full_inventory_built_by_lager_as_recorded() {
    if common::skip_without_python("cli_si_reads_a_full_inventory_built_by_lager_as_recorded") {
        return;
    }
    let db = temp_db();
    build(&open(&db).await).await;

    check_golden("list", &run_si(&db.path, &["--list"]));
    for host in ["web1", "web2", "db1"] {
        check_golden(
            &format!("host-{host}"),
            &run_si(&db.path, &["--host", host]),
        );
    }
}
