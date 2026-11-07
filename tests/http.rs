//! Seam 2: the web app, driven with HTTP requests against a temporary
//! database. Kept deliberately small so Topcoat's breaking changes touch few
//! tests; the rules themselves are covered at the Inventory seam.

mod common;

use common::{open, temp_db};
use topcoat::router::{
    Body, Router, StatusCode, header, request::Request, response::Response, to_bytes,
};

async fn app() -> (common::TempDb, Router) {
    let db = temp_db();
    let router = lager::web::router(open(&db).await);
    (db, router)
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

#[tokio::test]
async fn a_host_in_a_group_cannot_be_deleted_yet() {
    let (db, router) = app().await;
    let host = create_host(&router, "web1").await;
    let conn = rusqlite::Connection::open(&db.path).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO host_group(id, name) VALUES (1, 'eu');
         INSERT INTO host_group_membership(host, \"group\") SELECT id, 1 FROM host;",
    )
    .unwrap();

    let confirm = text(get(&router, &format!("{host}/delete")).await).await;
    assert!(confirm.contains("member of 1 Group"), "{confirm}");
    assert!(!confirm.contains("<button"), "no delete button: {confirm}");

    let refused = post_form(&router, &format!("{host}/delete"), "").await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(get(&router, &host).await.status(), StatusCode::OK);
}
