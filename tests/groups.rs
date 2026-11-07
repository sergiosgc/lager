//! Seam 1: Groups, their hierarchy and Host membership, through the
//! Inventory module.

mod common;

use common::{open, rows_referring_to, sql, temp_db};
use lager::inventory::{Error, Group, Host, Inventory};

async fn group(inventory: &Inventory, name: &str) -> Group {
    inventory.create_group(name).await.unwrap().value
}

async fn host(inventory: &Inventory, name: &str) -> Host {
    inventory.create_host(name, None).await.unwrap().value
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<String> {
    items.iter().map(|i| name(i).to_string()).collect()
}

fn group_names(groups: &[Group]) -> Vec<String> {
    names(groups, |g| &g.name)
}

fn host_names(hosts: &[Host]) -> Vec<String> {
    names(hosts, |h| &h.name)
}

fn assert_invalid(err: Error, field: &str, says: &str) {
    assert!(
        matches!(err, Error::Invalid { field: f, ref message } if f == field && message.contains(says)),
        "expected {field} error saying {says:?}, got {err:?}"
    );
}

#[tokio::test]
async fn creates_groups_listed_by_name() {
    let db = temp_db();
    let inventory = open(&db).await;

    let web = group(&inventory, "web").await;
    group(&inventory, "eu").await;

    assert_eq!(web.parent_id, None);
    assert_eq!(inventory.group(web.id).await.unwrap(), Some(web.clone()));
    assert_eq!(
        group_names(&inventory.groups().await.unwrap()),
        ["eu", "web"]
    );
}

#[tokio::test]
async fn group_names_must_be_non_empty_and_unique() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let eu = group(&inventory, "eu").await;

    assert_invalid(
        inventory.create_group("  ").await.unwrap_err(),
        "name",
        "empty",
    );
    assert_invalid(
        inventory.create_group("web").await.unwrap_err(),
        "name",
        "web",
    );
    assert_invalid(
        inventory.rename_group(eu.id, " web ").await.unwrap_err(),
        "name",
        "web",
    );
    assert_invalid(
        inventory.rename_group(eu.id, "").await.unwrap_err(),
        "name",
        "empty",
    );
    assert_eq!(
        inventory.create_group(" db ").await.unwrap().value.name,
        "db"
    );
    assert_eq!(
        inventory.rename_group(web.id, "web").await.unwrap().value,
        web
    );
}

#[tokio::test]
async fn renaming_a_group_keeps_its_members_and_children() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let child = group(&inventory, "web-eu").await;
    inventory
        .set_group_parent(child.id, Some(web.id))
        .await
        .unwrap();
    let web1 = host(&inventory, "web1").await;
    inventory.add_host_to_group(web1.id, web.id).await.unwrap();

    let renamed = inventory
        .rename_group(web.id, "frontend")
        .await
        .unwrap()
        .value;

    assert_eq!((renamed.id, renamed.name.as_str()), (web.id, "frontend"));
    assert_eq!(
        host_names(&inventory.group_hosts(web.id).await.unwrap()),
        ["web1"]
    );
    assert_eq!(
        inventory.group(child.id).await.unwrap().unwrap().parent_id,
        Some(web.id)
    );
}

#[tokio::test]
async fn nests_and_un_nests_groups() {
    let db = temp_db();
    let inventory = open(&db).await;
    let all = group(&inventory, "all-sites").await;
    let eu = group(&inventory, "eu").await;
    let us = group(&inventory, "us").await;
    let lisbon = group(&inventory, "lisbon").await;
    inventory
        .set_group_parent(eu.id, Some(all.id))
        .await
        .unwrap();
    inventory
        .set_group_parent(us.id, Some(all.id))
        .await
        .unwrap();
    inventory
        .set_group_parent(lisbon.id, Some(eu.id))
        .await
        .unwrap();

    let tree: Vec<(usize, String)> = inventory
        .group_tree()
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.depth, e.group.name))
        .collect();
    assert_eq!(
        tree,
        [
            (0, "all-sites".to_string()),
            (1, "eu".to_string()),
            (2, "lisbon".to_string()),
            (1, "us".to_string()),
        ]
    );

    let cleared = inventory.set_group_parent(eu.id, None).await.unwrap().value;
    assert_eq!(cleared.parent_id, None);
    assert_eq!(
        inventory.child_groups(all.id).await.unwrap(),
        [inventory.group(us.id).await.unwrap().unwrap()]
    );
}

