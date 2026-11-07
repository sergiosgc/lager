//! Host pages: the list, a Host's page with its Own Variables, and the
//! create, rename and delete forms.

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

use super::groups::{GroupId, group_detail};
use super::service_instances::{
    FailedInstance, InstanceFields, add_instance_form, host_instances_list,
    removed_instances_section,
};
use super::variables::{
    FailedVariable, RemoveVariable, VariableInput, effective_variables_section,
    own_variables_section, removed_variables_section,
};
use super::{FieldError, inventory, page_error};
use crate::inventory::{
    EffectiveVariable, Group, Host, HostDeletion, OwnVariable, Owner, Service, ServiceInstance,
};

#[page("/hosts")]
pub(super) async fn hosts_list(cx: &Cx) -> Result<impl View> {
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
pub(super) struct NewHost {
    name: String,
    /// Blank or missing means "assign the next Server Index automatically".
    #[serde(default)]
    server_index: String,
}

#[page("/hosts/new")]
pub(super) async fn new_host_form(cx: &Cx) -> Result<impl View> {
    let suggested = inventory(cx).next_server_index().await?;
    Ok(
        view! { new_host_view(name: String::new(), server_index: suggested.to_string(), error: None) },
    )
}

#[page(POST "/hosts/new")]
pub(super) async fn create_host(cx: &Cx, Form(input): Form<NewHost>) -> Result<impl View> {
    match inventory(cx)
        .create_host(&input.name, Some(&input.server_index))
        .await
    {
        Ok(done) => Err(see_other(href!(host_detail, HostId(done.value.id)).resolve(cx)).into()),
        Err(inventory::Error::Invalid { field, message }) => Ok(view! {
            new_host_view(
                name: input.name,
                server_index: input.server_index,
                error: Some(FieldError { field, message }),
            )
        }),
        Err(e) => Err(page_error(e)),
    }
}

/// The new-Host form, showing the input as typed and the error it was
/// rejected with, if any.
#[component]
async fn new_host_view(
    name: String,
    server_index: String,
    error: Option<FieldError>,
) -> Result<impl View> {
    let field_error = |field: &str| error.as_ref().and_then(|e| e.on(field));
    let (name_error, index_error) = (field_error("name"), field_error(inventory::SERVER_INDEX));
    Ok(view! {
        if error.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <h1>"New Host"</h1>
        <form method="post" action=(href!(create_host))>
            <label for="name">"Name"</label>
            <input id="name" name="name" value=(name) required=(true) autofocus=(true)
                if name_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="name-hint name-error"
                } else {
                    aria-describedby="name-hint"
                }
            >
            <p id="name-hint" class="hint">
                "Saved in lowercase, keeping only letters a–z, digits, \"-\" and \"_\"."
            </p>
            if let Some(error) = name_error {
                <p id="name-error" class="error">(error)</p>
            }
            <label for="server_index">"Server Index"</label>
            <input id="server_index" name="server_index" value=(server_index)
                if index_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="server_index-hint server_index-error"
                } else {
                    aria-describedby="server_index-hint"
                }
            >
            <p id="server_index-hint" class="hint">
                "Suggested: one more than the highest Server Index in use. Any JSON value is accepted; leave blank to use the next number."
            </p>
            if let Some(error) = index_error {
                <p id="server_index-error" class="error">(error)</p>
            }
            <p><button type="submit">"Create Host"</button></p>
        </form>
    })
}

topcoat::router::path_param!(pub(super) host_id: i64, error = not_found);

#[page("/hosts/{host_id}")]
pub(super) async fn host_detail(cx: &Cx) -> Result<impl View> {
    let host = path_host(cx).await?;
    let page = load_host_page(cx, host).await?;
    Ok(view! { host_view(page: page, failed: None, failed_instance: None) })
}

/// Everything a Host's page shows.
pub(super) struct HostPage {
    host: Host,
    variables: Vec<OwnVariable>,
    effective: Vec<EffectiveVariable>,
    warnings: Vec<String>,
    /// The Groups the Host is directly in.
    groups: Vec<Group>,
    /// Every Group, for adding the Host to one or replacing its set.
    all_groups: Vec<Group>,
    instances: Vec<ServiceInstance>,
    /// Every Service, for adding a Service Instance.
    services: Vec<Service>,
}

