//! Own Variables: names in the shared `variable` table, values in one
//! table per kind of entity (ADR 0003).

use super::*;

/// A Variable set directly on an entity, with its value as JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnVariable {
    pub name: String,
    pub value: String,
}

impl Inventory {
    /// A Host's Own Variables, sorted by name.
    pub async fn host_variables(&self, host_id: i64) -> Result<Vec<OwnVariable>, Error> {
        let mut db = self.db.clone();
        let values =
            models::HostVariable::filter(models::HostVariable::fields().host_id().eq(host_id))
                .exec(&mut db)
                .await?;
        own_variables(
            &mut db,
            values
                .into_iter()
                .map(|v| (v.variable_id, v.value))
                .collect(),
            Unnamed::Skip,
        )
        .await
    }

    /// Sets a Host's Own Variable, adding the Variable name if it is new.
    ///
    /// The name must not be blank, and is used exactly as given. The value
    /// must parse as JSON, and is stored exactly as given, as `cli/si` does.
    pub async fn set_host_variable(
        &self,
        host_id: i64,
        name: &str,
        value: &str,
    ) -> Result<Done<OwnVariable>, Error> {
        valid_variable_name(name)?;
        valid_json("value", value)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        write_host_variable(&mut tx, host_id, name, value).await?;
        let warnings = host_warnings(&mut tx, host_id).await?;
        tx.commit().await?;
        Ok(Done {
            value: OwnVariable {
                name: name.to_string(),
                value: value.to_string(),
            },
            warnings,
        })
    }

    /// Removes a Host's Own Variable. The Variable name stays, for any other
    /// entity using it. Removing a Variable the Host doesn't have does nothing.
    pub async fn remove_host_variable(&self, host_id: i64, name: &str) -> Result<Done<()>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        if let Some(variable) = variable_by_name(&mut tx, name).await? {
            host_variable(variable.id, host_id)
                .delete()
                .exec(&mut tx)
                .await?;
        }
        tx.commit().await?;
        Ok(Done::ok(()))
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

/// What to do with a value whose Variable row is missing (possible with
/// foreign keys off, ADR 0003).
#[derive(Clone, Copy)]
pub(super) enum Unnamed {
    /// Leave it out, as `cli/si`'s joins do.
    Skip,
    /// List it under a placeholder name, e.g. for a delete preview, which
    /// must show everything the delete removes.
    Label,
}

/// Pairs each `(variable id, value)` with its Variable name, sorted by name.
pub(super) async fn own_variables(
    db: &mut impl toasty::Executor,
    values: Vec<(i64, String)>,
    unnamed: Unnamed,
) -> Result<Vec<OwnVariable>, Error> {
    let ids: Vec<i64> = values.iter().map(|(id, _)| *id).collect();
    let names: std::collections::HashMap<i64, String> =
        models::Variable::filter(toasty::stmt::in_list(models::Variable::fields().id(), ids))
            .exec(db)
            .await?
            .into_iter()
            .map(|v| (v.id, v.name))
            .collect();
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

/// Sets a Host's value of the Variable `name`, adding the name if it is new.
/// The caller has checked the inputs and that the Host exists.
pub(super) async fn write_host_variable(
    tx: &mut toasty::Transaction<'_>,
    host_id: i64,
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
    match host_variable(variable_id, host_id)
        .first()
        .exec(&mut *tx)
        .await?
    {
        Some(mut row) => row.update().value(value).exec(&mut *tx).await?,
        None => {
            models::HostVariable::create()
                .variable_id(variable_id)
                .host_id(host_id)
                .value(value)
                .exec(&mut *tx)
                .await?;
        }
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
pub(super) fn host_variable(
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

/// Rejects a blank Variable name.
pub(super) fn valid_variable_name(name: &str) -> Result<(), Error> {
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