#[tokio::test]
async fn rejects_a_group_as_its_own_parent() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;

    let err = inventory
        .set_group_parent(web.id, Some(web.id))
        .await
        .unwrap_err();

    assert_invalid(err, "parent", "web → web");
    assert_eq!(
        inventory.group(web.id).await.unwrap().unwrap().parent_id,
        None
    );
}

#[tokio::test]
async fn rejects_a_parent_that_creates_a_cycle_naming_the_loop() {
    let db = temp_db();
    let inventory = open(&db).await;
    let a = group(&inventory, "a").await;
    let b = group(&inventory, "b").await;
    let c = group(&inventory, "c").await;
    inventory.set_group_parent(b.id, Some(a.id)).await.unwrap();
    inventory.set_group_parent(c.id, Some(b.id)).await.unwrap();

    let err = inventory
        .set_group_parent(a.id, Some(c.id))
        .await
        .unwrap_err();

    assert_invalid(err, "parent", "a → c → b → a");
    assert_eq!(
        inventory.group(a.id).await.unwrap().unwrap().parent_id,
        None
    );
}

#[tokio::test]
async fn the_tree_survives_a_cycle_already_in_the_data() {
    let db = temp_db();
    let inventory = open(&db).await;
    let top = group(&inventory, "top").await;
    let x = group(&inventory, "x").await;
    let y = group(&inventory, "y").await;
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

    let tree = inventory.group_tree().await.unwrap();

    let mut listed = group_names(&tree.into_iter().map(|e| e.group).collect::<Vec<_>>());
    listed.sort();
    assert_eq!(listed, ["top", "x", "y"]);
    // Setting a parent elsewhere still terminates.
    let err = inventory.set_group_parent(top.id, Some(x.id)).await;
    assert!(err.is_ok(), "{err:?}");
}

#[tokio::test]
async fn membership_edits_show_from_both_sides() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let eu = group(&inventory, "eu").await;
    let web1 = host(&inventory, "web1").await;
    let web2 = host(&inventory, "web2").await;

    inventory.add_host_to_group(web1.id, web.id).await.unwrap();
    inventory.add_host_to_group(web1.id, eu.id).await.unwrap();
    inventory.add_host_to_group(web2.id, web.id).await.unwrap();
    inventory.add_host_to_group(web2.id, web.id).await.unwrap();

    assert_eq!(
        group_names(&inventory.host_groups(web1.id).await.unwrap()),
        ["eu", "web"]
    );
    assert_eq!(
        host_names(&inventory.group_hosts(web.id).await.unwrap()),
        ["web1", "web2"]
    );
    assert_eq!(
        rows_referring_to(&db, "host_group_membership", "host", web2.id),
        1
    );

    inventory
        .remove_host_from_group(web1.id, web.id)
        .await
        .unwrap();

    assert_eq!(
        group_names(&inventory.host_groups(web1.id).await.unwrap()),
        ["eu"]
    );
    assert_eq!(
        host_names(&inventory.group_hosts(web.id).await.unwrap()),
        ["web2"]
    );
}

#[tokio::test]
async fn replaces_a_hosts_whole_set_of_groups() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let eu = group(&inventory, "eu").await;
    let db_group = group(&inventory, "db").await;
    let web1 = host(&inventory, "web1").await;
    inventory.add_host_to_group(web1.id, web.id).await.unwrap();
    inventory.add_host_to_group(web1.id, eu.id).await.unwrap();

    let done = inventory
        .set_host_groups(web1.id, &[eu.id, db_group.id, db_group.id])
        .await
        .unwrap();

    assert_eq!(group_names(&done.value), ["db", "eu"]);
    assert_eq!(
        group_names(&inventory.host_groups(web1.id).await.unwrap()),
        ["db", "eu"]
    );
    assert!(inventory.group_hosts(web.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn replacing_groups_with_an_unknown_group_changes_nothing() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let web1 = host(&inventory, "web1").await;
    inventory.add_host_to_group(web1.id, web.id).await.unwrap();

    let err = inventory
        .set_host_groups(web1.id, &[999])
        .await
        .unwrap_err();

    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    assert_eq!(
        group_names(&inventory.host_groups(web1.id).await.unwrap()),
        ["web"]
    );
}

#[tokio::test]
async fn membership_of_unknown_hosts_or_groups_is_not_found() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let web1 = host(&inventory, "web1").await;

    for err in [
        inventory.add_host_to_group(999, web.id).await.unwrap_err(),
        inventory.add_host_to_group(web1.id, 999).await.unwrap_err(),
        inventory
            .remove_host_from_group(web1.id, 999)
            .await
            .unwrap_err(),
        inventory.set_group_parent(999, None).await.unwrap_err(),
        inventory
            .set_group_parent(web.id, Some(999))
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }
}

