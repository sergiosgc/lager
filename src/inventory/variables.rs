//! Variables: names in the shared `variable` table, values in one table per
//! kind of entity (ADR 0003), and Inheritance, which turns an entity's Own
//! Variables and its ancestors' into its Effective Variables.

use std::collections::{HashMap, HashSet};

use super::*;

/// A Variable set directly on an entity, with its value as JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnVariable {
    pub name: String,
    pub value: String,
}

/// An entity that can have Own Variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Host(i64),
    Group(i64),
}

/// Where an Effective Variable's value comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The entity's own value.
    Own,
    /// Inherited from this Group.
    Group { id: i64, name: String },
}

/// The value an entity ends up with once Inheritance is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveVariable {
    pub name: String,
    /// The winning value, as JSON text.
    pub value: String,
    pub source: Source,
    /// The next source in line that also sets this Variable, whose value
    /// the winning one hides, if any.
    pub overrides: Option<Source>,
}

/// The Group Variable Ansible uses to rank Groups at the same distance.
const ANSIBLE_GROUP_PRIORITY: &str = "ansible_group_priority";

/// Ansible's `ansible_group_priority` when a Group doesn't set one.
const DEFAULT_GROUP_PRIORITY: i64 = 1;

impl Inventory {
    /// An entity's Own Variables, sorted by name.
    pub async fn own_variables(&self, owner: Owner) -> Result<Vec<OwnVariable>, Error> {
        let mut db = self.db.clone();
        let values = owner_values(&mut db, owner).await?;
        own_variables(&mut db, values, Unnamed::Skip).await
    }

    /// Sets an entity's Own Variable, adding the Variable name if it is new.
    ///
    /// The name must not be blank, and is used exactly as given. The value
    /// must parse as JSON, and is stored exactly as given, as `cli/si` does.
    pub async fn set_variable(
        &self,
        owner: Owner,
        name: &str,
        value: &str,
    ) -> Result<Done<OwnVariable>, Error> {
        valid_variable_name(name)?;
        valid_json("value", value)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_owner(&mut tx, owner).await?;
        write_variable(&mut tx, owner, name, value).await?;
        let warnings = match owner {
            Owner::Host(host_id) => host_warnings(&mut tx, host_id).await?,
            Owner::Group(_) => Vec::new(),
        };
        tx.commit().await?;
        Ok(Done {
            value: OwnVariable {
                name: name.to_string(),
                value: value.to_string(),
            },
            warnings,
        })
    }

    /// Removes an entity's Own Variable. The Variable name stays, for any
    /// other entity using it. Removing one the entity doesn't have does
    /// nothing.
    pub async fn remove_variable(&self, owner: Owner, name: &str) -> Result<Done<()>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_owner(&mut tx, owner).await?;
        if let Some(variable) = variable_by_name(&mut tx, name).await? {
            delete_value(&mut tx, owner, variable.id).await?;
        }
        tx.commit().await?;
        Ok(Done::ok(()))
    }

    /// An entity's Effective Variables, sorted by name, each with where its
    /// value comes from.
    ///
    /// The entity's own value wins. Then come the Groups it inherits from: for
    /// a Host, its Groups and all their Parent Groups; for a Group, its
    /// Parent Groups. They are ranked as Ansible ranks them: the deepest
    /// below the top of the hierarchy first, so a child Group always wins
    /// over its Parent Group; at the same depth, the highest
    /// `ansible_group_priority`; then the last by name.
    pub async fn effective_variables(&self, owner: Owner) -> Result<Vec<EffectiveVariable>, Error> {
        let mut db = self.db.clone();
        existing_owner(&mut db, owner).await?;
        let groups: HashMap<i64, Group> = all_groups(&mut db)
            .await?
            .into_iter()
            .map(|g| (g.id, g))
            .collect();
        let ancestors = match owner {
            Owner::Host(host_id) => {
                let direct =
                    models::Membership::filter(models::Membership::fields().host_id().eq(host_id))
                        .exec(&mut db)
                        .await?
                        .into_iter()
                        .map(|m| m.group_id)
                        .collect();
                with_ancestors(&groups, direct)
            }
            Owner::Group(group_id) => {
                let parent = groups.get(&group_id).and_then(|g| g.parent_id);
                with_ancestors(&groups, parent.into_iter().collect())
            }
        };
        let group_values = group_values(&mut db, ancestors.iter().copied()).await?;
        let ancestors = rank_by_priority(&mut db, &groups, ancestors, &group_values).await?;

        let mut layers = vec![(Source::Own, owner_values(&mut db, owner).await?)];
        for id in ancestors {
            let group = &groups[&id];
            let source = Source::Group {
                id,
                name: group.name.clone(),
            };
            layers.push((source, group_values.get(&id).cloned().unwrap_or_default()));
        }
        resolve(&mut db, layers).await
    }

    /// Every Variable name, sorted, whether or not any entity has a value
    /// for it.
    pub async fn variable_names(&self) -> Result<Vec<String>, Error> {
        let mut db = self.db.clone();
        let mut names: Vec<String> = models::Variable::all()
            .exec(&mut db)
            .await?
            .into_iter()
            .map(|v| v.name)
            .collect();
        names.sort();
        Ok(names)
    }
}

