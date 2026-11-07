//! Services: their hierarchy (Parent Services). A Service's Variables pass
//! down to the Services below it (see [`variables`](super::variables)).

use std::collections::HashMap;

use super::*;

/// A kind of software that can be deployed. `parent_id` is its Parent
/// Service, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
}

impl Node for Service {
    fn id(&self) -> i64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn parent_id(&self) -> Option<i64> {
        self.parent_id
    }
}

impl From<models::Service> for Service {
    fn from(s: models::Service) -> Self {
        Service {
            id: s.id,
            name: s.name,
            parent_id: s.parent_id,
        }
    }
}

/// One Service in the hierarchy, `depth` levels below a top-level Service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceTreeEntry {
    pub depth: usize,
    pub service: Service,
}

/// What deleting a Service does: removes it and its Own Variables, and
/// clears the Parent Service of its child Services (which stay). While
/// `blockers` isn't empty, the Service can't be deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDeletion {
    pub service: Service,
    pub own_variables: Vec<OwnVariable>,
    /// Services that lose their Parent Service (and become top-level).
    pub child_services: Vec<Service>,
    /// Relations lager can't remove yet, each as a sentence for the operator.
    pub blockers: Vec<String>,
}

impl Inventory {
    /// All Services, sorted by name.
    pub async fn services(&self) -> Result<Vec<Service>, Error> {
        let mut services = all_services(&mut self.db.clone()).await?;
        services.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(services)
    }

    /// The Service with this id, if there is one.
    pub async fn service(&self, id: i64) -> Result<Option<Service>, Error> {
        let service = models::Service::filter_by_id(id)
            .first()
            .exec(&mut self.db.clone())
            .await?;
        Ok(service.map(Service::from))
    }

    /// Every Service in hierarchy order: each top-level Service followed by
    /// the Services below it, depth first, siblings by name.
    pub async fn service_tree(&self) -> Result<Vec<ServiceTreeEntry>, Error> {
        let services = all_services(&mut self.db.clone()).await?;
        Ok(tree_order(&services)
            .into_iter()
            .map(|(depth, service)| ServiceTreeEntry { depth, service })
            .collect())
    }

    /// The Services whose Parent Service is this one, sorted by name.
    pub async fn child_services(&self, service_id: i64) -> Result<Vec<Service>, Error> {
        child_services(&mut self.db.clone(), service_id).await
    }

    /// Creates a top-level Service. Its name, with surrounding whitespace
    /// removed, must be non-empty and not already used by another Service.
    pub async fn create_service(&self, name: &str) -> Result<Done<Service>, Error> {
        let name = valid_service_name(name)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        unused_service_name(&mut tx, name, None).await?;
        let service = models::Service::create().name(name).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(service.into()))
    }

    /// Renames a Service, keeping its identity, so everything attached to it
    /// stays attached. The new name follows the creation rules.
    pub async fn rename_service(
        &self,
        service_id: i64,
        name: &str,
    ) -> Result<Done<Service>, Error> {
        let name = valid_service_name(name)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let mut service = existing_service(&mut tx, service_id).await?;
        unused_service_name(&mut tx, name, Some(service_id)).await?;
        service.update().name(name).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(service.into()))
    }

    /// Sets or clears a Service's Parent Service. A parent that is the
    /// Service itself, or one below it, would create a cycle and is
    /// rejected, naming the loop.
    pub async fn set_service_parent(
        &self,
        service_id: i64,
        parent_id: Option<i64>,
    ) -> Result<Done<Service>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let mut service = existing_service(&mut tx, service_id).await?;
        if let Some(parent_id) = parent_id {
            existing_service(&mut tx, parent_id).await?;
            let services: HashMap<i64, Service> = all_services(&mut tx)
                .await?
                .into_iter()
                .map(|s| (s.id, s))
                .collect();
            if let Some(path) = cycle_through(&services, service_id, parent_id) {
                return Err(invalid(
                    "parent",
                    format!(
                        "A Service can't be its own Parent Service: {}.",
                        path.join(" → ")
                    ),
                ));
            }
        }
        service.update().parent_id(parent_id).exec(&mut tx).await?;
        tx.commit().await?;
        Ok(Done::ok(service.into()))
    }

    /// What deleting a Service would do, or why it can't be deleted yet.
    /// [`Inventory::delete_service`] does exactly this.
    pub async fn service_deletion(&self, service_id: i64) -> Result<ServiceDeletion, Error> {
        plan_service_deletion(&mut self.db.clone(), service_id).await
    }

    /// Deletes a Service as listed by [`Inventory::service_deletion`], in one
    /// transaction: its Own Variables go, its child Services become
    /// top-level. Variable names stay.
    pub async fn delete_service(&self, service_id: i64) -> Result<Done<ServiceDeletion>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let plan = plan_service_deletion(&mut tx, service_id).await?;
        refuse_if_blocked("service", &plan.service.name, &plan.blockers)?;
        delete_owner_values(&mut tx, Owner::Service(service_id)).await?;
        for child in &plan.child_services {
            let mut child = existing_service(&mut tx, child.id).await?;
            child.update().parent_id(None).exec(&mut tx).await?;
        }
        models::Service::filter_by_id(service_id)
            .delete()
            .exec(&mut tx)
            .await?;
        tx.commit().await?;
        Ok(Done::ok(plan))
    }
}

