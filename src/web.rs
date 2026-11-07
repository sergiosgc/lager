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
        error::{RouterErrorExt, SeeOther, not_found, see_other},
        href, layout, page, path_param, route,
    },
    view::{View, component, view},
};

use crate::inventory::{self, Host, Inventory, OwnVariable};

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
        .page(set_host_variable)
        .page(remove_host_variable)
        .build()
}

fn inventory(cx: &Cx) -> &Inventory {
    app_context::<Inventory>(cx)
}

const STYLE: &str = "
body { font-family: system-ui, sans-serif; max-width: 48rem; margin: 0 auto; padding: 0 1rem; }
nav { display: flex; gap: 1rem; padding: 1rem 0; border-bottom: 1px solid #ccc; }
.error { color: #b00020; }
.hint { color: #555; font-size: .9em; margin: .25rem 0; }
label { display: block; margin-bottom: .25rem; }
table { border-collapse: collapse; width: 100%; }
th, td { text-align: left; vertical-align: top; padding: .25rem .5rem .25rem 0; }
td form { display: flex; gap: .5rem; align-items: flex-start; }
textarea { font-family: ui-monospace, monospace; width: 100%; min-width: 12rem; }
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
        Err(e) => Err(page_error(e)),
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
                    aria-describedby="name-hint name-error"
                } else {
                    aria-describedby="name-hint"
                }
            >
            <p id="name-hint" class="hint">
                "Saved in lowercase, keeping only letters a–z, digits, \"-\" and \"_\"."
            </p>
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
    let host = path_host(cx).await?;
    let variables = inventory(cx).host_variables(host.id).await?;
    Ok(view! { host_view(host: host, variables: variables, failed: None) })
}

/// The Host named by the `{host_id}` path parameter, or a 404.
async fn path_host(cx: &Cx) -> Result<Host> {
    let id = *path_param::<HostId>(cx)?;
    Ok(inventory(cx).host(id).await?.ok_or_not_found()?)
}

/// Turns an Inventory error a page can't show in a form into a response:
/// a 404 when the entity is gone, otherwise a 500.
fn page_error(e: inventory::Error) -> topcoat::Error {
    match e {
        inventory::Error::NotFound(_) => not_found().into(),
        e => e.into(),
    }
}

/// Which Variable form a rejected submission came from.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum VariableFormKind {
    /// The form adding a new Variable.
    Add,
    /// The in-place edit form on an existing Variable's row.
    Edit,
}

#[derive(Deserialize)]
struct VariableInput {
    form: VariableFormKind,
    name: String,
    value: String,
}

/// A Variable submission that was rejected, to show again with its error.
struct FailedVariable {
    input: VariableInput,
    field: &'static str,
    message: String,
}

