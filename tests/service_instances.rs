//! Seam 1: Service Instances, their Variables and the cascades that remove
//! them, through the Inventory module. IPs are from the documentation
//! ranges only (192.0.2.0/24, 198.51.100.0/24, 2001:db8::/32).

mod common;

use common::{open, rows_referring_to, sql, temp_db};
use lager::inventory::{
    Error, Host, Inventory, OwnVariable, Owner, Service, ServiceInstance, Source,
};

struct Fixture {
    inventory: Inventory,
    web1: Host,
    web2: Host,
    nginx: Service,
}

/// A database with Hosts web1 and web2 and the Service nginx. The database
/// is returned separately for raw-SQL checks; keep it alive for the test.
async fn fixture() -> (common::TempDb, Fixture) {
    let db = temp_db();
    let inventory = open(&db).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    let web2 = inventory.create_host("web2", None).await.unwrap().value;
    let nginx = inventory.create_service("nginx").await.unwrap().value;
    (
        db,
        Fixture {
            inventory,
            web1,
            web2,
            nginx,
        },
    )
}

async fn add(f: &Fixture, host: &Host, name: &str, ip: &str) -> ServiceInstance {
    f.inventory
        .create_service_instance(host.id, f.nginx.id, name, ip)
        .await
        .unwrap()
        .value
}

fn own(name: &str, value: &str) -> OwnVariable {
    OwnVariable {
        name: name.to_string(),
        value: value.to_string(),
    }
}

fn assert_invalid(err: Error, field: &str, says: &str) {
    assert!(
        matches!(err, Error::Invalid { field: f, ref message } if f == field && message.contains(says)),
        "expected {field} error saying {says:?}, got {err:?}"
    );
}

fn labels(instances: &[ServiceInstance]) -> Vec<String> {
    instances
        .iter()
        .map(|i| format!("{}/{}@{}={}", i.service_name, i.name, i.host_name, i.ip))
        .collect()
}

#[tokio::test]
async fn adds_service_instances_listed_by_host_and_by_service() {
    let (_h, f) = fixture().await;
    let mysql = f.inventory.create_service("mysql").await.unwrap().value;
    add(&f, &f.web1, "main", "192.0.2.10").await;
    add(&f, &f.web1, "admin", "2001:db8::10").await;
    add(&f, &f.web2, "main", "192.0.2.20").await;
    f.inventory
        .create_service_instance(f.web1.id, mysql.id, " main ", " 198.51.100.5 ")
        .await
        .unwrap();

    assert_eq!(
        labels(&f.inventory.instances_on_host(f.web1.id).await.unwrap()),
        [
            "mysql/main@web1=198.51.100.5",
            "nginx/admin@web1=2001:db8::10",
            "nginx/main@web1=192.0.2.10",
        ]
    );
    assert_eq!(
        labels(&f.inventory.instances_of_service(f.nginx.id).await.unwrap()),
        [
            "nginx/admin@web1=2001:db8::10",
            "nginx/main@web1=192.0.2.10",
            "nginx/main@web2=192.0.2.20",
        ]
    );
}

