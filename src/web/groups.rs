//! Group pages: the nested list, a Group's page with its Parent Group,
//! child Groups and member Hosts, and the create, rename and delete forms.

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
use super::variables::{
    FailedVariable, RemoveVariable, VariableInput, effective_variables_section,
    own_variables_section, removed_variables_section,
};
use super::{inventory, page_error};
use crate::inventory::{EffectiveVariable, Group, GroupDeletion, Host, OwnVariable, Owner};

topcoat::router::path_param!(pub(super) group_id: i64, error = not_found);

/// Everything a Group's page shows.
pub(super) struct GroupPage {
    group: Group,
    parent: Option<Group>,
    /// Every other Group, as Parent Group candidates.
    other_groups: Vec<Group>,
    children: Vec<Group>,
    members: Vec<Host>,
    /// Hosts that aren't members yet, to add.
    non_members: Vec<Host>,
    variables: Vec<OwnVariable>,
    effective: Vec<EffectiveVariable>,
}

async fn load_group_page(cx: &Cx, group: Group) -> Result<GroupPage> {
    let inventory = inventory(cx);
    let all = inventory.groups().await?;
    let parent = all.iter().find(|g| Some(g.id) == group.parent_id).cloned();
    let other_groups = all.into_iter().filter(|g| g.id != group.id).collect();
    let children = inventory.child_groups(group.id).await?;
    let members = inventory.group_hosts(group.id).await?;
    let non_members = inventory
        .hosts()
        .await?
        .into_iter()
        .filter(|h| !members.iter().any(|m| m.id == h.id))
        .collect();
    Ok(GroupPage {
        variables: inventory.own_variables(Owner::Group(group.id)).await?,
        effective: inventory
            .effective_variables(Owner::Group(group.id))
            .await?,
        group,
        parent,
        other_groups,
        children,
        members,
        non_members,
    })
}

/// The Group named by the `{group_id}` path parameter, or a 404.
async fn path_group(cx: &Cx) -> Result<Group> {
    let id = *path_param::<GroupId>(cx)?;
    Ok(inventory(cx).group(id).await?.ok_or_not_found()?)
}

fn group_url(cx: &Cx, group_id: i64, section: &str) -> String {
    format!(
        "{}#{section}",
        href!(group_detail, GroupId(group_id)).resolve(cx)
    )
}

#[page("/groups")]
pub(super) async fn groups_list(cx: &Cx) -> Result<impl View> {
    let tree = inventory(cx).group_tree().await?;
    Ok(view! {
        <h1>"Groups"</h1>
        <p><a href=(href!(new_group_form))>"New Group"</a></p>
        if tree.is_empty() {
            <p>"There are no Groups yet."</p>
        } else {
            <ul class="tree">
                for entry in tree {
                    <li style=(format!("margin-left: {}rem", entry.depth * 2))>
                        <a href=(href!(group_detail, GroupId(entry.group.id)))>(entry.group.name)</a>
                    </li>
                }
            </ul>
        }
    })
}

#[derive(Deserialize)]
pub(super) struct GroupName {
    name: String,
}

#[page("/groups/new")]
pub(super) async fn new_group_form() -> Result<impl View> {
    Ok(view! { group_name_view(group: None, name: String::new(), error: None) })
}

