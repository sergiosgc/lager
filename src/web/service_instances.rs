//! Service Instance pages: a Service Instance's page (Instance Name, IP,
//! Own and Effective Variables) and its delete confirmation, plus the
//! pieces Host and Service pages use to list and add Service Instances.

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

use super::hosts::{HostId, host_detail};
use super::services::{ServiceId, service_detail};
use super::variables::{
    FailedVariable, RemoveVariable, VariableInput, effective_variables_section,
    own_variables_section, removed_variables_section,
};
use super::{FieldError, inventory, page_error};
use crate::inventory::{
    EffectiveVariable, Host, OwnVariable, Owner, ServiceInstance, ServiceInstanceDeletion,
};

topcoat::router::path_param!(pub(super) instance_id: i64, error = not_found);

/// The Instance Name and IP fields, as submitted.
#[derive(Deserialize, Clone)]
pub(super) struct InstanceFields {
    pub(super) name: String,
    pub(super) ip: String,
}

/// A rejected Service Instance submission: the choice made (a Service on a
/// Host page, a Host on a Service page), the fields as typed, and which
/// field was wrong and why.
pub(super) struct FailedInstance {
    pub(super) choice: Option<i64>,
    pub(super) fields: InstanceFields,
    pub(super) error: FieldError,
}

/// The form adding a Service Instance. `choice_name` is the field choosing
/// the other side (`service` on a Host page, `host` on a Service page),
/// `choices` its options as (id, name).
#[component]
pub(super) async fn add_instance_form(
    action: String,
    choice_name: &'static str,
    choice_label: &'static str,
    choices: Vec<(i64, String)>,
    failed: Option<FailedInstance>,
) -> Result<impl View> {
    let chosen = failed.as_ref().and_then(|f| f.choice);
    let (name, ip) = failed.as_ref().map_or((String::new(), String::new()), |f| {
        (f.fields.name.clone(), f.fields.ip.clone())
    });
    let error_for = |field: &str| failed.as_ref().and_then(|f| f.error.on(field));
    let (name_error, ip_error) = (error_for("name"), error_for("ip"));
    Ok(view! {
        <h3>"Add a Service Instance"</h3>
        if choices.is_empty() {
            <p class="hint">
                (if choice_name == "service" { "Create a Service first." } else { "Create a Host first." })
            </p>
        } else {
            <form method="post" action=(action)>
                <label for="instance-choice">(choice_label)</label>
                <select id="instance-choice" name=(choice_name)>
                    #[key(id)]
                    for (id, label) in choices {
                        <option value=(id)
                            if chosen == Some(id) {
                                selected=(true)
                            }
                        >(label)</option>
                    }
                </select>
                <label for="instance-name">"Instance Name"</label>
                <input id="instance-name" name="name" value=(name) required=(true)
                    if name_error.is_some() {
                        aria-invalid="true"
                        aria-describedby="instance-name-error"
                    }
                >
                if let Some(error) = name_error {
                    <p id="instance-name-error" class="error">(error)</p>
                }
                <label for="instance-ip">"IP"</label>
                <input id="instance-ip" name="ip" value=(ip) required=(true)
                    if ip_error.is_some() {
                        aria-invalid="true"
                        aria-describedby="instance-ip-error"
                    }
                >
                if let Some(error) = ip_error {
                    <p id="instance-ip-error" class="error">(error)</p>
                }
                <p><button type="submit">"Add Service Instance"</button></p>
            </form>
        }
    })
}

/// A Host's Service Instances, grouped by Service.
#[component]
pub(super) async fn host_instances_list(instances: Vec<ServiceInstance>) -> Result<impl View> {
    let mut by_service: Vec<(i64, String, bool, Vec<ServiceInstance>)> = Vec::new();
    for instance in instances {
        match by_service.last_mut() {
            Some((id, _, _, list)) if *id == instance.service_id => list.push(instance),
            _ => by_service.push((
                instance.service_id,
                instance.service_name.clone(),
                instance.service_exists,
                vec![instance],
            )),
        }
    }
    Ok(view! {
        if by_service.is_empty() {
            <p>"No Service is deployed on this Host."</p>
        } else {
            #[key(service_id)]
            for (service_id, service_name, exists, list) in by_service {
                <h3>
                    if exists {
                        <a href=(href!(service_detail, ServiceId(service_id)))>(service_name)</a>
                    } else {
                        (service_name)
                    }
                </h3>
                <ul>
                    #[key(instance.id)]
                    for instance in list {
                        <li>
                            <a href=(href!(instance_detail, InstanceId(instance.id)))>(instance.name.clone())</a>
                            " at " <code>(instance.ip.clone())</code>
                            " · "
                            <a href=(href!(clone_instance_form, InstanceId(instance.id)))
                                aria-label=(format!("Clone {}", instance.label()))
                            >"Clone…"</a>
                        </li>
                    }
                </ul>
            }
        }
    })
}

