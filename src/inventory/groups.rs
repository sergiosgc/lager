//! Groups: their hierarchy (Parent Groups) and their member Hosts.
//!
//! Nothing in the schema stops a Group from being its own ancestor (ADR
//! 0003), and `cli/si` doesn't either (#2), so every walk up or down the
//! hierarchy tolerates cycles already in the data, while lager itself never
//! creates one.

use std::collections::{HashMap, HashSet};

use super::*;

/// A named set of Hosts. `parent_id` is its Parent Group, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
}

impl From<models::Group> for Group {
    fn from(g: models::Group) -> Self {
        Group {
            id: g.id,
            name: g.name,
            parent_id: g.parent_id,
        }
    }
}

/// One Group in the hierarchy, `depth` levels below a top-level Group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupTreeEntry {
    pub depth: usize,
    pub group: Group,
}

/// What deleting a Group does: removes it and its memberships, and clears
/// the Parent Group of its child Groups (which stay). While `blockers` isn't
/// empty, the Group can't be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDeletion {
    pub group: Group,
    /// Hosts that stop being members (the Hosts themselves stay).
    pub member_hosts: Vec<Host>,
    /// Groups that lose their Parent Group (and become top-level).
    pub child_groups: Vec<Group>,
    /// Memberships of Hosts that no longer exist, left behind by `cli/si`;
    /// the delete removes them too.
    pub missing_memberships: usize,
    /// Relations lager can't remove yet, each as a sentence for the operator.
    pub blockers: Vec<String>,
}

impl Inventory {
    /// All Groups, sorted by name.
    pub async fn groups(&self) -> Result<Vec<Group>, Error> {
        let mut groups = all_groups(&mut self.db.clone()).await?;
        groups.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(groups)
    }

    /// The Group with this id, if there is one.
    pub async fn group(&self, id: i64) -> Result<Option<Group>, Error> {
        let group = models::Group::filter_by_id(id)
            .first()
            .exec(&mut self.db.clone())
            .await?;
        Ok(group.map(Group::from))
    }