/// The Groups in `start` and all their Parent Groups, each once. Stops at
/// cycles already in the data, and ignores Groups that don't exist.
fn with_ancestors(groups: &HashMap<i64, Group>, start: Vec<i64>) -> Vec<i64> {
    let mut seen = HashSet::new();
    let mut pending = start;
    while let Some(id) = pending.pop() {
        if groups.contains_key(&id) && seen.insert(id) {
            pending.extend(groups[&id].parent_id);
        }
    }
    seen.into_iter().collect()
}

/// How many levels a Group sits below the top of the hierarchy: 0 for a
/// top-level Group. A Group caught in a cycle counts the levels until the
/// walk up repeats.
fn depth(groups: &HashMap<i64, Group>, group_id: i64) -> usize {
    let mut seen = HashSet::from([group_id]);
    let mut depth = 0;
    let mut current = groups.get(&group_id).and_then(|g| g.parent_id);
    while let Some(id) = current.filter(|id| groups.contains_key(id) && seen.insert(*id)) {
        depth += 1;
        current = groups[&id].parent_id;
    }
    depth
}

/// A Group's `ansible_group_priority`, read the way Ansible's `int()` reads
/// it: an integer, a float (truncated) or a string holding an integer.
/// Anything else counts as the default.
fn read_priority(value: &str) -> i64 {
    match parse_json(value) {
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64))
            .unwrap_or(DEFAULT_GROUP_PRIORITY),
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(DEFAULT_GROUP_PRIORITY),
        _ => DEFAULT_GROUP_PRIORITY,
    }
}

/// Orders Groups by precedence, as Ansible does: deepest first; at the same
/// depth, the highest `ansible_group_priority` first; then the last by name
/// first.
async fn rank_by_priority(
    db: &mut impl toasty::Executor,
    groups: &HashMap<i64, Group>,
    ancestors: Vec<i64>,
    group_values: &HashMap<i64, Vec<(i64, String)>>,
) -> Result<Vec<i64>, Error> {
    let priority_id = variable_by_name(db, ANSIBLE_GROUP_PRIORITY)
        .await?
        .map(|v| v.id);
    let priority = |group_id: i64| -> i64 {
        group_values
            .get(&group_id)
            .and_then(|values| values.iter().find(|(v, _)| Some(*v) == priority_id))
            .map_or(DEFAULT_GROUP_PRIORITY, |(_, value)| read_priority(value))
    };
    let mut ranked = ancestors;
    ranked.sort_by(|a, b| {
        depth(groups, *b)
            .cmp(&depth(groups, *a))
            .then(priority(*b).cmp(&priority(*a)))
            .then(groups[b].name.cmp(&groups[a].name))
    });
    Ok(ranked)
}

/// The Own Variable values of each of these Groups, as `(variable id,
/// value)`, keyed by Group id.
async fn group_values(
    db: &mut impl toasty::Executor,
    group_ids: impl Iterator<Item = i64>,
) -> Result<HashMap<i64, Vec<(i64, String)>>, Error> {
    let ids: Vec<i64> = group_ids.collect();
    let mut values: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
    for row in models::GroupVariable::filter(toasty::stmt::in_list(
        models::GroupVariable::fields().group_id(),
        ids,
    ))
    .exec(db)
    .await?
    {
        values
            .entry(row.group_id)
            .or_default()
            .push((row.variable_id, row.value));
    }
    Ok(values)
}

