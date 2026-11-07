//! The web app: server-rendered pages over the Inventory module.
//!
//! Handlers only translate HTTP into calls on [`Inventory`] and its results
//! into pages. Forms post, then redirect to the detail page on success, or
//! re-render with the error and the submitted input.

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Router, RouterBuilderDirectoryExt, Slot,
        error::{SeeOther, not_found, see_other},
        href, layout, route,
    },
    view::{View, view},
};

mod groups;
mod hosts;
mod service_instances;
mod services;
mod variables;

use std::path::Path;

use crate::inventory::{self, Inventory};

/// Builds the router serving `inventory`.
pub fn router(inventory: Inventory, webroot: &Path) -> Router {
    Router::builder()
        .app_context(inventory)
        // Read from disk on every request, so stylesheet and image edits
        // show without rebuilding. Paths with `..` and missing files are
        // 404s.
        .serve_dir("/css/{*file}", webroot.join("css"))
        .serve_dir("/img/{*file}", webroot.join("img"))
        .serve_dir("/fonts/{*file}", webroot.join("fonts"))
        .layout(root_layout)
        .route(home)
        .page(hosts::hosts_list)
        .page(hosts::new_host_form)
        .page(hosts::create_host)
        .page(hosts::host_detail)
        .page(hosts::set_host_variable)
        .page(hosts::remove_host_variable)
        .page(hosts::rename_host_form)
        .page(hosts::rename_host)
        .page(hosts::clone_host_form)
        .page(hosts::clone_host)
        .page(hosts::delete_host_confirmation)
        .page(hosts::delete_host)
        .page(hosts::add_host_group)
        .page(hosts::remove_host_group)
        .page(hosts::set_host_groups)
        .page(groups::groups_list)
        .page(groups::new_group_form)
        .page(groups::create_group)
        .page(groups::group_detail)
        .page(groups::rename_group_form)
        .page(groups::rename_group)
        .page(groups::set_group_parent)
        .page(groups::add_group_host)
        .page(groups::remove_group_host)
        .page(groups::delete_group_confirmation)
        .page(groups::delete_group)
        .page(groups::set_group_variable)
        .page(groups::remove_group_variable)
        .page(services::services_list)
        .page(services::new_service_form)
        .page(services::create_service)
        .page(services::service_detail)
        .page(services::rename_service_form)
        .page(services::rename_service)
        .page(services::set_service_parent)
        .page(services::set_service_variable)
        .page(services::remove_service_variable)
        .page(services::delete_service_confirmation)
        .page(services::delete_service)
        .page(services::add_service_instance)
        .page(hosts::add_host_instance)
        .page(service_instances::instance_detail)
        .page(service_instances::update_instance)
        .page(service_instances::set_instance_variable)
        .page(service_instances::remove_instance_variable)
        .page(service_instances::delete_instance_confirmation)
        .page(service_instances::delete_instance)
        .page(service_instances::clone_instance_form)
        .page(service_instances::clone_instance)
        .page(variables::variables_list)
        .build()
}

pub(crate) fn inventory(cx: &Cx) -> &Inventory {
    app_context::<Inventory>(cx)
}

#[layout("/")]
async fn root_layout(slot: Slot<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Lager"</title>
                <link rel="stylesheet" href="/css/index.css">
            </head>
            <body>
                <nav>
                    <strong>"Lager"</strong>
                    <a href=(href!(hosts::hosts_list))>"Hosts"</a>
                    <a href=(href!(groups::groups_list))>"Groups"</a>
                    <a href=(href!(services::services_list))>"Services"</a>
                    <a href=(href!(variables::variables_list))>"Variables"</a>
                </nav>
                <main>(slot)</main>
            </body>
        </html>
    })
}

#[route(GET "/")]
async fn home(cx: &Cx) -> Result<SeeOther> {
    Ok(see_other(href!(hosts::hosts_list).resolve(cx)))
}

/// A rejected input: which field it was and why.
pub(crate) struct FieldError {
    pub(crate) field: &'static str,
    pub(crate) message: String,
}

impl FieldError {
    /// The message, if this error is about `field`.
    pub(crate) fn on(&self, field: &str) -> Option<String> {
        (self.field == field).then(|| self.message.clone())
    }
}

/// Turns an Inventory error a page can't show in a form into a response:
/// a 404 when the entity is gone, otherwise a 500.
pub(crate) fn page_error(e: inventory::Error) -> topcoat::Error {
    match e {
        inventory::Error::NotFound(_) => not_found().into(),
        e => e.into(),
    }
}
