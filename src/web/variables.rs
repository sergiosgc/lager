//! Variables on entity pages: the Own Variables editor and the Effective
//! Variables table, shared by Host and Group pages.

use serde::Deserialize;
use topcoat::{
    Result,
    router::href,
    view::{View, component, view},
};

use super::groups::{GroupId, group_detail};
use crate::inventory::{EffectiveVariable, OwnVariable, Source};

/// Which Variable form a rejected submission came from.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum VariableFormKind {
    /// The form adding a new Variable.
    Add,
    /// The in-place edit form on an existing Variable's row.
    Edit,
}

#[derive(Deserialize)]
pub(super) struct VariableInput {
    pub(super) form: VariableFormKind,
    pub(super) name: String,
    pub(super) value: String,
}

/// A Variable submission that was rejected, to show again with its error.
pub(super) struct FailedVariable {
    pub(super) input: VariableInput,
    pub(super) field: &'static str,
    pub(super) message: String,
}

#[derive(Deserialize)]
pub(super) struct RemoveVariable {
    pub(super) name: String,
}

/// An entity's Own Variables, each editable in place, plus a form to add
/// one. A rejected submission is shown again in the form it came from,
/// with its error. `set_url` and `remove_url` are where the forms post;
/// `empty` is what to say when there are none.
#[component]
pub(super) async fn own_variables_section(
    set_url: String,
    remove_url: String,
    variables: Vec<OwnVariable>,
    failed: Option<FailedVariable>,
    empty: String,
) -> Result<impl View> {
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
    Ok(view! {
        <h2 id="variables">"Own Variables"</h2>
        if variables.is_empty() {
            <p>(empty)</p>
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
                                <form method="post" action=(set_url.clone())>
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
                                <form method="post" action=(remove_url.clone())>
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
        <form method="post" action=(set_url.clone())>
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

/// An entity's Effective Variables: each value with where it comes from,
/// and which inherited value an Own Variable overrides.
#[component]
pub(super) async fn effective_variables_section(
    effective: Vec<EffectiveVariable>,
) -> Result<impl View> {
    Ok(view! {
        <h2 id="effective">"Effective Variables"</h2>
        if effective.is_empty() {
            <p>"No Variables apply."</p>
        } else {
            <table>
                <thead><tr><th>"Name"</th><th>"Value"</th><th>"From"</th></tr></thead>
                <tbody>
                    #[key(variable.name.clone())]
                    for variable in effective {
                        <tr>
                            <th scope="row">(variable.name)</th>
                            <td><code>(variable.value)</code></td>
                            <td>
                                source_link(source: variable.source)
                                if let Some(hidden) = variable.overrides {
                                    <span class="hint">" (overrides " source_link(source: hidden) ")"</span>
                                }
                            </td>
                        </tr>
                    }
                </tbody>
            </table>
        }
    })
}

/// Where a value comes from: "own value", or a link to the Group.
#[component]
async fn source_link(source: Source) -> Result<impl View> {
    Ok(view! {
        match source {
            Source::Own => "own value",
            Source::Group { id, name } => {
                "Group "
                <a href=(href!(group_detail, GroupId(id)))>(name)</a>
            },
        }
    })
}

/// On a delete confirmation: the Own Variables the delete removes.
#[component]
pub(super) async fn removed_variables_section(variables: Vec<OwnVariable>) -> Result<impl View> {
    Ok(view! {
        <h2>"Own Variables removed"</h2>
        if variables.is_empty() {
            <p>"None."</p>
        } else {
            <table>
                <thead><tr><th>"Name"</th><th>"Value"</th></tr></thead>
                <tbody>
                    #[key(variable.name.clone())]
                    for variable in variables {
                        <tr><th scope="row">(variable.name)</th><td><code>(variable.value)</code></td></tr>
                    }
                </tbody>
            </table>
            <p class="hint">"Variable names stay, for anything else using them."</p>
        }
    })
}