#[tokio::test]
async fn rejects_an_invalid_ip_or_a_blank_instance_name() {
    let (_h, f) = fixture().await;

    for ip in [
        "",
        "192.0.2",
        "192.0.2.300",
        "web1.example.org",
        "2001:db8::g",
    ] {
        let err = f
            .inventory
            .create_service_instance(f.web1.id, f.nginx.id, "main", ip)
            .await
            .unwrap_err();
        assert_invalid(err, "ip", "IP");
    }
    let err = f
        .inventory
        .create_service_instance(f.web1.id, f.nginx.id, "  ", "192.0.2.10")
        .await
        .unwrap_err();
    assert_invalid(err, "name", "empty");
    assert!(
        f.inventory
            .instances_on_host(f.web1.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rejects_a_duplicate_service_host_and_instance_name() {
    let (_h, f) = fixture().await;
    add(&f, &f.web1, "main", "192.0.2.10").await;

    let err = f
        .inventory
        .create_service_instance(f.web1.id, f.nginx.id, "main", "192.0.2.11")
        .await
        .unwrap_err();

    assert_invalid(err, "name", "nginx");
    // Same Instance Name on another Host, or of another Service, is fine.
    add(&f, &f.web2, "main", "192.0.2.20").await;
}

#[tokio::test]
async fn edits_the_ip_and_instance_name() {
    let (_h, f) = fixture().await;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    add(&f, &f.web1, "admin", "192.0.2.11").await;

    let edited = f
        .inventory
        .update_service_instance(main.id, "primary", "2001:db8::1")
        .await
        .unwrap()
        .value;

    assert_eq!(
        (edited.id, edited.name.as_str(), edited.ip.as_str()),
        (main.id, "primary", "2001:db8::1")
    );
    assert_eq!(
        f.inventory.service_instance(main.id).await.unwrap(),
        Some(edited)
    );
    let dup = f
        .inventory
        .update_service_instance(main.id, "admin", "192.0.2.10")
        .await
        .unwrap_err();
    assert_invalid(dup, "name", "admin");
    let bad_ip = f
        .inventory
        .update_service_instance(main.id, "primary", "nope")
        .await
        .unwrap_err();
    assert_invalid(bad_ip, "ip", "IP");
    assert!(
        f.inventory
            .update_service_instance(main.id, "primary", "2001:db8::1")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn unknown_hosts_services_and_instances_are_not_found() {
    let (_h, f) = fixture().await;

    for err in [
        f.inventory
            .create_service_instance(999, f.nginx.id, "main", "192.0.2.1")
            .await
            .unwrap_err(),
        f.inventory
            .create_service_instance(f.web1.id, 999, "main", "192.0.2.1")
            .await
            .unwrap_err(),
        f.inventory
            .update_service_instance(999, "main", "192.0.2.1")
            .await
            .unwrap_err(),
        f.inventory
            .service_instance_deletion(999)
            .await
            .unwrap_err(),
        f.inventory
            .set_variable(Owner::ServiceInstance(999), "x", "1")
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }
}

#[tokio::test]
async fn effective_variables_follow_instance_then_service_then_parent_services() {
    let (_h, f) = fixture().await;
    let web = f.inventory.create_service("web").await.unwrap().value;
    f.inventory
        .set_service_parent(f.nginx.id, Some(web.id))
        .await
        .unwrap();
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    let set = |owner, name: &'static str, value: &'static str| {
        let inventory = f.inventory.clone();
        async move { inventory.set_variable(owner, name, value).await.unwrap() }
    };
    set(Owner::Service(web.id), "port", "80").await;
    set(Owner::Service(web.id), "tls", "false").await;
    set(Owner::Service(f.nginx.id), "port", "8080").await;
    set(Owner::ServiceInstance(main.id), "tls", "true").await;
    set(Owner::ServiceInstance(main.id), "workers", "4").await;

    let effective = f
        .inventory
        .effective_variables(Owner::ServiceInstance(main.id))
        .await
        .unwrap();
    let got: Vec<(String, String, String)> = effective
        .into_iter()
        .map(|v| {
            let source = match v.source {
                Source::Own => "own".to_string(),
                Source::Service { name, .. } => name,
                other => panic!("unexpected {other:?}"),
            };
            (v.name, v.value, source)
        })
        .collect();
    let row = |n: &str, v: &str, s: &str| (n.to_string(), v.to_string(), s.to_string());
    assert_eq!(
        got,
        [
            row("port", "8080", "nginx"),
            row("tls", "true", "own"),
            row("workers", "4", "own"),
        ]
    );
}

#[tokio::test]
async fn deleting_a_service_instance_removes_its_variables_after_listing_them() {
    let (h, f) = fixture().await;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    let admin = add(&f, &f.web1, "admin", "192.0.2.11").await;
    f.inventory
        .set_variable(Owner::ServiceInstance(main.id), "workers", "4")
        .await
        .unwrap();
    f.inventory
        .set_variable(Owner::ServiceInstance(admin.id), "workers", "1")
        .await
        .unwrap();

    let preview = f
        .inventory
        .service_instance_deletion(main.id)
        .await
        .unwrap();
    let done = f.inventory.delete_service_instance(main.id).await.unwrap();

    assert_eq!(preview.instance, main);
    assert_eq!(preview.own_variables, [own("workers", "4")]);
    assert_eq!(done.value, preview);
    assert_eq!(f.inventory.service_instance(main.id).await.unwrap(), None);
    assert_eq!(
        rows_referring_to(&h, "service_instance_variable", "instance", main.id),
        0
    );
    assert_eq!(
        f.inventory
            .own_variables(Owner::ServiceInstance(admin.id))
            .await
            .unwrap(),
        [own("workers", "1")]
    );
}

#[tokio::test]
async fn deleting_a_host_removes_its_service_instances_and_their_variables() {
    let (h, f) = fixture().await;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    let other = add(&f, &f.web2, "main", "192.0.2.20").await;
    f.inventory
        .set_variable(Owner::ServiceInstance(main.id), "workers", "4")
        .await
        .unwrap();

    let preview = f.inventory.host_deletion(f.web1.id).await.unwrap();
    let done = f.inventory.delete_host(f.web1.id).await.unwrap();

    assert_eq!(preview.service_instances.len(), 1);
    assert_eq!(preview.service_instances[0].instance, main);
    assert_eq!(
        preview.service_instances[0].own_variables,
        [own("workers", "4")]
    );
    assert_eq!(done.value, preview);
    assert_eq!(
        rows_referring_to(&h, "service_instance", "host", f.web1.id),
        0
    );
    assert_eq!(
        rows_referring_to(&h, "service_instance_variable", "instance", main.id),
        0
    );
    assert_eq!(
        f.inventory.service_instance(other.id).await.unwrap(),
        Some(other)
    );
}

#[tokio::test]
async fn deleting_a_service_removes_its_service_instances_and_their_variables() {
    let (h, f) = fixture().await;
    let mysql = f.inventory.create_service("mysql").await.unwrap().value;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    let db_instance = f
        .inventory
        .create_service_instance(f.web1.id, mysql.id, "main", "192.0.2.30")
        .await
        .unwrap()
        .value;
    f.inventory
        .set_variable(Owner::ServiceInstance(main.id), "workers", "4")
        .await
        .unwrap();

    let preview = f.inventory.service_deletion(f.nginx.id).await.unwrap();
    let done = f.inventory.delete_service(f.nginx.id).await.unwrap();

    assert_eq!(preview.service_instances.len(), 1);
    assert_eq!(preview.service_instances[0].instance, main);
    assert_eq!(done.value, preview);
    assert_eq!(
        rows_referring_to(&h, "service_instance", "service", f.nginx.id),
        0
    );
    assert_eq!(
        rows_referring_to(&h, "service_instance_variable", "instance", main.id),
        0
    );
    assert_eq!(
        f.inventory.service_instance(db_instance.id).await.unwrap(),
        Some(db_instance)
    );
    assert_eq!(f.inventory.hosts().await.unwrap().len(), 2, "Hosts stay");
}

#[tokio::test]
async fn lists_service_instances_whose_service_no_longer_exists() {
    let (h, f) = fixture().await;
    // cli/si's service delete leaves Service Instances behind (ADR 0003).
    sql(
        &h,
        &format!(
            "INSERT INTO service_instance(service, host, ip, name) VALUES (999, {}, '192.0.2.99', 'stale');",
            f.web1.id
        ),
    );

    let listed = f.inventory.instances_on_host(f.web1.id).await.unwrap();
    let preview = f.inventory.host_deletion(f.web1.id).await.unwrap();
    f.inventory.delete_host(f.web1.id).await.unwrap();

    assert_eq!(
        labels(&listed),
        ["(Service 999, missing)/stale@web1=192.0.2.99"]
    );
    assert_eq!(preview.service_instances.len(), 1);
    assert_eq!(
        rows_referring_to(&h, "service_instance", "host", f.web1.id),
        0
    );
}

// Cloning

#[tokio::test]
async fn cloning_copies_the_instance_onto_another_host_with_a_new_ip_and_its_variables() {
    let (_h, f) = fixture().await;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    let source = Owner::ServiceInstance(main.id);
    f.inventory
        .set_variable(source, "workers", "4")
        .await
        .unwrap();
    f.inventory
        .set_variable(source, "tls", r#"{"enabled": true}"#)
        .await
        .unwrap();

    let clone = f
        .inventory
        .clone_service_instance(main.id, f.web2.id, "2001:db8::20")
        .await
        .unwrap()
        .value;

    assert_ne!(clone.id, main.id);
    assert_eq!(
        (
            clone.service_id,
            clone.name.as_str(),
            clone.host_id,
            clone.ip.as_str()
        ),
        (f.nginx.id, "main", f.web2.id, "2001:db8::20")
    );
    assert_eq!(
        f.inventory
            .own_variables(Owner::ServiceInstance(clone.id))
            .await
            .unwrap(),
        [own("tls", r#"{"enabled": true}"#), own("workers", "4")]
    );
}

#[tokio::test]
async fn a_clones_variables_are_independent_of_the_source() {
    let (_h, f) = fixture().await;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    let source = Owner::ServiceInstance(main.id);
    f.inventory
        .set_variable(source, "workers", "4")
        .await
        .unwrap();
    f.inventory
        .set_variable(source, "tls", "true")
        .await
        .unwrap();
    let clone = f
        .inventory
        .clone_service_instance(main.id, f.web2.id, "192.0.2.20")
        .await
        .unwrap()
        .value;
    let cloned = Owner::ServiceInstance(clone.id);

    f.inventory
        .set_variable(cloned, "workers", "8")
        .await
        .unwrap();
    f.inventory.remove_variable(source, "tls").await.unwrap();
    f.inventory
        .set_variable(source, "backlog", "511")
        .await
        .unwrap();

    assert_eq!(
        f.inventory.own_variables(source).await.unwrap(),
        [own("backlog", "511"), own("workers", "4")]
    );
    assert_eq!(
        f.inventory.own_variables(cloned).await.unwrap(),
        [own("tls", "true"), own("workers", "8")]
    );
}

#[tokio::test]
async fn cloning_onto_a_host_that_already_has_the_instance_is_refused() {
    let (_h, f) = fixture().await;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    add(&f, &f.web2, "main", "192.0.2.20").await;

    let onto_other = f
        .inventory
        .clone_service_instance(main.id, f.web2.id, "192.0.2.21")
        .await
        .unwrap_err();
    let onto_itself = f
        .inventory
        .clone_service_instance(main.id, f.web1.id, "192.0.2.11")
        .await
        .unwrap_err();

    assert_invalid(
        onto_other,
        "host",
        "web2 already has a Service Instance of nginx named main",
    );
    assert_invalid(onto_itself, "host", "web1 already has");
    assert_eq!(
        f.inventory
            .instances_of_service(f.nginx.id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn cloning_checks_the_ip_and_that_both_ends_exist() {
    let (_h, f) = fixture().await;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;

    let bad_ip = f
        .inventory
        .clone_service_instance(main.id, f.web2.id, "999.0.2.1")
        .await
        .unwrap_err();
    assert_invalid(bad_ip, "ip", "IP");
    for err in [
        f.inventory
            .clone_service_instance(999, f.web2.id, "192.0.2.1")
            .await
            .unwrap_err(),
        f.inventory
            .clone_service_instance(main.id, 999, "192.0.2.1")
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }
    assert!(
        f.inventory
            .instances_on_host(f.web2.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn cloning_copies_values_as_stored_and_needs_the_source_service() {
    let (h, f) = fixture().await;
    let main = add(&f, &f.web1, "main", "192.0.2.10").await;
    // A value whose Variable name row is gone, as cli/si can leave behind;
    // cli/si's clone copies it too.
    sql(
        &h,
        &format!(
            "INSERT INTO service_instance_variable(variable, instance, value) VALUES (998, {}, '1')",
            main.id
        ),
    );
    let clone = f
        .inventory
        .clone_service_instance(main.id, f.web2.id, "192.0.2.20")
        .await
        .unwrap()
        .value;
    assert_eq!(
        rows_referring_to(&h, "service_instance_variable", "instance", clone.id),
        1
    );

    // An orphaned Service Instance (its Service gone) can't be cloned.
    sql(
        &h,
        &format!("DELETE FROM service WHERE id = {}", f.nginx.id),
    );
    let web3 = f.inventory.create_host("web3", None).await.unwrap().value;
    let err = f
        .inventory
        .clone_service_instance(main.id, web3.id, "192.0.2.21")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
}
