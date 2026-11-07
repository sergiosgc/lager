//! Seam 1: Group Own Variables and Effective Variables (Inheritance),
//! through the Inventory module. All fixtures are synthetic.

mod common;

use common::{open, rows_referring_to, sql, temp_db};
use lager::inventory::{EffectiveVariable, Error, Group, Inventory, OwnVariable, Owner, Source};

async fn group(inventory: &Inventory, name: &str) -> Group {
    inventory.create_group(name).await.unwrap().value
}

async fn nest(inventory: &Inventory, child: &Group, parent: &Group) {
    inventory
        .set_group_parent(child.id, Some(parent.id))
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

/// The Effective Variable called `name`, if any.
async fn effective(inventory: &Inventory, owner: Owner, name: &str) -> Option<EffectiveVariable> {
    inventory
        .effective_variables(owner)
        .await
        .unwrap()
        .into_iter()
        .find(|v| v.name == name)
}

/// The value and where it came from, as "value from source".
async fn resolved(inventory: &Inventory, owner: Owner, name: &str) -> String {
    let v = effective(inventory, owner, name)
        .await
        .unwrap_or_else(|| panic!("no Effective Variable {name}"));
    format!("{} from {}", v.value, source_name(&v.source))
}

fn source_name(source: &Source) -> String {
    match source {
        Source::Own => "own".to_string(),
        Source::Group { name, .. } => name.clone(),
    }
}

// Group Own Variables

#[tokio::test]
async fn sets_edits_and_removes_group_own_variables() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let owner = Owner::Group(web.id);

    set(&inventory, owner, "port", "80").await;
    set(&inventory, owner, "ntp_server", r#""ntp.example.org""#).await;
    set(&inventory, owner, "port", "8080").await;
    assert_eq!(
        inventory.own_variables(owner).await.unwrap(),
        [
            own("ntp_server", r#""ntp.example.org""#),
            own("port", "8080")
        ]
    );

    inventory.remove_variable(owner, "port").await.unwrap();
    assert_eq!(
        inventory.own_variables(owner).await.unwrap(),
        [own("ntp_server", r#""ntp.example.org""#)]
    );
    assert!(
        inventory
            .variable_names()
            .await
            .unwrap()
            .contains(&"port".to_string())
    );
}

#[tokio::test]
async fn group_variables_follow_the_host_variable_rules() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let owner = Owner::Group(web.id);

    let bad_json = inventory
        .set_variable(owner, "port", "eighty")
        .await
        .unwrap_err();
    let blank = inventory.set_variable(owner, " ", "1").await.unwrap_err();
    let unknown = inventory
        .set_variable(Owner::Group(999), "port", "1")
        .await
        .unwrap_err();

    assert!(
        matches!(bad_json, Error::Invalid { field: "value", .. }),
        "{bad_json:?}"
    );
    assert!(
        matches!(blank, Error::Invalid { field: "name", .. }),
        "{blank:?}"
    );
    assert!(matches!(unknown, Error::NotFound(_)), "{unknown:?}");
    assert!(inventory.own_variables(owner).await.unwrap().is_empty());
}

#[tokio::test]
async fn deleting_a_group_removes_its_own_variables_after_listing_them() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let eu = group(&inventory, "eu").await;
    set(&inventory, Owner::Group(web.id), "port", "80").await;
    set(&inventory, Owner::Group(eu.id), "port", "81").await;

    let preview = inventory.group_deletion(web.id).await.unwrap();
    let done = inventory.delete_group(web.id).await.unwrap();

    assert_eq!(preview.own_variables, [own("port", "80")]);
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
    assert_eq!(done.value, preview);
    assert_eq!(
        rows_referring_to(&db, "host_group_variable", "\"group\"", web.id),
        0
    );
    assert_eq!(
        inventory.own_variables(Owner::Group(eu.id)).await.unwrap(),
        [own("port", "81")]
    );
    assert!(
        inventory
            .variable_names()
            .await
            .unwrap()
            .contains(&"port".to_string())
    );
}

// Host Effective Variables

#[tokio::test]
async fn a_hosts_own_value_wins_over_its_groups() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory.add_host_to_group(web1.id, web.id).await.unwrap();
    set(&inventory, Owner::Group(web.id), "port", "80").await;
    set(&inventory, Owner::Group(web.id), "tls", "true").await;
    set(&inventory, Owner::Host(web1.id), "port", "8080").await;

    let port = effective(&inventory, Owner::Host(web1.id), "port")
        .await
        .unwrap();

    assert_eq!((port.value.as_str(), &port.source), ("8080", &Source::Own));
    assert_eq!(
        port.overrides,
        Some(Source::Group {
            id: web.id,
            name: "web".to_string()
        })
    );
    assert_eq!(
        resolved(&inventory, Owner::Host(web1.id), "tls").await,
        "true from web"
    );
}

#[tokio::test]
async fn nearer_groups_win_over_their_parent_groups() {
    let db = temp_db();
    let inventory = open(&db).await;
    let all = group(&inventory, "all_sites").await;
    let eu = group(&inventory, "eu").await;
    let lisbon = group(&inventory, "lisbon").await;
    nest(&inventory, &eu, &all).await;
    nest(&inventory, &lisbon, &eu).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .add_host_to_group(web1.id, lisbon.id)
        .await
        .unwrap();
    for (g, ntp) in [(&all, "\"ntp.all.example\""), (&eu, "\"ntp.eu.example\"")] {
        set(&inventory, Owner::Group(g.id), "ntp_server", ntp).await;
    }
    set(&inventory, Owner::Group(all.id), "dns", "\"192.0.2.53\"").await;

    let host = Owner::Host(web1.id);
    assert_eq!(
        resolved(&inventory, host, "ntp_server").await,
        "\"ntp.eu.example\" from eu"
    );
    assert_eq!(
        resolved(&inventory, host, "dns").await,
        "\"192.0.2.53\" from all_sites"
    );
}

#[tokio::test]
async fn a_child_group_wins_over_its_parent_even_when_the_host_is_in_both() {
    let db = temp_db();
    let inventory = open(&db).await;
    let shared = group(&inventory, "shared").await;
    let middle = group(&inventory, "middle").await;
    nest(&inventory, &middle, &shared).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_groups(web1.id, &[shared.id, middle.id])
        .await
        .unwrap();
    // shared would win a name tie-break; the child must win anyway.
    set(&inventory, Owner::Group(shared.id), "x", "1").await;
    set(&inventory, Owner::Group(middle.id), "x", "2").await;

    assert_eq!(
        resolved(&inventory, Owner::Host(web1.id), "x").await,
        "2 from middle"
    );
}

#[tokio::test]
async fn deeper_groups_win_over_shallower_ones_whatever_their_priority() {
    let db = temp_db();
    let inventory = open(&db).await;
    let a = group(&inventory, "a").await;
    let c = group(&inventory, "c").await;
    let b = group(&inventory, "b").await;
    nest(&inventory, &b, &c).await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    // Both direct Groups, but b is a level deeper below the top (Ansible's
    // depth), so it wins even against a's higher priority.
    inventory
        .set_host_groups(web1.id, &[a.id, b.id])
        .await
        .unwrap();
    set(&inventory, Owner::Group(a.id), "port", "1").await;
    set(
        &inventory,
        Owner::Group(a.id),
        "ansible_group_priority",
        "10",
    )
    .await;
    set(&inventory, Owner::Group(b.id), "port", "2").await;

    assert_eq!(
        resolved(&inventory, Owner::Host(web1.id), "port").await,
        "2 from b"
    );
}

#[tokio::test]
async fn ansible_group_priority_may_be_written_as_a_string_or_a_float() {
    let db = temp_db();
    let inventory = open(&db).await;
    let a = group(&inventory, "a").await;
    let m = group(&inventory, "m").await;
    let z = group(&inventory, "z").await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_groups(web1.id, &[a.id, m.id, z.id])
        .await
        .unwrap();
    set(&inventory, Owner::Group(a.id), "port", "1").await;
    set(&inventory, Owner::Group(m.id), "port", "2").await;
    set(&inventory, Owner::Group(z.id), "port", "3").await;
    // As Ansible's int(): "20" is 20, 15.7 is 15; both beat z's default 1.
    set(
        &inventory,
        Owner::Group(a.id),
        "ansible_group_priority",
        "\"20\"",
    )
    .await;
    set(
        &inventory,
        Owner::Group(m.id),
        "ansible_group_priority",
        "15.7",
    )
    .await;

    assert_eq!(
        resolved(&inventory, Owner::Host(web1.id), "port").await,
        "1 from a"
    );
}

#[tokio::test]
async fn at_the_same_distance_the_highest_ansible_group_priority_wins() {
    let db = temp_db();
    let inventory = open(&db).await;
    let a = group(&inventory, "a").await;
    let z = group(&inventory, "z").await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_groups(web1.id, &[a.id, z.id])
        .await
        .unwrap();
    set(&inventory, Owner::Group(a.id), "port", "1").await;
    set(&inventory, Owner::Group(z.id), "port", "2").await;
    set(
        &inventory,
        Owner::Group(a.id),
        "ansible_group_priority",
        "10",
    )
    .await;

    assert_eq!(
        resolved(&inventory, Owner::Host(web1.id), "port").await,
        "1 from a"
    );
}

#[tokio::test]
async fn at_the_same_distance_and_priority_the_last_by_name_wins() {
    let db = temp_db();
    let inventory = open(&db).await;
    let a = group(&inventory, "a").await;
    let z = group(&inventory, "z").await;
    let m = group(&inventory, "m").await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_groups(web1.id, &[a.id, z.id, m.id])
        .await
        .unwrap();
    for g in [&a, &z, &m] {
        set(
            &inventory,
            Owner::Group(g.id),
            "port",
            &format!("\"{}\"", g.name),
        )
        .await;
    }
    // An explicit priority equal to Ansible's default changes nothing.
    set(
        &inventory,
        Owner::Group(m.id),
        "ansible_group_priority",
        "1",
    )
    .await;

    assert_eq!(
        resolved(&inventory, Owner::Host(web1.id), "port").await,
        "\"z\" from z"
    );
}

#[tokio::test]
async fn an_unreadable_ansible_group_priority_counts_as_the_default() {
    let db = temp_db();
    let inventory = open(&db).await;
    let a = group(&inventory, "a").await;
    let z = group(&inventory, "z").await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory
        .set_host_groups(web1.id, &[a.id, z.id])
        .await
        .unwrap();
    set(&inventory, Owner::Group(a.id), "port", "1").await;
    set(&inventory, Owner::Group(z.id), "port", "2").await;
    set(
        &inventory,
        Owner::Group(a.id),
        "ansible_group_priority",
        "\"high\"",
    )
    .await;

    assert_eq!(
        resolved(&inventory, Owner::Host(web1.id), "port").await,
        "2 from z"
    );
}

#[tokio::test]
async fn effective_variables_list_everything_sorted_by_name() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory.add_host_to_group(web1.id, web.id).await.unwrap();
    set(&inventory, Owner::Group(web.id), "b", "1").await;
    set(&inventory, Owner::Host(web1.id), "a", "2").await;

    let names: Vec<String> = inventory
        .effective_variables(Owner::Host(web1.id))
        .await
        .unwrap()
        .into_iter()
        .map(|v| v.name)
        .collect();

    assert_eq!(names, ["a", "b", "server_index"]);
    let a = effective(&inventory, Owner::Host(web1.id), "a")
        .await
        .unwrap();
    assert_eq!(a.overrides, None, "nothing inherited to override");
}