#[page(POST "/groups/new")]
pub(super) async fn create_group(cx: &Cx, Form(input): Form<GroupName>) -> Result<impl View> {
    match inventory(cx).create_group(&input.name).await {
        Ok(done) => Err(see_other(href!(group_detail, GroupId(done.value.id)).resolve(cx)).into()),
        Err(crate::inventory::Error::Invalid { message, .. }) => {
            Ok(view! { group_name_view(group: None, name: input.name, error: Some(message)) })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[page("/groups/{group_id}/rename")]
pub(super) async fn rename_group_form(cx: &Cx) -> Result<impl View> {
    let group = path_group(cx).await?;
    let name = group.name.clone();
    Ok(view! { group_name_view(group: Some(group), name: name, error: None) })
}

#[page(POST "/groups/{group_id}/rename")]
pub(super) async fn rename_group(cx: &Cx, Form(input): Form<GroupName>) -> Result<impl View> {
    let group = path_group(cx).await?;
    match inventory(cx).rename_group(group.id, &input.name).await {
        Ok(_) => Err(see_other(href!(group_detail, GroupId(group.id)).resolve(cx)).into()),
        Err(crate::inventory::Error::Invalid { message, .. }) => Ok(
            view! { group_name_view(group: Some(group), name: input.name, error: Some(message)) },
        ),
        Err(e) => Err(page_error(e)),
    }
}

/// The create form (no `group`) or the rename form for `group`, showing
/// `name` as typed and the error it was rejected with, if any.
#[component]
async fn group_name_view(
    cx: &Cx,
    group: Option<Group>,
    name: String,
    error: Option<String>,
) -> Result<impl View> {
    Ok(view! {
        if error.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        match &group {
            Some(group) => <h1>"Rename " (group.name.clone())</h1>,
            None => <h1>"New Group"</h1>,
        }
        <form method="post"
            action=(match &group {
                Some(group) => href!(rename_group, GroupId(group.id)).resolve(cx),
                None => href!(create_group).resolve(cx),
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
                    (if group.is_some() { "Rename" } else { "Create Group" })
                </button>
                <a href=(match &group {
                    Some(group) => href!(group_detail, GroupId(group.id)).resolve(cx),
                    None => href!(groups_list).resolve(cx),
                })>"Cancel"</a>
            </p>
        </form>
    })
}

#[page("/groups/{group_id}")]
pub(super) async fn group_detail(cx: &Cx) -> Result<impl View> {
    let group = path_group(cx).await?;
    let page = load_group_page(cx, group).await?;
    Ok(view! { group_view(page: page, parent_error: None, failed: None) })
}

#[derive(Deserialize)]
pub(super) struct ParentChoice {
    /// Blank means "no Parent Group".
    parent: Option<i64>,
}

#[page(POST "/groups/{group_id}/parent")]
pub(super) async fn set_group_parent(
    cx: &Cx,
    Form(input): Form<ParentChoice>,
) -> Result<impl View> {
    let group = path_group(cx).await?;
    match inventory(cx).set_group_parent(group.id, input.parent).await {
        Ok(_) => Err(see_other(group_url(cx, group.id, "parent")).into()),
        Err(crate::inventory::Error::Invalid { message, .. }) => {
            let page = load_group_page(cx, group).await?;
            Ok(view! { group_view(page: page, parent_error: Some(message), failed: None) })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[derive(Deserialize)]
pub(super) struct HostChoice {
    host: i64,
}

#[page(POST "/groups/{group_id}/hosts")]
pub(super) async fn add_group_host(cx: &Cx, Form(input): Form<HostChoice>) -> Result<impl View> {
    let group = path_group(cx).await?;
    inventory(cx)
        .add_host_to_group(input.host, group.id)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(group_url(cx, group.id, "members")).into())
}

#[page(POST "/groups/{group_id}/hosts/remove")]
pub(super) async fn remove_group_host(cx: &Cx, Form(input): Form<HostChoice>) -> Result<impl View> {
    let group = path_group(cx).await?;
    inventory(cx)
        .remove_host_from_group(input.host, group.id)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(group_url(cx, group.id, "members")).into())
}

#[page(POST "/groups/{group_id}/variables")]
pub(super) async fn set_group_variable(
    cx: &Cx,
    Form(input): Form<VariableInput>,
) -> Result<impl View> {
    let group = path_group(cx).await?;
    match inventory(cx)
        .set_variable(Owner::Group(group.id), &input.name, &input.value)
        .await
    {
        Ok(_) => Err(see_other(group_url(cx, group.id, "variables")).into()),
        Err(crate::inventory::Error::Invalid { field, message }) => {
            let page = load_group_page(cx, group).await?;
            let failed = FailedVariable {
                input,
                field,
                message,
            };
            Ok(view! { group_view(page: page, parent_error: None, failed: Some(failed)) })
        }
        Err(e) => Err(page_error(e)),
    }
}

#[page(POST "/groups/{group_id}/variables/remove")]
pub(super) async fn remove_group_variable(
    cx: &Cx,
    Form(input): Form<RemoveVariable>,
) -> Result<impl View> {
    let group = path_group(cx).await?;
    inventory(cx)
        .remove_variable(Owner::Group(group.id), &input.name)
        .await
        .map_err(page_error)?;
    Err::<(), _>(see_other(group_url(cx, group.id, "variables")).into())
}

/// A Group's page. `parent_error` is a rejected Parent Group change, shown
/// next to its form; `failed` a rejected Variable submission.
#[component]
async fn group_view(
    cx: &Cx,
    page: GroupPage,
    parent_error: Option<String>,
    failed: Option<FailedVariable>,
) -> Result<impl View> {
    let GroupPage {
        group,
        parent,
        other_groups,
        children,
        members,
        non_members,
        variables,
        effective,
    } = page;
    let group_id = group.id;
    Ok(view! {
        if parent_error.is_some() || failed.is_some() {
            (StatusCode::UNPROCESSABLE_ENTITY)
        }
        <p><a href=(href!(groups_list))>"All Groups"</a></p>
        <h1>(group.name.clone())</h1>
        <p class="actions">
            <a href=(href!(rename_group_form, GroupId(group_id)))>"Rename"</a>
            <a href=(href!(delete_group_confirmation, GroupId(group_id)))>"Delete…"</a>
        </p>

        <h2 id="parent">"Parent Group"</h2>
        <p>
            match &parent {
                Some(parent) => {
                    "Nested inside "
                    <a href=(href!(group_detail, GroupId(parent.id)))>(parent.name.clone())</a>
                    "."
                },
                None => "A top-level Group.",
            }
        </p>
        <form method="post" action=(href!(set_group_parent, GroupId(group_id)))>
            <label for="parent">"Parent Group"</label>
            <select id="parent" name="parent"
                if parent_error.is_some() {
                    aria-invalid="true"
                    aria-describedby="parent-error"
                }
            >
                <option value="">"(none: top-level)"</option>
                #[key(candidate.id)]
                for candidate in other_groups {
                    <option value=(candidate.id)
                        if Some(candidate.id) == group.parent_id {
                            selected=(true)
                        }
                    >(candidate.name)</option>
                }
            </select>
            if let Some(error) = parent_error {
                <p id="parent-error" class="error">(error)</p>
            }
            <p><button type="submit">"Set Parent Group"</button></p>
        </form>

        <h2 id="children">"Child Groups"</h2>
        if children.is_empty() {
            <p>"No Group is nested inside this one."</p>
        } else {
            <ul>
                #[key(child.id)]
                for child in children {
                    <li><a href=(href!(group_detail, GroupId(child.id)))>(child.name)</a></li>
                }
            </ul>
        }

        <h2 id="members">"Member Hosts"</h2>
        if members.is_empty() {
            <p>"No Host is directly in this Group."</p>
        } else {
            <ul class="items">
                #[key(member.id)]
                for member in members {
                    <li>
                        <a href=(href!(host_detail, HostId(member.id)))>(member.name.clone())</a>
                        <form method="post" action=(href!(remove_group_host, GroupId(group_id)))>
                            <input type="hidden" name="host" value=(member.id)>
                            <button type="submit"
                                aria-label=(format!("Remove {} from {}", member.name, group.name))
                            >"Remove"</button>
                        </form>
                    </li>
                }
            </ul>
        }
        if !non_members.is_empty() {
            <form method="post" action=(href!(add_group_host, GroupId(group_id))) class="inline">
                <label for="host">"Add a Host"</label>
                <select id="host" name="host">
                    #[key(host.id)]
                    for host in non_members {
                        <option value=(host.id)>(host.name)</option>
                    }
                </select>
                <button type="submit">"Add"</button>
            </form>
        }

        own_variables_section(
            set_url: href!(set_group_variable, GroupId(group_id)).resolve(cx),
            remove_url: href!(remove_group_variable, GroupId(group_id)).resolve(cx),
            variables: variables,
            failed: failed,
            empty: "This Group has no Own Variables.".to_string(),
        )
        effective_variables_section(effective: effective)
    })
}

#[page("/groups/{group_id}/delete")]
pub(super) async fn delete_group_confirmation(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<GroupId>(cx)?;
    let deletion = inventory(cx).group_deletion(id).await.map_err(page_error)?;
    Ok(view! { delete_group_view(deletion: deletion, refused: false) })
}

#[page(POST "/groups/{group_id}/delete")]
pub(super) async fn delete_group(cx: &Cx) -> Result<impl View> {
    let id = *path_param::<GroupId>(cx)?;
    match inventory(cx).delete_group(id).await {
        Ok(_) => Err(see_other(href!(groups_list).resolve(cx)).into()),
        // Something blocks the delete: show the confirmation page again,
        // which says what.
        Err(crate::inventory::Error::Invalid { .. }) => {
            let deletion = inventory(cx).group_deletion(id).await.map_err(page_error)?;
            Ok(view! { delete_group_view(deletion: deletion, refused: true) })
        }
        Err(e) => Err(page_error(e)),
    }
}

/// The delete confirmation: what changes, or why the Group can't be deleted
/// yet. `refused` marks a delete that was attempted and blocked.
#[component]
async fn delete_group_view(deletion: GroupDeletion, refused: bool) -> Result<impl View> {
    let GroupDeletion {
        group,
        member_hosts,
        child_groups,
        own_variables,
        missing_memberships,
        blockers,
    } = deletion;
    let group_id = group.id;
    let name = group.name;
    Ok(view! {
        if refused {
            (StatusCode::CONFLICT)
        }
        <h1>"Delete " (name.clone())</h1>
        if !blockers.is_empty() {
            <div class="error" role="alert">
                <p>(name.clone()) " can't be deleted yet."</p>
                <ul>
                    for blocker in blockers {
                        <li>(blocker)</li>
                    }
                </ul>
            </div>
            <p><a href=(href!(group_detail, GroupId(group_id)))>"Back to " (name)</a></p>
        } else {
            <p>"This removes the Group " <strong>(name.clone())</strong> "."</p>
            removed_variables_section(variables: own_variables)
            <h2>"Hosts leaving it"</h2>
            if member_hosts.is_empty() {
                <p>"None: no Host is directly in it."</p>
            } else {
                <p class="hint">"The Hosts themselves stay."</p>
                <ul>
                    #[key(host.id)]
                    for host in member_hosts {
                        <li>(host.name)</li>
                    }
                </ul>
            }
            if missing_memberships > 0 {
                <p>"Also removes " (missing_memberships) " membership(s) of Hosts that no longer exist."</p>
            }
            <h2>"Child Groups becoming top-level"</h2>
            if child_groups.is_empty() {
                <p>"None: no Group is nested inside it."</p>
            } else {
                <p class="hint">"These Groups stay, without a Parent Group."</p>
                <ul>
                    #[key(child.id)]
                    for child in child_groups {
                        <li>(child.name)</li>
                    }
                </ul>
            }
            <form method="post" action=(href!(delete_group, GroupId(group_id)))>
                <p class="actions">
                    <button type="submit" class="danger">"Delete " (name.clone())</button>
                    <a href=(href!(group_detail, GroupId(group_id)))>"Cancel"</a>
                </p>
            </form>
        }
    })
}