/// A Service's Service Instances: Host, Instance Name and IP.
#[component]
pub(super) async fn service_instances_list(instances: Vec<ServiceInstance>) -> Result<impl View> {
    Ok(view! {
        if instances.is_empty() {
            <p>"This Service isn't deployed anywhere."</p>
        } else {
            <table>
                <thead><tr><th>"Host"</th><th>"Instance Name"</th><th>"IP"</th></tr></thead>
                <tbody>
                    #[key(instance.id)]
                    for instance in instances {
                        <tr>
                            <td>
                                if instance.host_exists {
                                    <a href=(href!(host_detail, HostId(instance.host_id)))>(instance.host_name)</a>
                                } else {
                                    (instance.host_name)
                                }
                            </td>
                            <td><a href=(href!(instance_detail, InstanceId(instance.id)))>(instance.name)</a></td>
                            <td><code>(instance.ip)</code></td>
                        </tr>
                    }
                </tbody>
            </table>
        }
    })
}

/// On a delete confirmation: the Service Instances the delete removes, each
/// with its Own Variables.
#[component]
pub(super) async fn removed_instances_section(
    instances: Vec<ServiceInstanceDeletion>,
) -> Result<impl View> {
    Ok(view! {
        <h2>"Service Instances removed"</h2>
        if instances.is_empty() {
            <p>"None."</p>
        } else {
            <ul>
                #[key(deletion.instance.id)]
                for deletion in instances {
                    <li>
                        (deletion.instance.label())
                        " at " <code>(deletion.instance.ip.clone())</code>
                        if !deletion.own_variables.is_empty() {
                            ", with its Own Variables "
                            (deletion
                                .own_variables
                                .iter()
                                .map(|v| v.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", "))
                        }
                    </li>
                }
            </ul>
        }
    })
}

/// Everything a Service Instance's page shows.
pub(super) struct InstancePage {
    instance: ServiceInstance,
    variables: Vec<OwnVariable>,
    effective: Vec<EffectiveVariable>,
}

async fn load_instance_page(cx: &Cx, instance: ServiceInstance) -> Result<InstancePage> {
    let owner = Owner::ServiceInstance(instance.id);
    Ok(InstancePage {
        variables: inventory(cx).own_variables(owner).await?,
        effective: inventory(cx).effective_variables(owner).await?,
        instance,
    })
}

/// The Service Instance named by the `{instance_id}` path parameter, or a
/// 404.
async fn path_instance(cx: &Cx) -> Result<ServiceInstance> {
    let id = *path_param::<InstanceId>(cx)?;
    Ok(inventory(cx)
        .service_instance(id)
        .await?
        .ok_or_not_found()?)
}

fn instance_url(cx: &Cx, id: i64, section: &str) -> String {
    format!(
        "{}#{section}",
        href!(instance_detail, InstanceId(id)).resolve(cx)
    )
}

#[page("/instances/{instance_id}")]
pub(super) async fn instance_detail(cx: &Cx) -> Result<impl View> {
    let instance = path_instance(cx).await?;
    let page = load_instance_page(cx, instance).await?;
    Ok(view! { instance_view(page: page, edit: None, failed: None) })
}

