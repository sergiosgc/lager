//! The web app: server-rendered pages over the Inventory module.
//!
//! Handlers only translate HTTP into calls on [`Inventory`] and its results
//! into pages. Forms post, then redirect to the detail page on success, or
//! re-render with the error and the submitted input.

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Router, Slot, StatusCode,
        content::Form,
        error::{RouterErrorExt, SeeOther, see_other},
        href, layout, page, path_param, route,
    },
    view::{View, component, view},
};

use crate::inventory::{self, Inventory};

/// Builds the router serving `inventory`.
pub fn router(inventory: Inventory) -> Router {
    Router::builder()
        .app_context(inventory)
        .layout(root_layout)
        .route(home)
        .page(hosts_list)
        .page(new_host_form)
        .page(create_host)
        .page(host_detail)
        .build()
}

fn inventory(cx: &Cx) -> &Inventory {
    app_context::<Inventory>(cx)
}

const STYLE: &str = "
body { font-family: system-ui, sans-serif; max-width: 48rem; margin: 0 auto; padding: 0 1rem; }
nav { display: flex; gap: 1rem; padding: 1rem 0; border-bottom: 1px solid #ccc; }
.error { color: #b00020; }
label { display: block; margin-bottom: .25rem; }
";

#[layout("/")]
async fn root_layout(slot: Slot<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Lager"</title>
                <style>(STYLE)</style>
            </head>
            <body>
                <nav>
                    <strong>"Lager"</strong>
                    <a href=(href!(hosts_list))>"Hosts"</a>
                </nav>
                <main>(slot)</main>
            </body>
        </html>
    })
}

#[route(GET "/")]
async fn home(cx: &Cx) -> Result<SeeOther> {
    Ok(see_other(href!(hosts_list).resolve(cx)))
}

#[page("/hosts")]
async fn hosts_list(cx: &Cx) -> Result<impl View> {
    let hosts = inventory(cx).hosts().await?;
    Ok(view! {
        <h1>"Hosts"</h1>
        <p><a href=(href!(new_host_form))>"New Host"</a></p>
        if hosts.is_empty() {
            <p>"There are no Hosts yet."</p>
        } else {
            <ul>
                for host in hosts {
                    <li><a href=(href!(host_detail, HostId(host.id)))>(host.name)</a></li>
                }
            </ul>
        }
    })
}

#[derive(Deserialize)]
struct NewHost {
    name: String,
}

#[page("/hosts/new")]
async fn new_host_form() -> Result<impl View> {
    Ok(view! { new_host_view(name: String::new(), error: None) })
}

#[page(POST "/hosts/new")]
async fn create_host(cx: &Cx, Form(input): Form<NewHost>) -> Result<impl View> {
    match inventory(cx).create_host(&input.name).await {
        Ok(done) => Err(see_other(href!(host_detail, HostId(done.value.id)).resolve(cx)).into()),
        Err(inventory::Error::Invalid { message, .. }) => {
            Ok(view! { new_host_view(name: input.name, error: Some(message)) })
        }
        Err(e) => Err(e.into()),
    }
}

/// The new-Host form, showing `name` as typed and the error it was rejected
/// with, if any.
#[component]
async fn new_host_view(name: String, error: Option<String>) -> Result<impl View> {
    Ok(view! {
        if error.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <h1>"New Host"</h1>
        <form method="post" action=(href!(create_host))>
            <label for="name">"Name"</label>
            <input id="name" name="name" value=(name) required=(true) autofocus=(true)
                if error.is_some() {
                    aria-invalid="true"
                    aria-describedby="name-error"
                }
            >
            if let Some(error) = error {
                <p id="name-error" class="error">(error)</p>
            }
            <p><button type="submit">"Create Host"</button></p>
        </form>
    })
}

topcoat::router::path_param!(host_id: i64, error = not_found);

#[page("/hosts/{host_id}")]
async fn host_detail(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<HostId>(cx)?;
    let host = inventory(cx).host(id).await?.ok_or_not_found()?;
    Ok(view! {
        <p><a href=(href!(hosts_list))>"All Hosts"</a></p>
        <h1>(host.name)</h1>
    })
}
