//! Service pages: the list showing Parent Services, a Service's page with
//! its Parent Service, child Services and Variables, and the create, rename
//! and delete forms.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        StatusCode,
        content::Form,
        error::{RouterErrorExt, see_other},
        href, page, path_param,
    },
    view::{View, component, view},
};

use super::service_instances::{
    FailedInstance, InstanceFields, add_instance_form, removed_instances_section,
    service_instances_list,
};
use super::variables::{
    FailedVariable, RemoveVariable, VariableInput, effective_variables_section,
    own_variables_section, removed_variables_section,
};
use super::{FieldError, inventory, page_error};
use crate::inventory::{
    EffectiveVariable, Host, OwnVariable, Owner, Service, ServiceDeletion, ServiceInstance,
};

topcoat::router::path_param!(pub(super) service_id: i64, error = not_found);

/// Everything a Service's page shows.
pub(super) struct ServicePage {
    service: Service,
    parent: Option<Service>,
    /// Every other Service, as Parent Service candidates.
    other_services: Vec<Service>,
    children: Vec<Service>,
    variables: Vec<OwnVariable>,
    effective: Vec<EffectiveVariable>,
    instances: Vec<ServiceInstance>,
    /// Every Host, for adding a Service Instance.
    hosts: Vec<Host>,
}

async fn load_service_page(cx: &Cx, service: Service) -> Result<ServicePage> {
    let inventory = inventory(cx);
    let all = inventory.services().await?;
    let parent = all
        .iter()
        .find(|s| Some(s.id) == service.parent_id)
        .cloned();
    Ok(ServicePage {
        other_services: all.into_iter().filter(|s| s.id != service.id).collect(),
        children: inventory.child_services(service.id).await?,
        instances: inventory.instances_of_service(service.id).await?,
        hosts: inventory.hosts().await?,
        variables: inventory.own_variables(Owner::Service(service.id)).await?,
        effective: inventory
            .effective_variables(Owner::Service(service.id))
            .await?,
        parent,
        service,
    })
}

/// The Service named by the `{service_id}` path parameter, or a 404.
async fn path_service(cx: &Cx) -> Result<Service> {
    let id = *path_param::<ServiceId>(cx)?;
    Ok(inventory(cx).service(id).await?.ok_or_not_found()?)
}

fn service_url(cx: &Cx, service_id: i64, section: &str) -> String {
    format!(
        "{}#{section}",
        href!(service_detail, ServiceId(service_id)).resolve(cx)
    )
}

#[page("/services")]
pub(super) async fn services_list(cx: &Cx) -> Result<impl View> {
    let tree = inventory(cx).service_tree().await?;
    Ok(view! {
        <h1>"Services"</h1>
        <p><a href=(href!(new_service_form))>"New Service"</a></p>
        if tree.is_empty() {
            <p>"There are no Services yet."</p>
        } else {
            <ul class="tree">
                for entry in tree {
                    <li style=(format!("margin-left: {}rem", entry.depth * 2))>
                        <a href=(href!(service_detail, ServiceId(entry.service.id)))>(entry.service.name)</a>
                    </li>
                }
            </ul>
        }
    })
}

#[derive(Deserialize)]
pub(super) struct ServiceName {
    name: String,
}

#[page("/services/new")]
pub(super) async fn new_service_form() -> Result<impl View> {
    Ok(view! { service_name_view(service: None, name: String::new(), error: None) })
}