#[page(POST "/instances/{instance_id}")]
pub(super) async fn update_instance(
    cx: &Cx,
    Form(input): Form<InstanceFields>,
) -> Result<impl View> {
    let instance = path_instance(cx).await?;
    match inventory(cx)
        .update_service_instance(instance.id, &input.name, &input.ip)
        .await
    {
        Ok(_) => Err(see_other(instance_url(cx, instance.id, "identity")).into()),
        Err(crate::inventory::Error::Invalid { field, message }) => {
            let page = load_instance_page(cx, instance).await?;
            let edit = FailedInstance {
                choice: None,
                fields: input,
                error: FieldError { field, message },
            };
            Ok(view! { instance_view(page: page, edit: Some(edit), failed: None) })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[page(POST "/instances/{instance_id}/variables")]
pub(super) async fn set_instance_variable(
    cx: &Cx,
    Form(input): Form<VariableInput>,
) -> Result<impl View> {
    let instance = path_instance(cx).await?;
    match inventory(cx)
        .set_variable(
            Owner::ServiceInstance(instance.id),
            &input.name,
            &input.value,
        )
        .await
    {
        Ok(_) => Err(see_other(instance_url(cx, instance.id, "variables")).into()),
        Err(crate::inventory::Error::Invalid { field, message }) => {
            let page = load_instance_page(cx, instance).await?;
            let failed = FailedVariable {
                input,
                field,
                message,
            };
            Ok(view! { instance_view(page: page, edit: None, failed: Some(failed)) })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[page(POST "/instances/{instance_id}/variables/remove")]
pub(super) async fn remove_instance_variable(
    cx: &Cx,
    Form(input): Form<RemoveVariable>,
) -> Result<impl View> {
    let instance = path_instance(cx).await?;
    inventory(cx)
        .remove_variable(Owner::ServiceInstance(instance.id), &input.name)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(instance_url(cx, instance.id, "variables")).into())
}

/// A Service Instance's page. `edit` is a rejected Instance Name / IP
/// change, `failed` a rejected Variable submission.
#[component]
async fn instance_view(
    cx: &Cx,
    page: InstancePage,
    edit: Option<FailedInstance>,
    failed: Option<FailedVariable>,
) -> Result<impl View> {
    let InstancePage {
        instance,
        variables,
        effective,
    } = page;
    let id = instance.id;
    let (name, ip) = edit
        .as_ref()
        .map_or((instance.name.clone(), instance.ip.clone()), |e| {
            (e.fields.name.clone(), e.fields.ip.clone())
        });
    let error_for = |field: &str| edit.as_ref().and_then(|e| e.error.on(field));
    let (name_error, ip_error) = (error_for("name"), error_for("ip"));
    Ok(view! {
        if edit.is_some() || failed.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <h1>(instance.label())</h1>
        <p>
            "Service "
            if instance.service_exists {
                <a href=(href!(service_detail, ServiceId(instance.service_id)))>(instance.service_name.clone())</a>
            } else {
                (instance.service_name.clone())
            }
            ", on Host "
            if instance.host_exists {
                <a href=(href!(host_detail, HostId(instance.host_id)))>(instance.host_name.clone())</a>
            } else {
                (instance.host_name.clone())
            }
            "."
        </p>
        <p class="actions">
            <a href=(href!(delete_instance_confirmation, InstanceId(id)))>"Delete…"</a>
        </p>

        <h2 id="identity">"Instance Name and IP"</h2>
        <form method="post" action=(href!(update_instance, InstanceId(id)))>
            <label for="name">"Instance Name"</label>
            <input id="name" name="name" value=(name) required=(true)
                if name_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="name-error"
                }
            >
            if let Some(error) = name_error {
                <p id="name-error" class="error">(error)</p>
            }
            <label for="ip">"IP"</label>
            <input id="ip" name="ip" value=(ip) required=(true)
                if ip_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="ip-error"
                }
            >
            if let Some(error) = ip_error {
                <p id="ip-error" class="error">(error)</p>
            }
            <p><button type="submit">"Save"</button></p>
        </form>

        own_variables_section(
            set_url: href!(set_instance_variable, InstanceId(id)).resolve(cx),
            remove_url: href!(remove_instance_variable, InstanceId(id)).resolve(cx),
            variables: variables,
            failed: failed,
            empty: "This Service Instance has no Own Variables.".to_string(),
        )
        effective_variables_section(effective: effective)
    })
}

#[page("/instances/{instance_id}/delete")]
pub(super) async fn delete_instance_confirmation(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<InstanceId>(cx)?;
    let deletion = inventory(cx)
        .service_instance_deletion(id)
        .await
        .map_err(page_error)?;
    let title = deletion.instance.label();
    Ok(view! {
        <h1>"Delete " (title.clone())</h1>
        <p>"This removes the Service Instance " <strong>(title.clone())</strong> "."</p>
        removed_variables_section(variables: deletion.own_variables)
        <form method="post" action=(href!(delete_instance, InstanceId(id)))>
            <p class="actions">
                <button type="submit" class="danger">"Delete " (title)</button>
                <a href=(href!(instance_detail, InstanceId(id)))>"Cancel"</a>
            </p>
        </form>
    })
}

#[page(POST "/instances/{instance_id}/delete")]
pub(super) async fn delete_instance(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<InstanceId>(cx)?;
    let done = inventory(cx)
        .delete_service_instance(id)
        .await
        .map_err(page_error)?;
    // Back to its Host, or its Service if the Host is gone (cli/si data),
    // or the Hosts list if both are.
    let instance = done.value.instance;
    let back = if instance.host_exists {
        href!(host_detail, HostId(instance.host_id)).resolve(cx) + "#instances"
    } else if instance.service_exists {
        href!(service_detail, ServiceId(instance.service_id)).resolve(cx) + "#instances"
    } else {
        href!(super::hosts::hosts_list).resolve(cx)
    };
    Err::<(), _>(see_other(back).into())
}

#[derive(Deserialize)]
pub(super) struct CloneInput {
    host: i64,
    ip: String,
}

/// A rejected clone: the target Host and IP as submitted, and which field
/// was wrong and why.
pub(super) struct FailedClone {
    host: i64,
    ip: String,
    error: FieldError,
}

#[page("/instances/{instance_id}/clone")]
pub(super) async fn clone_instance_form(cx: &Cx) -> Result<impl View> {
    let instance = path_instance(cx).await?;
    let hosts = clone_targets(cx, &instance).await?;
    Ok(view! { clone_view(instance: instance, hosts: hosts, failed: None) })
}

#[page(POST "/instances/{instance_id}/clone")]
pub(super) async fn clone_instance(cx: &Cx, Form(input): Form<CloneInput>) -> Result<impl View> {
    let instance = path_instance(cx).await?;
    match inventory(cx)
        .clone_service_instance(instance.id, input.host, &input.ip)
        .await
    {
        Ok(done) => {
            Err(see_other(href!(instance_detail, InstanceId(done.value.id)).resolve(cx)).into())
        }
        Err(crate::inventory::Error::Invalid { field, message }) => {
            let hosts = clone_targets(cx, &instance).await?;
            let failed = FailedClone {
                host: input.host,
                ip: input.ip,
                error: FieldError { field, message },
            };
            Ok(view! { clone_view(instance: instance, hosts: hosts, failed: Some(failed)) })
        }
        Err(e) => Err(page_error(e)),
    }
}

/// The Hosts a Service Instance can be cloned onto: every Host but its own
/// (where its Instance Name is necessarily taken).
async fn clone_targets(cx: &Cx, instance: &ServiceInstance) -> Result<Vec<Host>> {
    Ok(inventory(cx)
        .hosts()
        .await?
        .into_iter()
        .filter(|h| h.id != instance.host_id)
        .collect())
}

/// The clone form: the target Host and the clone's IP. The Service,
/// Instance Name and Own Variables come from `instance`.
#[component]
async fn clone_view(
    instance: ServiceInstance,
    hosts: Vec<Host>,
    failed: Option<FailedClone>,
) -> Result<impl View> {
    let chosen = failed.as_ref().map(|f| f.host);
    let ip = failed.as_ref().map_or(String::new(), |f| f.ip.clone());
    let error_for = |field: &str| failed.as_ref().and_then(|f| f.error.on(field));
    let (host_error, ip_error) = (error_for("host"), error_for("ip"));
    let id = instance.id;
    Ok(view! {
        if failed.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <h1>"Clone " (instance.label())</h1>
        <p>
            "The clone runs " (instance.service_name.clone()) " as " <strong>(instance.name.clone())</strong>
            " on the Host you pick, at a new IP, with a copy of this Service Instance's Own Variables."
        </p>
        if hosts.is_empty() {
            <p>"There is no other Host to clone it onto. Create a Host first."</p>
        } else {
        <form method="post" action=(href!(clone_instance, InstanceId(id)))>
            <label for="host">"Target Host"</label>
            <select id="host" name="host"
                if host_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="host-error"
                }
            >
                #[key(host.id)]
                for host in hosts {
                    <option value=(host.id)
                        if chosen == Some(host.id) {
                            selected=(true)
                        }
                    >(host.name)</option>
                }
            </select>
            if let Some(error) = host_error {
                <p id="host-error" class="error">(error)</p>
            }
            <label for="ip">"IP"</label>
            <input id="ip" name="ip" value=(ip) required=(true)
                if ip_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="ip-error"
                }
            >
            if let Some(error) = ip_error {
                <p id="ip-error" class="error">(error)</p>
            }
            <p class="actions">
                <button type="submit">"Clone"</button>
                <a href=(href!(instance_detail, InstanceId(id)))>"Cancel"</a>
            </p>
        </form>
        }
    })
}
