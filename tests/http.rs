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