// Group Effective Variables

#[tokio::test]
async fn group_effective_variables_resolve_through_parent_groups() {
    let db = temp_db();
    let inventory = open(&db).await;
    let all = group(&inventory, "all_sites").await;
    let eu = group(&inventory, "eu").await;
    let lisbon = group(&inventory, "lisbon").await;
    nest(&inventory, &eu, &all).await;
    nest(&inventory, &lisbon, &eu).await;
    set(&inventory, Owner::Group(all.id), "ntp_server", "\"all\"").await;
    set(&inventory, Owner::Group(all.id), "dns", "\"all\"").await;
    set(&inventory, Owner::Group(eu.id), "ntp_server", "\"eu\"").await;
    set(&inventory, Owner::Group(lisbon.id), "dns", "\"lisbon\"").await;

    let lisbon_owner = Owner::Group(lisbon.id);
    assert_eq!(
        resolved(&inventory, lisbon_owner, "ntp_server").await,
        "\"eu\" from eu"
    );
    assert_eq!(
        resolved(&inventory, lisbon_owner, "dns").await,
        "\"lisbon\" from own"
    );
    let dns = effective(&inventory, lisbon_owner, "dns").await.unwrap();
    assert_eq!(
        dns.overrides.as_ref().map(source_name),
        Some("all_sites".to_string())
    );
    assert_eq!(
        resolved(&inventory, Owner::Group(eu.id), "dns").await,
        "\"all\" from all_sites"
    );
}

#[tokio::test]
async fn effective_variables_survive_a_cycle_already_in_the_data() {
    let db = temp_db();
    let inventory = open(&db).await;
    let x = group(&inventory, "x").await;
    let y = group(&inventory, "y").await;
    let web1 = inventory.create_host("web1", None).await.unwrap().value;
    inventory.add_host_to_group(web1.id, x.id).await.unwrap();
    set(&inventory, Owner::Group(y.id), "port", "80").await;
    // cli/si doesn't reject cycles (#2).
    sql(
        &db,
        &format!(
            "UPDATE host_group SET parent = {y} WHERE id = {x};
             UPDATE host_group SET parent = {x} WHERE id = {y};",
            x = x.id,
            y = y.id
        ),
    );

    assert_eq!(
        resolved(&inventory, Owner::Host(web1.id), "port").await,
        "80 from y"
    );
    assert_eq!(
        resolved(&inventory, Owner::Group(x.id), "port").await,
        "80 from y"
    );
}

#[tokio::test]
async fn effective_variables_of_an_unknown_owner_are_not_found() {
    let db = temp_db();
    let inventory = open(&db).await;

    for owner in [Owner::Host(999), Owner::Group(999)] {
        let err = inventory.effective_variables(owner).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }
}
