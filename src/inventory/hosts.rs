//! Hosts: listing, creating, renaming and deleting them.

use super::*;

/// A machine Ansible manages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: i64,
    pub name: String,
}

impl From<models::Host> for Host {
    fn from(h: models::Host) -> Self {
        Host {
            id: h.id,
            name: h.name,
        }
    }
}

/// What deleting a Host removes: the Host, its Own Variables and its Group
/// memberships. While `blockers` isn't empty, the Host can't be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostDeletion {
    pub host: Host,
    pub own_variables: Vec<OwnVariable>,
    /// The Groups the Host stops being a member of (the Groups stay).
    pub groups: Vec<Group>,
    /// Memberships of Groups that no longer exist, left behind by `cli/si`;
    /// the delete removes them too.
    pub missing_memberships: usize,
    /// Relations lager can't remove yet (Service Instances), each as a
    /// sentence for the operator.
    pub blockers: Vec<String>,
}

impl Inventory {
    /// All Hosts, sorted by name.
    pub async fn hosts(&self) -> Result<Vec<Host>, Error> {
        let mut db = self.db.clone();
        let mut hosts: Vec<Host> = models::Host::all()
            .exec(&mut db)
            .await?
            .into_iter()
            .map(Host::from)
            .collect();
        hosts.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(hosts)
    }

    /// The Host with this id, if there is one.
    pub async fn host(&self, id: i64) -> Result<Option<Host>, Error> {
        let mut db = self.db.clone();
        let host = models::Host::filter_by_id(id).first().exec(&mut db).await?;
        Ok(host.map(Host::from))
    }

    /// Creates a Host with its Server Index. Its name is normalised (see
    /// [`normalize_host_name`]) and must then be non-empty and not already
    /// used by another Host.
    ///
    /// Without `server_index`, or with a blank one, the Host gets
    /// [`Inventory::next_server_index`].
    /// An override must parse as JSON and is stored as given; if another
    /// Host already has it, the Host is still created, with a warning.
    pub async fn create_host(
        &self,
        name: &str,
        server_index: Option<&str>,
    ) -> Result<Done<Host>, Error> {
        let name = normalized_host_name(name)?;
        let server_index = server_index.filter(|v| !v.trim().is_empty());
        if let Some(value) = server_index {
            valid_json(SERVER_INDEX, value)?;
        }
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        unused_host_name(&mut tx, &name, None).await?;
        let host = models::Host::create().name(name).exec(&mut tx).await?;
        let index = match server_index {
            Some(value) => value.to_string(),
            None => next_server_index(&mut tx).await?.to_string(),
        };
        write_variable(&mut tx, Owner::Host(host.id), SERVER_INDEX, &index).await?;
        let warnings = host_warnings(&mut tx, host.id).await?;
        tx.commit().await?;
        Ok(Done {
            value: host.into(),
            warnings,
        })
    }

    /// Renames a Host, keeping its identity, so everything attached to it
    /// stays attached. The new name follows the same rules as on creation.
    pub async fn rename_host(&self, host_id: i64, name: &str) -> Result<Done<Host>, Error> {
        let name = normalized_host_name(name)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let mut host = existing_host(&mut tx, host_id).await?;
        unused_host_name(&mut tx, &name, Some(host_id)).await?;
        host.update().name(name).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(host.into()))
    }

    /// What deleting a Host would remove, or why it can't be deleted yet.
    /// [`Inventory::delete_host`] removes exactly this.
    pub async fn host_deletion(&self, host_id: i64) -> Result<HostDeletion, Error> {
        plan_host_deletion(&mut self.db.clone(), host_id).await
    }

    /// Deletes a Host and everything listed by [`Inventory::host_deletion`],
    /// in one transaction, returning what was removed. Variable names stay.
    /// Refused while the Host has relations lager can't remove yet.
    pub async fn delete_host(&self, host_id: i64) -> Result<Done<HostDeletion>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let plan = plan_host_deletion(&mut tx, host_id).await?;
        refuse_if_blocked("host", &plan.host.name, &plan.blockers)?;
        delete_owner_values(&mut tx, Owner::Host(host_id)).await?;
        models::Membership::filter(models::Membership::fields().host_id().eq(host_id))
            .delete()
            .exec(&mut tx)
            .await?;
        models::Host::filter_by_id(host_id)
            .delete()
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(Done::ok(plan))
    }
}

/// The Host with this id, or [`Error::NotFound`].
pub(super) async fn existing_host(
    db: &mut impl toasty::Executor,
    host_id: i64,
) -> Result<models::Host, Error> {
    models::Host::filter_by_id(host_id)
        .first()
        .exec(db)
        .await?
        .ok_or_else(|| Error::NotFound(format!("Host {host_id}")))
}

/// A Host name after [`normalize_host_name`], if it isn't empty.
pub(super) fn normalized_host_name(name: &str) -> Result<String, Error> {
    let name = normalize_host_name(name);
    if name.is_empty() {
        return Err(invalid(
            "name",
            "A Host name can't be empty: it needs at least one letter, digit, \"-\" or \"_\".",
        ));
    }
    Ok(name)
}

/// Fails unless no Host other than `except` is called `name`.
pub(super) async fn unused_host_name(
    db: &mut impl toasty::Executor,
    name: &str,
    except: Option<i64>,
) -> Result<(), Error> {
    // Every match, not just the first: cli/si may have left duplicate names.
    let taken = models::Host::filter(models::Host::fields().name().eq(name))
        .exec(db)
        .await?
        .iter()
        .any(|other| Some(other.id) != except);
    if taken {
        return Err(invalid(
            "name",
            format!("A Host named {name} already exists."),
        ));
    }
    Ok(())
}

/// The shared plan behind [`Inventory::host_deletion`] and
/// [`Inventory::delete_host`], so the preview and the delete can't disagree.
pub(super) async fn plan_host_deletion(
    db: &mut impl toasty::Executor,
    host_id: i64,
) -> Result<HostDeletion, Error> {
    let host = existing_host(&mut *db, host_id).await?;
    let own_variables = own_variables_to_delete(&mut *db, Owner::Host(host_id)).await?;
    let groups = host_groups(&mut *db, host_id).await?;
    let memberships =
        models::Membership::filter(models::Membership::fields().host_id().eq(host_id))
            .exec(&mut *db)
            .await?
            .len();
    // TEMPORARY until lager manages Service Instances (#13): not modelled
    // yet, so counted with raw SQL, and they block the delete instead of
    // cascading.
    let instances = count_rows(
        &mut *db,
        "SELECT count(*) FROM service_instance WHERE host = ?1",
        host_id,
    )
    .await?;
    let mut blockers = Vec::new();
    if instances > 0 {
        blockers.push(format!(
            "It has {}, and lager can't delete Service Instances yet.",
            plural(instances, "Service Instance")
        ));
    }
    Ok(HostDeletion {
        host: host.into(),
        own_variables,
        missing_memberships: memberships - groups.len(),
        groups,
        blockers,
    })
}

/// The name a Host is stored under: lowercased, keeping only `a`-`z`,
/// `0`-`9`, `-` and `_`. Anything else, including spaces, dots and
/// non-ASCII letters, is dropped.
pub fn normalize_host_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| matches!(c, 'a'..='z' | '0'..='9' | '-' | '_'))
        .collect()
}