#[page(POST "/hosts/{host_id}/variables")]
async fn set_host_variable(cx: &Cx, Form(input): Form<VariableInput>) -> Result<impl View> {
    let host = path_host(cx).await?;
    match inventory(cx)
        .set_host_variable(host.id, &input.name, &input.value)
        .await
    {
        Ok(_) => Err(see_other(host_variables_url(cx, host.id)).into()),
        Err(inventory::Error::Invalid { field, message }) => {
            let variables = inventory(cx).host_variables(host.id).await?;
            let failed = FailedVariable {
                input,
                field,
                message,
            };
            Ok(view! { host_view(host: host, variables: variables, failed: Some(failed)) })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[derive(Deserialize)]
struct RemoveVariable {
    name: String,
}

#[page(POST "/hosts/{host_id}/variables/remove")]
async fn remove_host_variable(cx: &Cx, Form(input): Form<RemoveVariable>) -> Result<impl View> {
    let host = path_host(cx).await?;
    inventory(cx)
        .remove_host_variable(host.id, &input.name)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(host_variables_url(cx, host.id)).into())
}

fn host_variables_url(cx: &Cx, host_id: i64) -> String {
    format!(
        "{}#variables",
        href!(host_detail, HostId(host_id)).resolve(cx)
    )
}

/// A Host's page: its name and its Own Variables, each editable in place,
/// plus a form to add one. A rejected submission is shown again in the form
/// it came from, with its error.
#[component]
async fn host_view(
    host: Host,
    variables: Vec<OwnVariable>,
    failed: Option<FailedVariable>,
) -> Result<impl View> {
    let host_id = host.id;
    let (edit_failure, add_failure) = match failed {
        // An edit whose row is gone (e.g. removed meanwhile) shows in the add
        // form instead, so its input isn't lost.
        Some(f)
            if f.input.form == VariableFormKind::Edit
                && variables.iter().any(|v| v.name == f.input.name) =>
        {
            (Some(f), None)
        }
        other => (None, other),
    };
    let add_name = add_failure
        .as_ref()
        .map_or(String::new(), |f| f.input.name.clone());
    let add_value = add_failure
        .as_ref()
        .map_or(String::new(), |f| f.input.value.clone());
    let add_error = |field: &str| {
        add_failure
            .as_ref()
            .filter(|f| f.field == field)
            .map(|f| f.message.clone())
    };
    let (add_name_error, add_value_error) = (add_error("name"), add_error("value"));
    let status_failed = edit_failure.is_some() || add_failure.is_some();
    Ok(view! {
        if status_failed {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <p><a href=(href!(hosts_list))>"All Hosts"</a></p>
        <h1>(host.name)</h1>

        <h2 id="variables">"Own Variables"</h2>
        if variables.is_empty() {
            <p>"This Host has no Own Variables."</p>
        } else {
            <table>
                <thead><tr><th>"Name"</th><th>"Value (JSON)"</th><th></th></tr></thead>
                <tbody>
                    #[key(variable.name.clone())]
                    for variable in variables {
                        let failure = edit_failure
                            .as_ref()
                            .filter(|f| f.input.name == variable.name);
                        let value = failure.map_or(variable.value.clone(), |f| f.input.value.clone());
                        let error = failure.map(|f| f.message.clone());
                        let error_id = format!("variable-error-{}", variable.name);
                        <tr>
                            <th scope="row">(variable.name.clone())</th>
                            <td>
                                <form method="post" action=(href!(set_host_variable, HostId(host_id)))>
                                    <input type="hidden" name="form" value="edit">
                                    <input type="hidden" name="name" value=(variable.name.clone())>
                                    <div>
                                        <textarea name="value" rows="1"
                                            aria-label=(format!("Value of {}", variable.name))
                                            if error.is_some() {
                                                aria-invalid="true"
                                                aria-describedby=(error_id.clone())
                                            }
                                        >(value)</textarea>
                                        if let Some(error) = error {
                                            <p id=(error_id) class="error">(error)</p>
                                        }
                                    </div>
                                    <button type="submit">"Save"</button>
                                </form>
                            </td>
                            <td>
                                <form method="post" action=(href!(remove_host_variable, HostId(host_id)))>
                                    <input type="hidden" name="name" value=(variable.name.clone())>
                                    <button type="submit"
                                        aria-label=(format!("Remove {}", variable.name))
                                    >"Remove"</button>
                                </form>
                            </td>
                        </tr>
                    }
                </tbody>
            </table>
        }

        <h3>"Add a Variable"</h3>
        <form method="post" action=(href!(set_host_variable, HostId(host_id)))>
            <input type="hidden" name="form" value="add">
            <label for="variable-name">"Name"</label>
            <input id="variable-name" name="name" value=(add_name) required=(true)
                if add_name_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="variable-name-error"
                }
            >
            if let Some(error) = add_name_error {
                <p id="variable-name-error" class="error">(error)</p>
            }
            <label for="variable-value">"Value (JSON)"</label>
            <textarea id="variable-value" name="value" rows="3"
                aria-describedby=(if add_value_error.is_some() {
                    "variable-value-hint variable-value-error"
                } else {
                    "variable-value-hint"
                })
                if add_value_error.is_some() {
                    aria-invalid="true"
                }
            >(add_value)</textarea>
            <p id="variable-value-hint" class="hint">
                "Any JSON value: a quoted string like \"ntp.example.org\", a number, true or false, a list or an object."
            </p>
            if let Some(error) = add_value_error {
                <p id="variable-value-error" class="error">(error)</p>
            }
            <p><button type="submit">"Add Variable"</button></p>
        </form>
    })
}