/// Picks, for each Variable, the value from the first layer that sets it,
/// noting the next layer it hides. Layers are in precedence order.
async fn resolve(
    db: &mut impl toasty::Executor,
    layers: Vec<(Source, Vec<(i64, String)>)>,
) -> Result<Vec<EffectiveVariable>, Error> {
    let ids: HashSet<i64> = layers
        .iter()
        .flat_map(|(_, values)| values.iter().map(|(id, _)| *id))
        .collect();
    let names = variable_names_by_id(db, ids.into_iter().collect()).await?;
    let mut resolved: HashMap<i64, EffectiveVariable> = HashMap::new();
    for (source, values) in layers {
        for (variable_id, value) in values {
            // A value whose Variable name is missing is skipped, as cli/si's
            // joins do.
            let Some(name) = names.get(&variable_id) else {
                continue;
            };
            match resolved.get_mut(&variable_id) {
                None => {
                    resolved.insert(
                        variable_id,
                        EffectiveVariable {
                            name: name.clone(),
                            value,
                            source: source.clone(),
                            overrides: None,
                        },
                    );
                }
                Some(winner) if winner.overrides.is_none() => {
                    winner.overrides = Some(source.clone());
                }
                Some(_) => {}
            }
        }
    }
    let mut effective: Vec<EffectiveVariable> = resolved.into_values().collect();
    effective.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(effective)
}

/// Fails with [`Error::NotFound`] unless the entity exists.
pub(super) async fn existing_owner(
    db: &mut impl toasty::Executor,
    owner: Owner,
) -> Result<(), Error> {
    match owner {
        Owner::Host(id) => existing_host(db, id).await.map(|_| ()),
        Owner::Group(id) => existing_group(db, id).await.map(|_| ()),
    }
}

/// An entity's Own Variable values, as `(variable id, value)`.
async fn owner_values(
    db: &mut impl toasty::Executor,
    owner: Owner,
) -> Result<Vec<(i64, String)>, Error> {
    Ok(match owner {
        Owner::Host(id) => {
            models::HostVariable::filter(models::HostVariable::fields().host_id().eq(id))
                .exec(db)
                .await?
                .into_iter()
                .map(|v| (v.variable_id, v.value))
                .collect()
        }
        Owner::Group(id) => {
            models::GroupVariable::filter(models::GroupVariable::fields().group_id().eq(id))
                .exec(db)
                .await?
                .into_iter()
                .map(|v| (v.variable_id, v.value))
                .collect()
        }
    })
}

/// An entity's Own Variables for a delete preview, which must list every
/// value the delete removes, including those whose name is missing.
pub(super) async fn own_variables_to_delete(
    db: &mut impl toasty::Executor,
    owner: Owner,
) -> Result<Vec<OwnVariable>, Error> {
    let values = owner_values(&mut *db, owner).await?;
    own_variables(db, values, Unnamed::Label).await
}

/// Removes every Own Variable value of an entity (the names stay).
pub(super) async fn delete_owner_values(
    tx: &mut toasty::Transaction<'_>,
    owner: Owner,
) -> Result<(), Error> {
    match owner {
        Owner::Host(id) => {
            models::HostVariable::filter(models::HostVariable::fields().host_id().eq(id))
                .delete()
                .exec(tx)
                .await?
        }
        Owner::Group(id) => {
            models::GroupVariable::filter(models::GroupVariable::fields().group_id().eq(id))
                .delete()
                .exec(tx)
                .await?
        }
    }
    Ok(())
}

async fn delete_value(
    tx: &mut toasty::Transaction<'_>,
    owner: Owner,
    variable_id: i64,
) -> Result<(), Error> {
    match owner {
        Owner::Host(id) => host_variable(variable_id, id).delete().exec(tx).await?,
        Owner::Group(id) => group_variable(variable_id, id).delete().exec(tx).await?,
    }
    Ok(())
}

/// What to do with a value whose Variable row is missing (possible with
/// foreign keys off, ADR 0003).
#[derive(Clone, Copy)]
enum Unnamed {
    /// Leave it out, as `cli/si`'s joins do.
    Skip,
    /// List it under a placeholder name, e.g. for a delete preview, which
    /// must show everything the delete removes.
    Label,
}