async fn load_host_page(cx: &Cx, host: Host) -> Result<HostPage> {
    let inventory = inventory(cx);
    Ok(HostPage {
        variables: inventory.own_variables(Owner::Host(host.id)).await?,
        effective: inventory.effective_variables(Owner::Host(host.id)).await?,
        warnings: inventory.host_warnings(host.id).await?,
        groups: inventory.host_groups(host.id).await?,
        all_groups: inventory.groups().await?,
        instances: inventory.instances_on_host(host.id).await?,
        services: inventory.services().await?,
        host,
    })
}

/// The Host named by the `{host_id}` path parameter, or a 404.
async fn path_host(cx: &Cx) -> Result<Host> {
    let id = *path_param::<HostId>(cx)?;
    Ok(inventory(cx).host(id).await?.ok_or_not_found()?)
}

#[page(POST "/hosts/{host_id}/variables")]
pub(super) async fn set_host_variable(
    cx: &Cx,
    Form(input): Form<VariableInput>,
) -> Result<impl View> {
    let host = path_host(cx).await?;
    match inventory(cx)
        .set_variable(Owner::Host(host.id), &input.name, &input.value)
        .await
    {
        Ok(_) => Err(see_other(host_variables_url(cx, host.id)).into()),
        Err(inventory::Error::Invalid { field, message }) => {
            let page = load_host_page(cx, host).await?;
            let failed = FailedVariable {
                input,
                field,
                message,
            };
            Ok(view! { host_view(page: page, failed: Some(failed), failed_instance: None) })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[page(POST "/hosts/{host_id}/variables/remove")]
pub(super) async fn remove_host_variable(
    cx: &Cx,
    Form(input): Form<RemoveVariable>,
) -> Result<impl View> {
    let host = path_host(cx).await?;
    inventory(cx)
        .remove_variable(Owner::Host(host.id), &input.name)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(host_variables_url(cx, host.id)).into())
}

fn host_variables_url(cx: &Cx, host_id: i64) -> String {
    host_url(cx, host_id, "variables")
}

fn host_url(cx: &Cx, host_id: i64, section: &str) -> String {
    format!(
        "{}#{section}",
        href!(host_detail, HostId(host_id)).resolve(cx)
    )
}

#[derive(Deserialize)]
pub(super) struct HostInstanceInput {
    service: i64,
    #[serde(flatten)]
    fields: InstanceFields,
}

#[page(POST "/hosts/{host_id}/instances")]
pub(super) async fn add_host_instance(
    cx: &Cx,
    Form(input): Form<HostInstanceInput>,
) -> Result<impl View> {
    let host = path_host(cx).await?;
    match inventory(cx)
        .create_service_instance(host.id, input.service, &input.fields.name, &input.fields.ip)
        .await
    {
        Ok(_) => Err(see_other(host_url(cx, host.id, "instances")).into()),
        Err(inventory::Error::Invalid { field, message }) => {
            let page = load_host_page(cx, host).await?;
            let failed_instance = FailedInstance {
                choice: Some(input.service),
                fields: input.fields,
                error: FieldError { field, message },
            };
            Ok(
                view! { host_view(page: page, failed: None, failed_instance: Some(failed_instance)) },
            )
        }
        Err(e) => Err(page_error(e)),
    }
}

#[derive(Deserialize)]
pub(super) struct GroupChoice {
    group: i64,
}

#[page(POST "/hosts/{host_id}/groups")]
pub(super) async fn add_host_group(cx: &Cx, Form(input): Form<GroupChoice>) -> Result<impl View> {
    let host = path_host(cx).await?;
    inventory(cx)
        .add_host_to_group(host.id, input.group)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(host_url(cx, host.id, "groups")).into())
}

#[page(POST "/hosts/{host_id}/groups/remove")]
pub(super) async fn remove_host_group(
    cx: &Cx,
    Form(input): Form<GroupChoice>,
) -> Result<impl View> {
    let host = path_host(cx).await?;
    inventory(cx)
        .remove_host_from_group(host.id, input.group)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(host_url(cx, host.id, "groups")).into())
}