    /// Every Group in hierarchy order: each top-level Group followed by its
    /// descendants, depth first, siblings by name. Groups caught in a cycle
    /// already in the data come last, each listed once.
    pub async fn group_tree(&self) -> Result<Vec<GroupTreeEntry>, Error> {
        let groups = all_groups(&mut self.db.clone()).await?;
        let ids: HashSet<i64> = groups.iter().map(|g| g.id).collect();
        let mut children: HashMap<Option<i64>, Vec<&Group>> = HashMap::new();
        for g in &groups {
            // A parent that doesn't exist counts as none.
            let parent = g.parent_id.filter(|p| ids.contains(p));
            children.entry(parent).or_default().push(g);
        }
        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| a.name.cmp(&b.name));
        }
        let mut tree = Vec::new();
        let mut visited = HashSet::new();
        let mut stack: Vec<(usize, &Group)> = children
            .get(&None)
            .map(|roots| roots.iter().rev().map(|g| (0, *g)).collect())
            .unwrap_or_default();
        let mut leftovers: Vec<&Group> = groups.iter().collect();
        leftovers.sort_by(|a, b| b.name.cmp(&a.name));
        loop {
            while let Some((depth, g)) = stack.pop() {
                if !visited.insert(g.id) {
                    continue;
                }
                tree.push(GroupTreeEntry {
                    depth,
                    group: g.clone(),
                });
                if let Some(kids) = children.get(&Some(g.id)) {
                    stack.extend(kids.iter().rev().map(|k| (depth + 1, *k)));
                }
            }
            // Whatever is left is unreachable from a top-level Group: a cycle.
            match leftovers.pop() {
                Some(g) if !visited.contains(&g.id) => stack.push((0, g)),
                Some(_) => {}
                None => break,
            }
        }
        Ok(tree)
    }

    /// The Groups whose Parent Group is this one, sorted by name.
    pub async fn child_groups(&self, group_id: i64) -> Result<Vec<Group>, Error> {
        child_groups(&mut self.db.clone(), group_id).await
    }

    /// Creates a top-level Group. Its name, with surrounding whitespace
    /// removed, must be non-empty and not already used by another Group.
    pub async fn create_group(&self, name: &str) -> Result<Done<Group>, Error> {
        let name = valid_group_name(name)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        unused_group_name(&mut tx, name, None).await?;
        let group = models::Group::create().name(name).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(group.into()))
    }

    /// Renames a Group, keeping its identity, so its members, children and
    /// Variables stay attached. The new name follows the creation rules.
    pub async fn rename_group(&self, group_id: i64, name: &str) -> Result<Done<Group>, Error> {
        let name = valid_group_name(name)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let mut group = existing_group(&mut tx, group_id).await?;
        unused_group_name(&mut tx, name, Some(group_id)).await?;
        group.update().name(name).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(group.into()))
    }

    /// Sets or clears a Group's Parent Group. A parent that is the Group
    /// itself, or one of its descendants, would create a cycle and is
    /// rejected, naming the loop.
    pub async fn set_group_parent(
        &self,
        group_id: i64,
        parent_id: Option<i64>,
    ) -> Result<Done<Group>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let mut group = existing_group(&mut tx, group_id).await?;
        if let Some(parent_id) = parent_id {
            existing_group(&mut tx, parent_id).await?;
            let groups: HashMap<i64, Group> = all_groups(&mut tx)
                .await?
                .into_iter()
                .map(|g| (g.id, g))
                .collect();
            if let Some(path) = cycle_through(&groups, group_id, parent_id) {
                return Err(invalid(
                    "parent",
                    format!(
                        "A Group can't be nested inside itself: {}.",
                        path.join(" → ")
                    ),
                ));
            }
        }
        group.update().parent_id(parent_id).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(group.into()))
    }

    /// The Hosts directly in a Group, sorted by name.
    pub async fn group_hosts(&self, group_id: i64) -> Result<Vec<Host>, Error> {
        group_hosts(&mut self.db.clone(), group_id).await
    }

    /// The Groups a Host is directly in, sorted by name. Membership of their
    /// Parent Groups is implied, not listed.
    pub async fn host_groups(&self, host_id: i64) -> Result<Vec<Group>, Error> {
        host_groups(&mut self.db.clone(), host_id).await
    }

    /// Puts a Host directly in a Group. Adding it again does nothing.
    pub async fn add_host_to_group(&self, host_id: i64, group_id: i64) -> Result<Done<()>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        existing_group(&mut tx, group_id).await?;
        if membership(host_id, group_id)
            .first()
            .exec(&mut tx)
            .await?
            .is_none()
        {
            models::Membership::create()
                .host_id(host_id)
                .group_id(group_id)
                .exec(&mut tx)
                .await?;
        }
        tx.commit().await?;
        Ok(Done::ok(()))
    }

    /// Takes a Host out of a Group. Removing it when it isn't a member does
    /// nothing.
    pub async fn remove_host_from_group(
        &self,
        host_id: i64,
        group_id: i64,
    ) -> Result<Done<()>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        existing_group(&mut tx, group_id).await?;
        membership(host_id, group_id).delete().exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(()))
    }

    /// Replaces the whole set of Groups a Host is directly in, returning the
    /// new set sorted by name. Repeated ids count once; an unknown one
    /// changes nothing.
    pub async fn set_host_groups(
        &self,
        host_id: i64,
        group_ids: &[i64],
    ) -> Result<Done<Vec<Group>>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        let wanted: HashSet<i64> = group_ids.iter().copied().collect();
        for &group_id in &wanted {
            existing_group(&mut tx, group_id).await?;
        }
        models::Membership::filter(models::Membership::fields().host_id().eq(host_id))
            .delete()
            .exec(&mut tx)
            .await?;
        for &group_id in &wanted {
            models::Membership::create()
                .host_id(host_id)
                .group_id(group_id)
                .exec(&mut tx)
                .await?;
        }
        let groups = host_groups(&mut tx, host_id).await?;
        tx.commit().await?;
        Ok(Done::ok(groups))
    }

    /// What deleting a Group would do, or why it can't be deleted yet.
    /// [`Inventory::delete_group`] does exactly this.
    pub async fn group_deletion(&self, group_id: i64) -> Result<GroupDeletion, Error> {
        plan_group_deletion(&mut self.db.clone(), group_id).await
    }

    /// Deletes a Group as listed by [`Inventory::group_deletion`], in one
    /// transaction: its memberships go, its child Groups become top-level.
    pub async fn delete_group(&self, group_id: i64) -> Result<Done<GroupDeletion>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let plan = plan_group_deletion(&mut tx, group_id).await?;
        refuse_if_blocked("group", &plan.group.name, &plan.blockers)?;
        models::Membership::filter(models::Membership::fields().group_id().eq(group_id))
            .delete()
            .exec(&mut tx)
            .await?;
        for child in &plan.child_groups {
            let mut child = existing_group(&mut tx, child.id).await?;
            child.update().parent_id(None).exec(&mut tx).await?;
        }
        models::Group::filter_by_id(group_id)
            .delete()
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(Done::ok(plan))
    }
}

async fn all_groups(db: &mut impl toasty::Executor) -> Result<Vec<Group>, Error> {
    Ok(models::Group::all()
        .exec(db)
        .await?
        .into_iter()
        .map(Group::from)
        .collect())
}

/// The Group with this id, or [`Error::NotFound`].
pub(super) async fn existing_group(
    db: &mut impl toasty::Executor,
    group_id: i64,
) -> Result<models::Group, Error> {
    models::Group::filter_by_id(group_id)
        .first()
        .exec(db)
        .await?
        .ok_or_else(|| Error::NotFound(format!("Group {group_id}")))
}