pub(super) async fn all_services(db: &mut impl toasty::Executor) -> Result<Vec<Service>, Error> {
    Ok(models::Service::all()
        .exec(db)
        .await?
        .into_iter()
        .map(Service::from)
        .collect())
}

/// The Service with this id, or [`Error::NotFound`].
pub(super) async fn existing_service(
    db: &mut impl toasty::Executor,
    service_id: i64,
) -> Result<models::Service, Error> {
    models::Service::filter_by_id(service_id)
        .first()
        .exec(db)
        .await?
        .ok_or_else(|| Error::NotFound(format!("Service {service_id}")))
}

/// A Service name with surrounding whitespace removed, if it isn't empty.
fn valid_service_name(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("name", "A Service name can't be empty."));
    }
    Ok(name)
}

/// Fails unless no Service other than `except` is called `name`.
async fn unused_service_name(
    db: &mut impl toasty::Executor,
    name: &str,
    except: Option<i64>,
) -> Result<(), Error> {
    // Every match, not just the first: cli/si may have left duplicate names.
    let taken = models::Service::filter(models::Service::fields().name().eq(name))
        .exec(db)
        .await?
        .iter()
        .any(|other| Some(other.id) != except);
    if taken {
        return Err(invalid(
            "name",
            format!("A Service named {name} already exists."),
        ));
    }
    Ok(())
}

async fn child_services(
    db: &mut impl toasty::Executor,
    service_id: i64,
) -> Result<Vec<Service>, Error> {
    let mut children: Vec<Service> =
        models::Service::filter(models::Service::fields().parent_id().eq(service_id))
            .exec(db)
            .await?
            .into_iter()
            .map(Service::from)
            .filter(|s| s.id != service_id)
            .collect();
    children.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(children)
}

/// The shared plan behind [`Inventory::service_deletion`] and
/// [`Inventory::delete_service`], so the preview and the delete can't
/// disagree.
async fn plan_service_deletion(
    db: &mut impl toasty::Executor,
    service_id: i64,
) -> Result<ServiceDeletion, Error> {
    let service = existing_service(&mut *db, service_id).await?;
    let own_variables = own_variables_to_delete(&mut *db, Owner::Service(service_id)).await?;
    let child_services = child_services(&mut *db, service_id).await?;
    // TEMPORARY until lager manages Service Instances (#13): not modelled
    // yet, so counted with raw SQL, and they block the delete instead of
    // cascading.
    let instances = count_rows(
        &mut *db,
        "SELECT count(*) FROM service_instance WHERE service = ?1",
        service_id,
    )
    .await?;
    let mut blockers = Vec::new();
    if instances > 0 {
        blockers.push(format!(
            "It has {}, and lager can't delete Service Instances yet.",
            plural(instances, "Service Instance")
        ));
    }
    Ok(ServiceDeletion {
        service: service.into(),
        own_variables,
        child_services,
        blockers,
    })
}