async fn variable_names_by_id(
    db: &mut impl toasty::Executor,
    ids: Vec<i64>,
) -> Result<HashMap<i64, String>, Error> {
    Ok(
        models::Variable::filter(toasty::stmt::in_list(models::Variable::fields().id(), ids))
            .exec(db)
            .await?
            .into_iter()
            .map(|v| (v.id, v.name))
            .collect(),
    )
}

/// Pairs each `(variable id, value)` with its Variable name, sorted by name.
async fn own_variables(
    db: &mut impl toasty::Executor,
    values: Vec<(i64, String)>,
    unnamed: Unnamed,
) -> Result<Vec<OwnVariable>, Error> {
    let names = variable_names_by_id(db, values.iter().map(|(id, _)| *id).collect()).await?;
    let mut own: Vec<OwnVariable> = values
        .into_iter()
        .filter_map(|(id, value)| {
            let name = match (names.get(&id), unnamed) {
                (Some(name), _) => name.clone(),
                (None, Unnamed::Label) => format!("(Variable {id}, name missing)"),
                (None, Unnamed::Skip) => return None,
            };
            Some(OwnVariable { name, value })
        })
        .collect();
    own.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(own)
}

/// Sets an entity's value of the Variable `name`, adding the name if it is
/// new. The caller has checked the inputs and that the entity exists.
pub(super) async fn write_variable(
    tx: &mut toasty::Transaction<'_>,
    owner: Owner,
    name: &str,
    value: &str,
) -> Result<(), Error> {
    let variable_id = match variable_by_name(&mut *tx, name).await? {
        Some(variable) => variable.id,
        None => {
            models::Variable::create()
                .name(name)
                .exec(&mut *tx)
                .await?
                .id
        }
    };
    match owner {
        Owner::Host(id) => match host_variable(variable_id, id)
            .first()
            .exec(&mut *tx)
            .await?
        {
            Some(mut row) => row.update().value(value).exec(&mut *tx).await?,
            None => {
                models::HostVariable::create()
                    .variable_id(variable_id)
                    .host_id(id)
                    .value(value)
                    .exec(&mut *tx)
                    .await?;
            }
        },
        Owner::Group(id) => match group_variable(variable_id, id)
            .first()
            .exec(&mut *tx)
            .await?
        {
            Some(mut row) => row.update().value(value).exec(&mut *tx).await?,
            None => {
                models::GroupVariable::create()
                    .variable_id(variable_id)
                    .group_id(id)
                    .value(value)
                    .exec(&mut *tx)
                    .await?;
            }
        },
    }
    Ok(())
}

/// The Variable called exactly `name`, if there is one.
pub(super) async fn variable_by_name(
    db: &mut impl toasty::Executor,
    name: &str,
) -> Result<Option<models::Variable>, Error> {
    Ok(
        models::Variable::filter(models::Variable::fields().name().eq(name))
            .first()
            .exec(db)
            .await?,
    )
}

/// The query for one Host's value of one Variable.
fn host_variable(
    variable_id: i64,
    host_id: i64,
) -> toasty::schema::QueryMany<models::HostVariable> {
    models::HostVariable::filter(
        models::HostVariable::fields()
            .variable_id()
            .eq(variable_id)
            .and(models::HostVariable::fields().host_id().eq(host_id)),
    )
}

/// The query for one Group's value of one Variable.
fn group_variable(
    variable_id: i64,
    group_id: i64,
) -> toasty::schema::QueryMany<models::GroupVariable> {
    models::GroupVariable::filter(
        models::GroupVariable::fields()
            .variable_id()
            .eq(variable_id)
            .and(models::GroupVariable::fields().group_id().eq(group_id)),
    )
}

/// Rejects a blank Variable name.
fn valid_variable_name(name: &str) -> Result<(), Error> {
    if name.trim().is_empty() {
        return Err(invalid("name", "A Variable name can't be empty."));
    }
    Ok(())
}

/// The JSON document in a stored Variable value, if it is one.
pub(super) fn parse_json(value: &str) -> Option<serde_json::Value> {
    serde_json::from_str(value).ok()
}

/// Checks that a Variable value, submitted as `field`, is a JSON document.
pub(super) fn valid_json(field: &'static str, value: &str) -> Result<(), Error> {
    serde_json::from_str::<serde_json::Value>(value)
        .map(|_| ())
        .map_err(|e| invalid(field, format!("This is not valid JSON: {e}.")))
}