/// A Group name with surrounding whitespace removed, if it isn't empty.
fn valid_group_name(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("name", "A Group name can't be empty."));
    }
    Ok(name)
}

/// Fails unless no Group other than `except` is called `name`.
async fn unused_group_name(
    db: &mut impl toasty::Executor,
    name: &str,
    except: Option<i64>,
) -> Result<(), Error> {
    // Every match, not just the first: cli/si may have left duplicate names.
    let taken = models::Group::filter(models::Group::fields().name().eq(name))
        .exec(db)
        .await?
        .iter()
        .any(|other| Some(other.id) != except);
    if taken {
        return Err(invalid(
            "name",
            format!("A Group named {name} already exists."),
        ));
    }
    Ok(())
}

/// If making `parent_id` the Parent Group of `group_id` would create a
/// cycle, the loop's Group names, starting and ending with `group_id`'s.
/// Walks up from `parent_id`, stopping at any cycle already in the data.
fn cycle_through(
    groups: &HashMap<i64, Group>,
    group_id: i64,
    parent_id: i64,
) -> Option<Vec<String>> {
    let mut path = vec![groups.get(&group_id)?.name.clone()];
    let mut visited = HashSet::new();
    let mut current = Some(parent_id);
    while let Some(id) = current {
        let group = groups.get(&id)?;
        path.push(group.name.clone());
        if id == group_id {
            return Some(path);
        }
        if !visited.insert(id) {
            return None;
        }
        current = group.parent_id;
    }
    None
}

/// The query for one membership row.
fn membership(host_id: i64, group_id: i64) -> toasty::schema::QueryMany<models::Membership> {
    models::Membership::filter(
        models::Membership::fields()
            .host_id()
            .eq(host_id)
            .and(models::Membership::fields().group_id().eq(group_id)),
    )
}

async fn child_groups(db: &mut impl toasty::Executor, group_id: i64) -> Result<Vec<Group>, Error> {
    let mut children: Vec<Group> =
        models::Group::filter(models::Group::fields().parent_id().eq(group_id))
            .exec(db)
            .await?
            .into_iter()
            .map(Group::from)
            .collect();
    children.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(children)
}

pub(super) async fn host_groups(
    db: &mut impl toasty::Executor,
    host_id: i64,
) -> Result<Vec<Group>, Error> {
    let ids: Vec<i64> =
        models::Membership::filter(models::Membership::fields().host_id().eq(host_id))
            .exec(&mut *db)
            .await?
            .into_iter()
            .map(|m| m.group_id)
            .collect();
    let mut groups: Vec<Group> =
        models::Group::filter(toasty::stmt::in_list(models::Group::fields().id(), ids))
            .exec(&mut *db)
            .await?
            .into_iter()
            .map(Group::from)
            .collect();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(groups)
}

async fn group_hosts(db: &mut impl toasty::Executor, group_id: i64) -> Result<Vec<Host>, Error> {
    let ids: Vec<i64> =
        models::Membership::filter(models::Membership::fields().group_id().eq(group_id))
            .exec(&mut *db)
            .await?
            .into_iter()
            .map(|m| m.host_id)
            .collect();
    let mut hosts: Vec<Host> =
        models::Host::filter(toasty::stmt::in_list(models::Host::fields().id(), ids))
            .exec(&mut *db)
            .await?
            .into_iter()
            .map(Host::from)
            .collect();
    hosts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(hosts)
}

/// The shared plan behind [`Inventory::group_deletion`] and
/// [`Inventory::delete_group`], so the preview and the delete can't disagree.
async fn plan_group_deletion(
    db: &mut impl toasty::Executor,
    group_id: i64,
) -> Result<GroupDeletion, Error> {
    let group = existing_group(&mut *db, group_id).await?;
    let member_hosts = group_hosts(&mut *db, group_id).await?;
    let memberships =
        models::Membership::filter(models::Membership::fields().group_id().eq(group_id))
            .exec(&mut *db)
            .await?
            .len();
    let child_groups = child_groups(&mut *db, group_id)
        .await?
        .into_iter()
        .filter(|g| g.id != group_id)
        .collect();
    // TEMPORARY until Group Variables (#11): not modelled yet, so counted
    // with raw SQL, and they block the delete instead of cascading.
    let variables = count_rows(
        &mut *db,
        r#"SELECT count(*) FROM host_group_variable WHERE "group" = ?1"#,
        group_id,
    )
    .await?;
    let mut blockers = Vec::new();
    if variables > 0 {
        blockers.push(format!(
            "It has {}, and lager can't remove Group Variables yet.",
            plural(variables, "Own Variable")
        ));
    }
    Ok(GroupDeletion {
        group: group.into(),
        missing_memberships: memberships - member_hosts.len(),
        member_hosts,
        child_groups,
        blockers,
    })
}