/// Replaces the Host's whole set of Groups with the checked ones. The form
/// repeats the `group` key once per checked box, so the body is read as
/// pairs.
#[page(POST "/hosts/{host_id}/groups/set")]
pub(super) async fn set_host_groups(
    cx: &Cx,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Result<impl View> {
    let host = path_host(cx).await?;
    let mut group_ids = Vec::new();
    for (key, value) in pairs {
        if key == "group" {
            group_ids.push(
                value
                    .parse::<i64>()
                    .map_err(|_| topcoat::router::error::bad_request("group must be a Group id"))?,
            );
        }
    }
    inventory(cx)
        .set_host_groups(host.id, &group_ids)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(host_url(cx, host.id, "groups")).into())
}

/// A Host's page: its Groups, Own Variables and Effective Variables. A
/// rejected Variable submission is shown again with its error.
#[component]
async fn host_view(
    cx: &Cx,
    page: HostPage,
    failed: Option<FailedVariable>,
    failed_instance: Option<FailedInstance>,
) -> Result<impl View> {
    let HostPage {
        host,
        variables,
        effective,
        warnings,
        groups,
        all_groups,
        instances,
        services,
    } = page;
    let host_id = host.id;
    let addable: Vec<Group> = all_groups
        .iter()
        .filter(|g| !groups.iter().any(|m| m.id == g.id))
        .cloned()
        .collect();
    let status_failed = failed.is_some() || failed_instance.is_some();
    Ok(view! {
        if status_failed {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <p><a href=(href!(hosts_list))>"All Hosts"</a></p>
        <h1>(host.name.clone())</h1>
        <p class="actions">
            <a href=(href!(rename_host_form, HostId(host_id)))>"Rename"</a>
            <a href=(href!(delete_host_confirmation, HostId(host_id)))>"Delete…"</a>
        </p>
        if !warnings.is_empty() {
            <ul class="warnings" role="status">
                for warning in warnings {
                    <li>(warning)</li>
                }
            </ul>
        }

        <h2 id="groups">"Groups"</h2>
        if groups.is_empty() {
            <p>"This Host isn't directly in any Group."</p>
        } else {
            <ul class="items">
                #[key(group.id)]
                for group in groups.clone() {
                    <li>
                        <a href=(href!(group_detail, GroupId(group.id)))>(group.name.clone())</a>
                        <form method="post" action=(href!(remove_host_group, HostId(host_id)))>
                            <input type="hidden" name="group" value=(group.id)>
                            <button type="submit"
                                aria-label=(format!("Remove {} from {}", host.name, group.name))
                            >"Remove"</button>
                        </form>
                    </li>
                }
            </ul>
        }
        if !addable.is_empty() {
            <form method="post" action=(href!(add_host_group, HostId(host_id))) class="inline">
                <label for="group">"Add to a Group"</label>
                <select id="group" name="group">
                    #[key(group.id)]
                    for group in addable {
                        <option value=(group.id)>(group.name)</option>
                    }
                </select>
                <button type="submit">"Add"</button>
            </form>
        }
        if !all_groups.is_empty() {
            <details>
                <summary>"Change all Groups at once"</summary>
                <form method="post" action=(href!(set_host_groups, HostId(host_id)))>
                    <fieldset>
                        <legend>"Groups " (host.name.clone()) " is directly in"</legend>
                        #[key(group.id)]
                        for group in all_groups {
                            <label class="check">
                                <input type="checkbox" name="group" value=(group.id)
                                    if groups.iter().any(|m| m.id == group.id) {
                                        checked=(true)
                                    }
                                >
                                (group.name)
                            </label>
                        }
                    </fieldset>
                    <p><button type="submit">"Save Groups"</button></p>
                </form>
            </details>
        }

        <h2 id="instances">"Service Instances"</h2>
        host_instances_list(instances: instances)
        add_instance_form(
            action: href!(add_host_instance, HostId(host_id)).resolve(cx),
            choice_name: "service",
            choice_label: "Service",
            choices: services.into_iter().map(|s| (s.id, s.name)).collect(),
            failed: failed_instance,
        )

        own_variables_section(
            set_url: href!(set_host_variable, HostId(host_id)).resolve(cx),
            remove_url: href!(remove_host_variable, HostId(host_id)).resolve(cx),
            variables: variables,
            failed: failed,
            empty: "This Host has no Own Variables.".to_string(),
        )
        effective_variables_section(effective: effective)
    })
}

#[derive(Deserialize)]
pub(super) struct RenameHost {
    name: String,
}

#[page("/hosts/{host_id}/rename")]
pub(super) async fn rename_host_form(cx: &Cx) -> Result<impl View> {
    let host = path_host(cx).await?;
    let name = host.name.clone();
    Ok(view! { rename_host_view(host: host, name: name, error: None) })
}

#[page(POST "/hosts/{host_id}/rename")]
pub(super) async fn rename_host(cx: &Cx, Form(input): Form<RenameHost>) -> Result<impl View> {
    let host = path_host(cx).await?;
    match inventory(cx).rename_host(host.id, &input.name).await {
        Ok(_) => Err(see_other(href!(host_detail, HostId(host.id)).resolve(cx)).into()),
        Err(inventory::Error::Invalid { message, .. }) => {
            Ok(view! { rename_host_view(host: host, name: input.name, error: Some(message)) })
        }
        Err(e) => Err(page_error(e)),
    }
}

/// The rename form for `host`, showing `name` as typed and the error it was
/// rejected with, if any.
#[component]
async fn rename_host_view(host: Host, name: String, error: Option<String>) -> Result<impl View> {
    Ok(view! {
        if error.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <h1>"Rename " (host.name)</h1>
        <form method="post" action=(href!(rename_host, HostId(host.id)))>
            <label for="name">"New name"</label>
            <input id="name" name="name" value=(name) required=(true) autofocus=(true)
                if error.is_some() {
                    aria-invalid="true"
                    aria-describedby="name-hint name-error"
                } else {
                    aria-describedby="name-hint"
                }
            >
            <p id="name-hint" class="hint">
                "Saved in lowercase, keeping only letters a–z, digits, \"-\" and \"_\". Everything attached to the Host stays attached."
            </p>
            if let Some(error) = error {
                <p id="name-error" class="error">(error)</p>
            }
            <p class="actions">
                <button type="submit">"Rename"</button>
                <a href=(href!(host_detail, HostId(host.id)))>"Cancel"</a>
            </p>
        </form>
    })
}

#[page("/hosts/{host_id}/delete")]
pub(super) async fn delete_host_confirmation(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<HostId>(cx)?;
    let deletion = inventory(cx).host_deletion(id).await.map_err(page_error)?;
    Ok(view! { delete_host_view(deletion: deletion) })
}

#[page(POST "/hosts/{host_id}/delete")]
pub(super) async fn delete_host(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<HostId>(cx)?;
    inventory(cx).delete_host(id).await.map_err(page_error)?;
    Err::<(), _>(see_other(href!(hosts_list).resolve(cx)).into())
}

/// The delete confirmation: everything that will be removed.
#[component]
async fn delete_host_view(deletion: HostDeletion) -> Result<impl View> {
    let HostDeletion {
        host,
        own_variables,
        groups,
        missing_memberships,
        service_instances,
    } = deletion;
    let host_id = host.id;
    Ok(view! {
        <h1>"Delete " (host.name.clone())</h1>
        <p>"This removes the Host " <strong>(host.name.clone())</strong> " and:"</p>
        removed_variables_section(variables: own_variables)
        removed_instances_section(instances: service_instances)
        <h2>"Groups it leaves"</h2>
        if groups.is_empty() {
            <p>"None: it isn't directly in any Group."</p>
        } else {
            <p class="hint">"The Groups themselves stay."</p>
            <ul>
                #[key(group.id)]
                for group in groups {
                    <li>(group.name)</li>
                }
            </ul>
        }
        if missing_memberships > 0 {
            <p>"Also removes " (missing_memberships) " membership(s) of Groups that no longer exist."</p>
        }
        <form method="post" action=(href!(delete_host, HostId(host_id)))>
            <p class="actions">
                <button type="submit" class="danger">"Delete " (host.name)</button>
                <a href=(href!(host_detail, HostId(host_id)))>"Cancel"</a>
            </p>
        </form>
    })
}
