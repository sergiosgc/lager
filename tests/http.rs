//! Seam 2: the web app, driven with HTTP requests against a temporary
//! database. Kept deliberately small so Topcoat's breaking changes touch few
//! tests; the rules themselves are covered at the Inventory seam.

mod common;

use common::{open, temp_db};
use topcoat::router::{
    Body, Router, StatusCode, header, request::Request, response::Response, to_bytes,
};

/// The app on a temporary database, with an empty webroot: these tests
/// don't depend on the stylesheets checked out next to them.
async fn app() -> (common::TempDb, Router) {
    let db = temp_db();
    let webroot = db.dir.path().join("webroot");
    let router = lager::web::router(open(&db).await, &webroot);
    (db, router)
}

/// A temporary webroot holding these files, as (relative path, contents).
fn webroot_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let path = root.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    root
}

async fn get(router: &Router, uri: &str) -> Response {
    let request = Request::<()>::get(uri).body(Body::empty()).unwrap();
    router.handle(request).await
}

/// Submits a form the way a browser does from a lager page.
async fn post_form(router: &Router, uri: &str, form: &str) -> Response {
    let request = Request::<()>::post(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header(header::HOST, "127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(form.to_string()))
        .unwrap();
    router.handle(request).await
}

async fn text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn location(response: &Response) -> String {
    response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn creating_a_host_lands_on_its_page_and_lists_it() {
    let (_db, router) = app().await;

    let form = text(get(&router, "/hosts/new").await).await;
    assert!(form.contains(r#"name="name""#), "{form}");

    let created = post_form(&router, "/hosts/new", "name=web1").await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let page = get(&router, &location(&created)).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(text(page).await.contains("<h1>web1</h1>"));

    let list = text(get(&router, "/hosts").await).await;
    assert!(list.contains("web1"), "{list}");
}

#[tokio::test]
async fn a_rejected_host_re_renders_the_form_with_the_error_and_input() {
    let (_db, router) = app().await;
    post_form(&router, "/hosts/new", "name=web1").await;

    let rejected = post_form(&router, "/hosts/new", "name=web1").await;

    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = text(rejected).await;
    assert!(page.contains("A Host named web1 already exists."), "{page}");
    assert!(page.contains(r#"value="web1""#), "{page}");
}

#[tokio::test]
async fn an_unknown_host_is_not_found() {
    let (_db, router) = app().await;
    assert_eq!(
        get(&router, "/hosts/42").await.status(),
        StatusCode::NOT_FOUND
    );
}

/// Creates a Host through the form and returns its page's path.
async fn create_host(router: &Router, name: &str) -> String {
    location(&post_form(router, "/hosts/new", &format!("name={name}")).await)
}

#[tokio::test]
async fn sets_edits_and_removes_a_host_variable_from_its_page() {
    let (_db, router) = app().await;
    let host = create_host(&router, "web1").await;
    let variables = format!("{host}/variables");

    let added = post_form(&router, &variables, "form=add&name=port&value=80").await;
    assert_eq!(added.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&added), format!("{host}#variables"));
    let page = text(get(&router, &host).await).await;
    assert!(page.contains(r#"<th scope="row">port</th>"#), "{page}");
    assert!(page.contains(">80</textarea>"), "{page}");

    let edited = post_form(&router, &variables, "form=edit&name=port&value=8080").await;
    assert_eq!(edited.status(), StatusCode::SEE_OTHER);
    assert!(
        text(get(&router, &host).await)
            .await
            .contains(">8080</textarea>")
    );

    let removed = post_form(&router, &format!("{variables}/remove"), "name=port").await;
    assert_eq!(removed.status(), StatusCode::SEE_OTHER);
    let page = text(get(&router, &host).await).await;
    assert!(!page.contains(r#"<th scope="row">port</th>"#), "{page}");
}

#[tokio::test]
async fn invalid_json_re_renders_the_host_page_with_the_error_and_input() {
    let (_db, router) = app().await;
    let host = create_host(&router, "web1").await;
    let variables = format!("{host}/variables");
    post_form(&router, &variables, "form=add&name=port&value=80").await;

    let rejected = post_form(
        &router,
        &variables,
        "form=add&name=ntp&value=ntp.example.org",
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = text(rejected).await;
    assert!(page.contains("This is not valid JSON"), "{page}");
    assert!(page.contains(r#"value="ntp""#), "{page}");
    assert!(page.contains(">ntp.example.org</textarea>"), "{page}");

    let rejected = post_form(&router, &variables, "form=edit&name=port&value=%5B1%2C").await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = text(rejected).await;
    assert!(page.contains("This is not valid JSON"), "{page}");
    assert!(page.contains(">[1,</textarea>"), "{page}");

    let rejected = post_form(&router, &variables, "form=add&name=+&value=%22x%22").await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = text(rejected).await;
    assert!(page.contains("A Variable name can"), "{page}");
    assert!(page.contains(r#"value=" ""#), "{page}");
    assert!(page.contains(">\"x\"</textarea>"), "{page}");
}

#[tokio::test]
async fn suggests_the_next_server_index_and_warns_about_a_duplicate_after_saving() {
    let (_db, router) = app().await;
    create_host(&router, "web1").await;

    let form = text(get(&router, "/hosts/new").await).await;
    assert!(form.contains(r#"name="server_index""#), "{form}");
    assert!(form.contains(r#"value="2""#), "{form}");

    let created = post_form(&router, "/hosts/new", "name=web2&server_index=1").await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let page = text(get(&router, &location(&created)).await).await;
    assert!(
        page.contains("Server Index 1 is also used by web1."),
        "{page}"
    );
}

#[tokio::test]
async fn renames_a_host_from_its_rename_page() {
    let (_db, router) = app().await;
    let host = create_host(&router, "web1").await;
    create_host(&router, "web2").await;

    let form = text(get(&router, &format!("{host}/rename")).await).await;
    assert!(form.contains(r#"value="web1""#), "{form}");

    let rejected = post_form(&router, &format!("{host}/rename"), "name=web2").await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = text(rejected).await;
    assert!(page.contains("A Host named web2 already exists."), "{page}");
    assert!(page.contains(r#"value="web2""#), "{page}");

    let renamed = post_form(&router, &format!("{host}/rename"), "name=web-01").await;
    assert_eq!(location(&renamed), host);
    assert!(
        text(get(&router, &host).await)
            .await
            .contains("<h1>web-01</h1>")
    );
}

#[tokio::test]
async fn deleting_a_host_goes_through_a_confirmation_listing_its_variables() {
    let (_db, router) = app().await;
    let host = create_host(&router, "web1").await;
    post_form(
        &router,
        &format!("{host}/variables"),
        "form=add&name=port&value=80",
    )
    .await;

    // Opening the confirmation, then cancelling, changes nothing.
    let confirm = text(get(&router, &format!("{host}/delete")).await).await;
    assert!(confirm.contains("port"), "{confirm}");
    assert!(confirm.contains("server_index"), "{confirm}");
    assert!(
        confirm.contains(&format!(r#"href="{host}""#)),
        "cancel link: {confirm}"
    );
    let page = text(get(&router, &host).await).await;
    assert!(page.contains(r#"<th scope="row">port</th>"#), "{page}");

    let deleted = post_form(&router, &format!("{host}/delete"), "").await;
    assert_eq!(deleted.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&deleted), "/hosts");
    assert_eq!(get(&router, &host).await.status(), StatusCode::NOT_FOUND);
    assert!(!text(get(&router, "/hosts").await).await.contains("web1"));
}

/// Creates a Group through the form and returns its page's path.
async fn create_group(router: &Router, name: &str) -> String {
    let created = post_form(router, "/groups/new", &format!("name={name}")).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    location(&created)
}

/// The id at the end of an entity page's path.
fn id_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap()
}

#[tokio::test]
async fn manages_groups_nesting_and_membership_from_both_sides() {
    let (_db, router) = app().await;
    let eu = create_group(&router, "eu").await;
    let all = create_group(&router, "all_sites").await;
    let host = create_host(&router, "web1").await;

    // Nesting, and a cycle rejected with the loop named.
    let nested = post_form(
        &router,
        &format!("{eu}/parent"),
        &format!("parent={}", id_of(&all)),
    )
    .await;
    assert_eq!(nested.status(), StatusCode::SEE_OTHER);
    let cycle = post_form(
        &router,
        &format!("{all}/parent"),
        &format!("parent={}", id_of(&eu)),
    )
    .await;
    assert_eq!(cycle.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(cycle).await.contains("all_sites → eu → all_sites"));
    let cleared = post_form(&router, &format!("{eu}/parent"), "parent=").await;
    assert_eq!(cleared.status(), StatusCode::SEE_OTHER);
    assert!(
        text(get(&router, &eu).await)
            .await
            .contains("A top-level Group.")
    );
    post_form(
        &router,
        &format!("{eu}/parent"),
        &format!("parent={}", id_of(&all)),
    )
    .await;
    let list = text(get(&router, "/groups").await).await;
    let (all_at, eu_at) = (
        list.find(">all_sites<").unwrap(),
        list.find(">eu<").unwrap(),
    );
    assert!(all_at < eu_at, "eu nested under all_sites: {list}");

    // Add from the Group page, see it on the Host page.
    let added = post_form(
        &router,
        &format!("{eu}/hosts"),
        &format!("host={}", id_of(&host)),
    )
    .await;
    assert_eq!(added.status(), StatusCode::SEE_OTHER);
    assert!(
        text(get(&router, &host).await)
            .await
            .contains(&format!(r#"href="{eu}""#))
    );

    // Replace from the Host page, see it on both Group pages.
    let replaced = post_form(
        &router,
        &format!("{host}/groups/set"),
        &format!("group={}", id_of(&all)),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::SEE_OTHER);
    assert!(
        text(get(&router, &all).await)
            .await
            .contains(&format!(r#"href="{host}""#))
    );
    assert!(
        !text(get(&router, &eu).await)
            .await
            .contains(&format!(r#"href="{host}""#))
    );

    // Remove from the Host page.
    let removed = post_form(
        &router,
        &format!("{host}/groups/remove"),
        &format!("group={}", id_of(&all)),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::SEE_OTHER);
    assert!(
        !text(get(&router, &all).await)
            .await
            .contains(&format!(r#"href="{host}""#))
    );
}

#[tokio::test]
async fn deleting_a_group_goes_through_a_confirmation() {
    let (_db, router) = app().await;
    let eu = create_group(&router, "eu").await;
    let all = create_group(&router, "all_sites").await;
    let host = create_host(&router, "web1").await;
    post_form(
        &router,
        &format!("{eu}/parent"),
        &format!("parent={}", id_of(&all)),
    )
    .await;
    post_form(
        &router,
        &format!("{all}/hosts"),
        &format!("host={}", id_of(&host)),
    )
    .await;

    let host_confirm = text(get(&router, &format!("{host}/delete")).await).await;
    assert!(host_confirm.contains(">all_sites<"), "{host_confirm}");

    let confirm = text(get(&router, &format!("{all}/delete")).await).await;
    assert!(confirm.contains(">web1<"), "{confirm}");
    assert!(confirm.contains(">eu<"), "{confirm}");

    let deleted = post_form(&router, &format!("{all}/delete"), "").await;
    assert_eq!(deleted.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&deleted), "/groups");
    assert_eq!(get(&router, &all).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(get(&router, &eu).await.status(), StatusCode::OK);
    assert_eq!(get(&router, &host).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn group_variables_show_as_effective_variables_with_their_source() {
    let (_db, router) = app().await;
    let web = create_group(&router, "web").await;
    let host = create_host(&router, "web1").await;
    post_form(
        &router,
        &format!("{web}/hosts"),
        &format!("host={}", id_of(&host)),
    )
    .await;

    let set = post_form(
        &router,
        &format!("{web}/variables"),
        "form=add&name=port&value=80",
    )
    .await;
    assert_eq!(set.status(), StatusCode::SEE_OTHER);
    let rejected = post_form(
        &router,
        &format!("{web}/variables"),
        "form=add&name=tls&value=yes",
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = text(rejected).await;
    assert!(page.contains("This is not valid JSON"), "{page}");
    assert!(page.contains(">yes</textarea>"), "{page}");

    let page = text(get(&router, &host).await).await;
    let effective = &page[page.find(r#"id="effective""#).unwrap()..];
    assert!(effective.contains("<code>80</code>"), "{effective}");
    assert!(
        effective.contains(&format!(r#"href="{web}""#)),
        "{effective}"
    );

    post_form(
        &router,
        &format!("{host}/variables"),
        "form=add&name=port&value=8080",
    )
    .await;
    let page = text(get(&router, &host).await).await;
    let effective = &page[page.find(r#"id="effective""#).unwrap()..];
    assert!(effective.contains("<code>8080</code>"), "{effective}");
    assert!(effective.contains("overrides"), "{effective}");
}

/// Creates a Service through the form and returns its page's path.
async fn create_service(router: &Router, name: &str) -> String {
    let created = post_form(router, "/services/new", &format!("name={name}")).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    location(&created)
}

#[tokio::test]
async fn manages_services_their_parents_and_inherited_variables() {
    let (_db, router) = app().await;
    let mysql = create_service(&router, "mysql").await;
    let mariadb = create_service(&router, "mariadb").await;

    let nested = post_form(
        &router,
        &format!("{mariadb}/parent"),
        &format!("parent={}", id_of(&mysql)),
    )
    .await;
    assert_eq!(nested.status(), StatusCode::SEE_OTHER);
    let cycle = post_form(
        &router,
        &format!("{mysql}/parent"),
        &format!("parent={}", id_of(&mariadb)),
    )
    .await;
    assert_eq!(cycle.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(cycle).await.contains("mysql → mariadb → mysql"));
    let list = text(get(&router, "/services").await).await;
    assert!(
        list.find(">mysql<").unwrap() < list.find(">mariadb<").unwrap(),
        "{list}"
    );

    let set = post_form(
        &router,
        &format!("{mysql}/variables"),
        "form=add&name=port&value=3306",
    )
    .await;
    assert_eq!(set.status(), StatusCode::SEE_OTHER);
    let page = text(get(&router, &mariadb).await).await;
    let effective = &page[page.find(r#"id="effective""#).unwrap()..];
    assert!(effective.contains("<code>3306</code>"), "{effective}");
    assert!(
        effective.contains(&format!(r#"href="{mysql}""#)),
        "{effective}"
    );

    let confirm = text(get(&router, &format!("{mysql}/delete")).await).await;
    assert!(confirm.contains(">mariadb<"), "{confirm}");
    assert!(confirm.contains(">port<"), "{confirm}");
    let deleted = post_form(&router, &format!("{mysql}/delete"), "").await;
    assert_eq!(location(&deleted), "/services");
    assert_eq!(get(&router, &mysql).await.status(), StatusCode::NOT_FOUND);
    assert!(
        text(get(&router, &mariadb).await)
            .await
            .contains("A top-level Service.")
    );
}

#[tokio::test]
async fn manages_service_instances_from_host_and_service_pages() {
    let (_db, router) = app().await;
    let host = create_host(&router, "web1").await;
    let web2 = create_host(&router, "web2").await;
    let nginx = create_service(&router, "nginx").await;

    // From the Host page: a bad IP is rejected with the input kept.
    let rejected = post_form(
        &router,
        &format!("{host}/instances"),
        &format!("service={}&name=main&ip=192.0.2.999", id_of(&nginx)),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = text(rejected).await;
    assert!(page.contains("is not an IP address"), "{page}");
    assert!(page.contains(r#"value="192.0.2.999""#), "{page}");

    let added = post_form(
        &router,
        &format!("{host}/instances"),
        &format!("service={}&name=main&ip=192.0.2.10", id_of(&nginx)),
    )
    .await;
    assert_eq!(added.status(), StatusCode::SEE_OTHER);
    // From the Service page, on another Host.
    let added = post_form(
        &router,
        &format!("{nginx}/instances"),
        &format!("host={}&name=main&ip=2001:db8::20", id_of(&web2)),
    )
    .await;
    assert_eq!(added.status(), StatusCode::SEE_OTHER);
    // A duplicate (Service, Host, Instance Name) is rejected.
    let duplicate = post_form(
        &router,
        &format!("{nginx}/instances"),
        &format!("host={}&name=main&ip=192.0.2.11", id_of(&host)),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        text(duplicate)
            .await
            .contains("web1 already has a Service Instance of nginx named main")
    );

    let service_page = text(get(&router, &nginx).await).await;
    assert!(
        service_page.contains("<code>2001:db8::20</code>"),
        "{service_page}"
    );
    let host_page = text(get(&router, &host).await).await;
    let start = host_page.find("/instances/").expect("an instance link");
    let instance = host_page[start..].split('"').next().unwrap().to_string();

    // Edit it, give it a Variable, see it inherit from its Service.
    post_form(
        &router,
        &format!("{nginx}/variables"),
        "form=add&name=port&value=80",
    )
    .await;
    let edited = post_form(&router, &instance, "name=primary&ip=192.0.2.12").await;
    assert_eq!(edited.status(), StatusCode::SEE_OTHER);
    post_form(
        &router,
        &format!("{instance}/variables"),
        "form=add&name=workers&value=4",
    )
    .await;
    let page = text(get(&router, &instance).await).await;
    assert!(page.contains(r#"value="primary""#), "{page}");
    assert!(page.contains("<code>80</code>"), "inherited port: {page}");
    assert!(page.contains(&format!(r#"href="{nginx}""#)), "{page}");

    // Deleting the Host lists the Service Instance, then removes it.
    let confirm = text(get(&router, &format!("{host}/delete")).await).await;
    assert!(confirm.contains("nginx primary on web1"), "{confirm}");
    assert!(confirm.contains("workers"), "{confirm}");
    post_form(&router, &format!("{host}/delete"), "").await;
    assert_eq!(
        get(&router, &instance).await.status(),
        StatusCode::NOT_FOUND
    );
    assert!(
        !text(get(&router, &nginx).await)
            .await
            .contains("192.0.2.12")
    );
}

#[tokio::test]
async fn a_service_instance_whose_host_is_gone_can_still_be_deleted() {
    let (db, router) = app().await;
    let nginx = create_service(&router, "nginx").await;
    // cli/si's host delete leaves the Service Instance behind (ADR 0003).
    common::sql(
        &db,
        &format!(
            "INSERT INTO service_instance(id, service, host, ip, name) VALUES (50, {}, 999, '192.0.2.50', 'stale');",
            id_of(&nginx)
        ),
    );

    let page = text(get(&router, &nginx).await).await;
    assert!(page.contains("(Host 999, missing)"), "{page}");
    assert!(
        !page.contains(r#"href="/hosts/999""#),
        "no link to a missing Host: {page}"
    );

    let deleted = post_form(&router, "/instances/50/delete", "").await;
    assert_eq!(deleted.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&deleted), format!("{nginx}#instances"));
}

#[tokio::test]
async fn clones_a_service_instance_from_the_host_page() {
    let (_db, router) = app().await;
    let host = create_host(&router, "web1").await;
    let web2 = create_host(&router, "web2").await;
    let nginx = create_service(&router, "nginx").await;
    post_form(
        &router,
        &format!("{host}/instances"),
        &format!("service={}&name=main&ip=192.0.2.10", id_of(&nginx)),
    )
    .await;
    let host_page = text(get(&router, &host).await).await;
    let start = host_page.find(r#"href="/instances/"#).unwrap() + r#"href=""#.len();
    let instance = host_page[start..].split('"').next().unwrap().to_string();
    assert!(
        host_page.contains(&format!(r#"href="{instance}/clone""#)),
        "{host_page}"
    );
    post_form(
        &router,
        &format!("{instance}/variables"),
        "form=add&name=workers&value=4",
    )
    .await;

    let form = text(get(&router, &format!("{instance}/clone")).await).await;
    assert!(form.contains(">web2<"), "{form}");
    let rejected = post_form(
        &router,
        &format!("{instance}/clone"),
        &format!("host={}&ip=not-an-ip", id_of(&web2)),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(rejected).await.contains(r#"value="not-an-ip""#));

    let cloned = post_form(
        &router,
        &format!("{instance}/clone"),
        &format!("host={}&ip=192.0.2.20", id_of(&web2)),
    )
    .await;
    assert_eq!(cloned.status(), StatusCode::SEE_OTHER);
    let clone_page = text(get(&router, &location(&cloned)).await).await;
    assert!(clone_page.contains("nginx main on web2"), "{clone_page}");
    assert!(clone_page.contains(r#"value="192.0.2.20""#), "{clone_page}");
    assert!(
        clone_page.contains(r#"<th scope="row">workers</th>"#),
        "{clone_page}"
    );

    let again = post_form(
        &router,
        &format!("{instance}/clone"),
        &format!("host={}&ip=192.0.2.21", id_of(&web2)),
    )
    .await;
    assert_eq!(again.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        text(again)
            .await
            .contains("web2 already has a Service Instance of nginx named main")
    );
}

#[tokio::test]
async fn the_variables_page_lists_every_usage_and_entity_pages_link_to_it() {
    let (_db, router) = app().await;
    let host = create_host(&router, "web1").await;
    let web = create_group(&router, "web").await;
    post_form(
        &router,
        &format!("{host}/variables"),
        "form=add&name=ntp+server&value=%22a%22",
    )
    .await;
    post_form(
        &router,
        &format!("{web}/variables"),
        "form=add&name=ntp+server&value=%22b%22",
    )
    .await;
    post_form(
        &router,
        &format!("{web}/variables"),
        "form=add&name=gone&value=1",
    )
    .await;
    post_form(&router, &format!("{web}/variables/remove"), "name=gone").await;

    let page = text(get(&router, "/variables").await).await;
    let ntp = &page[page.find(">ntp server<").expect("ntp server listed")..];
    assert!(ntp.contains(&format!(r#"href="{host}""#)), "{page}");
    assert!(ntp.contains(&format!(r#"href="{web}""#)), "{page}");
    assert!(
        ntp.contains("<code>&quot;a&quot;</code>") || ntp.contains(r#"<code>"a"</code>"#),
        "{page}"
    );
    let gone = &page[page.find(">gone<").expect("gone listed")..];
    assert!(
        gone[..gone.find("</section>").unwrap()].contains("Unused"),
        "{page}"
    );

    // The entry's id is the hex-encoded name, and the Host page links its
    // Own Variable back to it.
    assert!(
        page.contains(r#"<section id="variable-6e747020736572766572">"#),
        "{page}"
    );
    let host_page = text(get(&router, &host).await).await;
    assert!(
        host_page.contains(r#"href="/variables#variable-6e747020736572766572""#),
        "{host_page}"
    );
}

#[tokio::test]
async fn pages_link_the_stylesheet_and_it_is_served() {
    let (db, _) = app().await;
    let root = webroot_with(&[("css/index.css", "body { margin: 0; }")]);
    let router = lager::web::router(open(&db).await, root.path());

    let page = text(get(&router, "/hosts").await).await;
    assert!(
        page.contains(r#"<link rel="stylesheet" href="/css/index.css">"#),
        "{page}"
    );

    let css = get(&router, "/css/index.css").await;
    assert_eq!(css.status(), StatusCode::OK);
    assert!(
        css.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/css"),
        "{:?}",
        css.headers()
    );
}

#[tokio::test]
async fn serves_css_from_webroot_css_and_images_from_webroot_img() {
    let (db, _) = app().await;
    let root = webroot_with(&[
        ("css/themes/dark.css", "body { color: #eee; }"),
        (
            "img/logo.svg",
            r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#,
        ),
        ("secret.txt", "not served"),
    ]);
    let router = lager::web::router(open(&db).await, root.path());

    let css = get(&router, "/css/themes/dark.css").await;
    assert_eq!(css.status(), StatusCode::OK);
    assert_eq!(text(css).await, "body { color: #eee; }");
    let svg = get(&router, "/img/logo.svg").await;
    assert_eq!(svg.status(), StatusCode::OK);
    assert!(
        svg.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("image/svg+xml")
    );

    for missing in [
        "/css/nope.css",
        "/img/nope.png",
        "/css/../secret.txt",
        "/img/..%2Fsecret.txt",
    ] {
        assert_eq!(
            get(&router, missing).await.status(),
            StatusCode::NOT_FOUND,
            "{missing}"
        );
    }
}