#[page(POST "/services/new")]
pub(super) async fn create_service(cx: &Cx, Form(input): Form<ServiceName>) -> Result<impl View> {
    match inventory(cx).create_service(&input.name).await {
        Ok(done) => {
            Err(see_other(href!(service_detail, ServiceId(done.value.id)).resolve(cx)).into())
        }
        Err(crate::inventory::Error::Invalid { message, .. }) => {
            Ok(view! { service_name_view(service: None, name: input.name, error: Some(message)) })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[page("/services/{service_id}/rename")]
pub(super) async fn rename_service_form(cx: &Cx) -> Result<impl View> {
    let service = path_service(cx).await?;
    let name = service.name.clone();
    Ok(view! { service_name_view(service: Some(service), name: name, error: None) })
}

#[page(POST "/services/{service_id}/rename")]
pub(super) async fn rename_service(cx: &Cx, Form(input): Form<ServiceName>) -> Result<impl View> {
    let service = path_service(cx).await?;
    match inventory(cx).rename_service(service.id, &input.name).await {
        Ok(_) => Err(see_other(href!(service_detail, ServiceId(service.id)).resolve(cx)).into()),
        Err(crate::inventory::Error::Invalid { message, .. }) => Ok(view! {
            service_name_view(service: Some(service), name: input.name, error: Some(message))
        }),
        Err(e) => Err(page_error(e)),
    }
}

/// The create form (no `service`) or the rename form for `service`, showing
/// `name` as typed and the error it was rejected with, if any.
#[component]
async fn service_name_view(
    cx: &Cx,
    service: Option<Service>,
    name: String,
    error: Option<String>,
) -> Result<impl View> {
    Ok(view! {
        if error.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        match &service {
            Some(service) => <h1>"Rename " (service.name.clone())</h1>,
            None => <h1>"New Service"</h1>,
        }
        <form method="post"
            action=(match &service {
                Some(service) => href!(rename_service, ServiceId(service.id)).resolve(cx),
                None => href!(create_service).resolve(cx),
            })
        >
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
            <p class="actions">
                <button type="submit">
                    (if service.is_some() { "Rename" } else { "Create Service" })
                </button>
                <a href=(match &service {
                    Some(service) => href!(service_detail, ServiceId(service.id)).resolve(cx),
                    None => href!(services_list).resolve(cx),
                })>"Cancel"</a>
            </p>
        </form>
    })
}

#[page("/services/{service_id}")]
pub(super) async fn service_detail(cx: &Cx) -> Result<impl View> {
    let service = path_service(cx).await?;
    let page = load_service_page(cx, service).await?;
    Ok(view! { service_view(page: page, parent_error: None, failed: None, failed_instance: None) })
}

#[derive(Deserialize)]
pub(super) struct ParentChoice {
    /// Blank means "no Parent Service".
    parent: Option<i64>,
}

#[page(POST "/services/{service_id}/parent")]
pub(super) async fn set_service_parent(
    cx: &Cx,
    Form(input): Form<ParentChoice>,
) -> Result<impl View> {
    let service = path_service(cx).await?;
    match inventory(cx)
        .set_service_parent(service.id, input.parent)
        .await
    {
        Ok(_) => Err(see_other(service_url(cx, service.id, "parent")).into()),
        Err(crate::inventory::Error::Invalid { message, .. }) => {
            let page = load_service_page(cx, service).await?;
            Ok(view! {
                service_view(page: page, parent_error: Some(message), failed: None, failed_instance: None)
            })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[derive(Deserialize)]
pub(super) struct ServiceInstanceInput {
    host: i64,
    #[serde(flatten)]
    fields: InstanceFields,
}

#[page(POST "/services/{service_id}/instances")]
pub(super) async fn add_service_instance(
    cx: &Cx,
    Form(input): Form<ServiceInstanceInput>,
) -> Result<impl View> {
    let service = path_service(cx).await?;
    match inventory(cx)
        .create_service_instance(input.host, service.id, &input.fields.name, &input.fields.ip)
        .await
    {
        Ok(_) => Err(see_other(service_url(cx, service.id, "instances")).into()),
        Err(crate::inventory::Error::Invalid { field, message }) => {
            let page = load_service_page(cx, service).await?;
            let failed_instance = FailedInstance {
                choice: Some(input.host),
                fields: input.fields,
                error: FieldError { field, message },
            };
            Ok(view! {
                service_view(page: page, parent_error: None, failed: None, failed_instance: Some(failed_instance))
            })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[page(POST "/services/{service_id}/variables")]
pub(super) async fn set_service_variable(
    cx: &Cx,
    Form(input): Form<VariableInput>,
) -> Result<impl View> {
    let service = path_service(cx).await?;
    match inventory(cx)
        .set_variable(Owner::Service(service.id), &input.name, &input.value)
        .await
    {
        Ok(_) => Err(see_other(service_url(cx, service.id, "variables")).into()),
        Err(crate::inventory::Error::Invalid { field, message }) => {
            let page = load_service_page(cx, service).await?;
            let failed = FailedVariable {
                input,
                field,
                message,
            };
            Ok(view! {
                service_view(page: page, parent_error: None, failed: Some(failed), failed_instance: None)
            })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[page(POST "/services/{service_id}/variables/remove")]
pub(super) async fn remove_service_variable(
    cx: &Cx,
    Form(input): Form<RemoveVariable>,
) -> Result<impl View> {
    let service = path_service(cx).await?;
    inventory(cx)
        .remove_variable(Owner::Service(service.id), &input.name)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(service_url(cx, service.id, "variables")).into())
}

/// A Service's page. `parent_error` is a rejected Parent Service change,
/// shown next to its form; `failed` a rejected Variable submission.
#[component]
async fn service_view(
    cx: &Cx,
    page: ServicePage,
    parent_error: Option<String>,
    failed: Option<FailedVariable>,
    failed_instance: Option<FailedInstance>,
) -> Result<impl View> {
    let ServicePage {
        service,
        parent,
        other_services,
        children,
        variables,
        effective,
        instances,
        hosts,
    } = page;
    let service_id = service.id;
    Ok(view! {
        if parent_error.is_some() || failed.is_some() || failed_instance.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <p><a href=(href!(services_list))>"All Services"</a></p>
        <h1>(service.name.clone())</h1>
        <p class="actions">
            <a href=(href!(rename_service_form, ServiceId(service_id)))>"Rename"</a>
            <a href=(href!(delete_service_confirmation, ServiceId(service_id)))>"Delete…"</a>
        </p>

        <h2 id="parent">"Parent Service"</h2>
        <p>
            match &parent {
                Some(parent) => {
                    "Specialises "
                    <a href=(href!(service_detail, ServiceId(parent.id)))>(parent.name.clone())</a>
                    "."
                },
                None => "A top-level Service.",
            }
        </p>
        <form method="post" action=(href!(set_service_parent, ServiceId(service_id)))>
            <label for="parent">"Parent Service"</label>
            <select id="parent" name="parent"
                if parent_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="parent-error"
                }
            >
                <option value="">"(none: top-level)"</option>
                #[key(candidate.id)]
                for candidate in other_services {
                    <option value=(candidate.id)
                        if Some(candidate.id) == service.parent_id {
                            selected=(true)
                        }
                    >(candidate.name)</option>
                }
            </select>
            if let Some(error) = parent_error {
                <p id="parent-error" class="error">(error)</p>
            }
            <p><button type="submit">"Set Parent Service"</button></p>
        </form>

        <h2 id="children">"Child Services"</h2>
        if children.is_empty() {
            <p>"No Service specialises this one."</p>
        } else {
            <ul>
                #[key(child.id)]
                for child in children {
                    <li><a href=(href!(service_detail, ServiceId(child.id)))>(child.name)</a></li>
                }
            </ul>
        }

        <h2 id="instances">"Service Instances"</h2>
        service_instances_list(instances: instances)
        add_instance_form(
            action: href!(add_service_instance, ServiceId(service_id)).resolve(cx),
            choice_name: "host",
            choice_label: "Host",
            choices: hosts.into_iter().map(|h| (h.id, h.name)).collect(),
            failed: failed_instance,
        )

        own_variables_section(
            set_url: href!(set_service_variable, ServiceId(service_id)).resolve(cx),
            remove_url: href!(remove_service_variable, ServiceId(service_id)).resolve(cx),
            variables: variables,
            failed: failed,
            empty: "This Service has no Own Variables.".to_string(),
        )
        effective_variables_section(effective: effective)
    })
}

#[page("/services/{service_id}/delete")]
pub(super) async fn delete_service_confirmation(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<ServiceId>(cx)?;
    let deletion = inventory(cx)
        .service_deletion(id)
        .await
        .map_err(page_error)?;
    Ok(view! { delete_service_view(deletion: deletion) })
}

#[page(POST "/services/{service_id}/delete")]
pub(super) async fn delete_service(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<ServiceId>(cx)?;
    inventory(cx).delete_service(id).await.map_err(page_error)?;
    Err::<(), _>(see_other(href!(services_list).resolve(cx)).into())
}

/// The delete confirmation: everything that will be removed or changed.
#[component]
async fn delete_service_view(deletion: ServiceDeletion) -> Result<impl View> {
    let ServiceDeletion {
        service,
        own_variables,
        child_services,
        service_instances,
    } = deletion;
    let service_id = service.id;
    let name = service.name;
    Ok(view! {
        <h1>"Delete " (name.clone())</h1>
        <p>"This removes the Service " <strong>(name.clone())</strong> "."</p>
        removed_variables_section(variables: own_variables)
        removed_instances_section(instances: service_instances)
        <h2>"Child Services becoming top-level"</h2>
        if child_services.is_empty() {
            <p>"None: no Service specialises it."</p>
        } else {
            <p class="hint">"These Services stay, without a Parent Service."</p>
            <ul>
                #[key(child.id)]
                for child in child_services {
                    <li>(child.name)</li>
                }
            </ul>
        }
        <form method="post" action=(href!(delete_service, ServiceId(service_id)))>
            <p class="actions">
                <button type="submit" class="danger">"Delete " (name.clone())</button>
                <a href=(href!(service_detail, ServiceId(service_id)))>"Cancel"</a>
            </p>
        </form>
    })
}