#[tokio::test]
async fn deleting_a_group_removes_memberships_and_un_nests_its_children() {
    let db = temp_db();
    let inventory = open(&db).await;
    let all = group(&inventory, "all-sites").await;
    let eu = group(&inventory, "eu").await;
    let lisbon = group(&inventory, "lisbon").await;
    let paris = group(&inventory, "paris").await;
    inventory
        .set_group_parent(eu.id, Some(all.id))
        .await
        .unwrap();
    inventory
        .set_group_parent(lisbon.id, Some(eu.id))
        .await
        .unwrap();
    inventory
        .set_group_parent(paris.id, Some(eu.id))
        .await
        .unwrap();
    let web1 = host(&inventory, "web1").await;
    let web2 = host(&inventory, "web2").await;
    inventory.add_host_to_group(web1.id, eu.id).await.unwrap();
    inventory.add_host_to_group(web2.id, eu.id).await.unwrap();
    inventory
        .add_host_to_group(web2.id, lisbon.id)
        .await
        .unwrap();

    let preview = inventory.group_deletion(eu.id).await.unwrap();
    let done = inventory.delete_group(eu.id).await.unwrap();

    assert_eq!(preview.group.name, "eu");
    assert_eq!(host_names(&preview.member_hosts), ["web1", "web2"]);
    assert_eq!(group_names(&preview.child_groups), ["lisbon", "paris"]);
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
    assert_eq!(done.value, preview);

    assert_eq!(inventory.group(eu.id).await.unwrap(), None);
    assert_eq!(
        rows_referring_to(&db, "host_group_membership", "\"group\"", eu.id),
        0
    );
    for child in [lisbon.id, paris.id] {
        assert_eq!(
            inventory.group(child).await.unwrap().unwrap().parent_id,
            None
        );
    }
    assert_eq!(
        host_names(&inventory.hosts().await.unwrap()),
        ["web1", "web2"]
    );
    assert_eq!(
        group_names(&inventory.host_groups(web2.id).await.unwrap()),
        ["lisbon"]
    );
}

#[tokio::test]
async fn deleting_a_host_removes_its_memberships() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let eu = group(&inventory, "eu").await;
    let web1 = host(&inventory, "web1").await;
    let web2 = host(&inventory, "web2").await;
    inventory.add_host_to_group(web1.id, web.id).await.unwrap();
    inventory.add_host_to_group(web1.id, eu.id).await.unwrap();
    inventory.add_host_to_group(web2.id, web.id).await.unwrap();

    let preview = inventory.host_deletion(web1.id).await.unwrap();
    let done = inventory.delete_host(web1.id).await.unwrap();

    assert_eq!(group_names(&preview.groups), ["eu", "web"]);
    assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
    assert_eq!(done.value, preview);
    assert_eq!(
        rows_referring_to(&db, "host_group_membership", "host", web1.id),
        0
    );
    assert_eq!(
        host_names(&inventory.group_hosts(web.id).await.unwrap()),
        ["web2"]
    );
    assert_eq!(
        group_names(&inventory.groups().await.unwrap()),
        ["eu", "web"]
    );
}

#[tokio::test]
async fn previews_count_memberships_of_hosts_or_groups_that_no_longer_exist() {
    let db = temp_db();
    let inventory = open(&db).await;
    let web = group(&inventory, "web").await;
    let web1 = host(&inventory, "web1").await;
    inventory.add_host_to_group(web1.id, web.id).await.unwrap();
    // cli/si's deletes leave memberships behind (ADR 0003).
    sql(
        &db,
        &format!(
            "INSERT INTO host_group_membership(host, \"group\") VALUES (998, {web}), ({web1}, 999);",
            web = web.id,
            web1 = web1.id
        ),
    );

    let group_preview = inventory.group_deletion(web.id).await.unwrap();
    let host_preview = inventory.host_deletion(web1.id).await.unwrap();

    assert_eq!(group_preview.missing_memberships, 1);
    assert_eq!(host_preview.missing_memberships, 1);
    inventory.delete_group(web.id).await.unwrap();
    inventory.delete_host(web1.id).await.unwrap();
    assert_eq!(
        rows_referring_to(&db, "host_group_membership", "\"group\"", web.id),
        0
    );
    assert_eq!(
        rows_referring_to(&db, "host_group_membership", "host", web1.id),
        0
    );
}
