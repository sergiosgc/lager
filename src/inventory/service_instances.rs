//! Service Instances: where each Service is deployed, at which IP, and how
//! each Service Instance is configured. A Service Instance is identified by its
//! (Service, Host, Instance Name).

use std::collections::HashMap;
use std::net::IpAddr;

use super::*;

/// One deployment of a Service on a Host, reachable at its own IP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInstance {
    pub id: i64,
    pub service_id: i64,
    /// The Service's name, or a placeholder if the Service no longer
    /// exists (`cli/si` leaves such rows behind, ADR 0003).
    pub service_name: String,
    pub service_exists: bool,
    pub host_id: i64,
    /// The Host's name, or a placeholder if the Host no longer exists.
    pub host_name: String,
    pub host_exists: bool,
    /// The Instance Name.
    pub name: String,
    /// The IP, as submitted.
    pub ip: String,
}

impl ServiceInstance {
    /// How the operator recognises it: "nginx main on web1".
    pub fn label(&self) -> String {
        format!("{} {} on {}", self.service_name, self.name, self.host_name)
    }
}

/// What deleting a Service Instance removes: it and its Own Variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInstanceDeletion {
    pub instance: ServiceInstance,
    pub own_variables: Vec<OwnVariable>,
}

impl Inventory {
    /// The Service Instance with this id, if there is one.
    pub async fn service_instance(&self, id: i64) -> Result<Option<ServiceInstance>, Error> {
        let mut db = self.db.clone();
        let rows = models::ServiceInstance::filter_by_id(id)
            .exec(&mut db)
            .await?;
        Ok(describe(&mut db, rows).await?.into_iter().next())
    }

    /// A Host's Service Instances, by Service name, then Instance Name.
    pub async fn instances_on_host(&self, host_id: i64) -> Result<Vec<ServiceInstance>, Error> {
        instances_on_host(&mut self.db.clone(), host_id).await
    }

    /// A Service's Service Instances, by Host name, then Instance Name.
    pub async fn instances_of_service(
        &self,
        service_id: i64,
    ) -> Result<Vec<ServiceInstance>, Error> {
        instances_of_service(&mut self.db.clone(), service_id).await
    }

    /// Records a Service deployed on a Host. The Instance Name, with
    /// surrounding whitespace removed, must be non-empty, and no other
    /// Service Instance of that Service on that Host may have it. The IP
    /// must be an IPv4 or IPv6 address; it is stored as submitted, less
    /// surrounding whitespace.
    pub async fn create_service_instance(
        &self,
        host_id: i64,
        service_id: i64,
        name: &str,
        ip: &str,
    ) -> Result<Done<ServiceInstance>, Error> {
        let name = valid_instance_name(name)?;
        let ip = valid_ip(ip)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        existing_host(&mut tx, host_id).await?;
        existing_service(&mut tx, service_id).await?;
        unused_identity(&mut tx, "name", service_id, host_id, name, None).await?;
        let row = models::ServiceInstance::create()
            .service_id(service_id)
            .host_id(host_id)
            .name(name)
            .ip(ip)
            .exec(&mut tx)
            .await?;
        let instance = describe(&mut tx, vec![row]).await?.remove(0);
        tx.commit().await?;
        Ok(Done::ok(instance))
    }

    /// Changes a Service Instance's Instance Name and IP, by the same rules
    /// as on creation. Its Own Variables stay attached.
    pub async fn update_service_instance(
        &self,
        id: i64,
        name: &str,
        ip: &str,
    ) -> Result<Done<ServiceInstance>, Error> {
        let name = valid_instance_name(name)?;
        let ip = valid_ip(ip)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let mut row = existing_instance(&mut tx, id).await?;
        unused_identity(&mut tx, "name", row.service_id, row.host_id, name, Some(id)).await?;
        row.update().name(name).ip(ip).exec(&mut tx).await?;
        let instance = describe(&mut tx, vec![row]).await?.remove(0);
        tx.commit().await?;
        Ok(Done::ok(instance))
    }

    /// Clones a Service Instance onto another Host, as `cli/si`'s
    /// `service_instance clone` does: same Service and Instance Name, the
    /// given IP, and a copy of the source's Own Variables, which from then
    /// on change independently. Refused if the target Host already has
    /// that Service with that Instance Name.
    pub async fn clone_service_instance(
        &self,
        source_id: i64,
        target_host_id: i64,
        ip: &str,
    ) -> Result<Done<ServiceInstance>, Error> {
        let ip = valid_ip(ip)?;
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let source = existing_instance(&mut tx, source_id).await?;
        // An orphaned Service Instance (its Service gone) isn't cloned, as
        // cli/si's clone inserts nothing then.
        existing_service(&mut tx, source.service_id).await?;
        existing_host(&mut tx, target_host_id).await?;
        unused_identity(
            &mut tx,
            "host",
            source.service_id,
            target_host_id,
            &source.name,
            None,
        )
        .await?;
        let row = models::ServiceInstance::create()
            .service_id(source.service_id)
            .host_id(target_host_id)
            .name(source.name.clone())
            .ip(ip)
            .exec(&mut tx)
            .await?;
        copy_owner_values(
            &mut tx,
            Owner::ServiceInstance(source_id),
            Owner::ServiceInstance(row.id),
        )
        .await?;
        let clone = describe(&mut tx, vec![row]).await?.remove(0);
        tx.commit().await?;
        Ok(Done::ok(clone))
    }

    /// What deleting a Service Instance would remove.
    /// [`Inventory::delete_service_instance`] removes exactly this.
    pub async fn service_instance_deletion(
        &self,
        id: i64,
    ) -> Result<ServiceInstanceDeletion, Error> {
        let mut db = self.db.clone();
        let row = existing_instance(&mut db, id).await?;
        let instance = describe(&mut db, vec![row]).await?.remove(0);
        plan_instance_deletion(&mut db, instance).await
    }

    /// Deletes a Service Instance and its Own Variables, in one
    /// transaction, returning what was removed. Variable names stay.
    pub async fn delete_service_instance(
        &self,
        id: i64,
    ) -> Result<Done<ServiceInstanceDeletion>, Error> {
        let mut db = self.db.clone();
        let mut tx = db.transaction().await?;
        let row = existing_instance(&mut tx, id).await?;
        let instance = describe(&mut tx, vec![row]).await?.remove(0);
        let plan = plan_instance_deletion(&mut tx, instance).await?;
        delete_instances(&mut tx, std::slice::from_ref(&plan)).await?;
        tx.commit().await?;
        Ok(Done::ok(plan))
    }
}

/// The Service Instance with this id, or [`Error::NotFound`].
pub(super) async fn existing_instance(
    db: &mut impl toasty::Executor,
    id: i64,
) -> Result<models::ServiceInstance, Error> {
    models::ServiceInstance::filter_by_id(id)
        .first()
        .exec(db)
        .await?
        .ok_or_else(|| Error::NotFound(format!("Service Instance {id}")))
}

pub(super) async fn instances_on_host(
    db: &mut impl toasty::Executor,
    host_id: i64,
) -> Result<Vec<ServiceInstance>, Error> {
    let rows =
        models::ServiceInstance::filter(models::ServiceInstance::fields().host_id().eq(host_id))
            .exec(&mut *db)
            .await?;
    let mut instances = describe(db, rows).await?;
    instances.sort_by(|a, b| (&a.service_name, &a.name).cmp(&(&b.service_name, &b.name)));
    Ok(instances)
}

pub(super) async fn instances_of_service(
    db: &mut impl toasty::Executor,
    service_id: i64,
) -> Result<Vec<ServiceInstance>, Error> {
    let rows = models::ServiceInstance::filter(
        models::ServiceInstance::fields()
            .service_id()
            .eq(service_id),
    )
    .exec(&mut *db)
    .await?;
    let mut instances = describe(db, rows).await?;
    instances.sort_by(|a, b| (&a.host_name, &a.name).cmp(&(&b.host_name, &b.name)));
    Ok(instances)
}

/// What deleting each of these Service Instances removes, in order.
pub(super) async fn plan_instance_deletions(
    db: &mut impl toasty::Executor,
    instances: Vec<ServiceInstance>,
) -> Result<Vec<ServiceInstanceDeletion>, Error> {
    let mut plans = Vec::new();
    for instance in instances {
        plans.push(plan_instance_deletion(&mut *db, instance).await?);
    }
    Ok(plans)
}

async fn plan_instance_deletion(
    db: &mut impl toasty::Executor,
    instance: ServiceInstance,
) -> Result<ServiceInstanceDeletion, Error> {
    let own_variables = own_variables_to_delete(db, Owner::ServiceInstance(instance.id)).await?;
    Ok(ServiceInstanceDeletion {
        instance,
        own_variables,
    })
}

/// Deletes these Service Instances and their Own Variables.
pub(super) async fn delete_instances(
    tx: &mut toasty::Transaction<'_>,
    plans: &[ServiceInstanceDeletion],
) -> Result<(), Error> {
    for plan in plans {
        delete_owner_values(&mut *tx, Owner::ServiceInstance(plan.instance.id)).await?;
        models::ServiceInstance::filter_by_id(plan.instance.id)
            .delete()
            .exec(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Adds the Service and Host names to Service Instance rows.
pub(super) async fn describe(
    db: &mut impl toasty::Executor,
    rows: Vec<models::ServiceInstance>,
) -> Result<Vec<ServiceInstance>, Error> {
    let services: HashMap<i64, String> = models::Service::filter(toasty::stmt::in_list(
        models::Service::fields().id(),
        rows.iter().map(|r| r.service_id).collect::<Vec<_>>(),
    ))
    .exec(&mut *db)
    .await?
    .into_iter()
    .map(|s| (s.id, s.name))
    .collect();
    let hosts: HashMap<i64, String> = models::Host::filter(toasty::stmt::in_list(
        models::Host::fields().id(),
        rows.iter().map(|r| r.host_id).collect::<Vec<_>>(),
    ))
    .exec(&mut *db)
    .await?
    .into_iter()
    .map(|h| (h.id, h.name))
    .collect();
    Ok(rows
        .into_iter()
        .map(|r| ServiceInstance {
            service_exists: services.contains_key(&r.service_id),
            host_exists: hosts.contains_key(&r.host_id),
            service_name: services
                .get(&r.service_id)
                .cloned()
                .unwrap_or_else(|| format!("(Service {}, missing)", r.service_id)),
            host_name: hosts
                .get(&r.host_id)
                .cloned()
                .unwrap_or_else(|| format!("(Host {}, missing)", r.host_id)),
            id: r.id,
            service_id: r.service_id,
            host_id: r.host_id,
            name: r.name,
            ip: r.ip,
        })
        .collect())
}

/// An Instance Name with surrounding whitespace removed, if it isn't empty.
fn valid_instance_name(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("name", "An Instance Name can't be empty."));
    }
    Ok(name)
}

/// An IP with surrounding whitespace removed, if it is an IPv4 or IPv6
/// address.
fn valid_ip(ip: &str) -> Result<&str, Error> {
    let ip = ip.trim();
    if ip.parse::<IpAddr>().is_err() {
        return Err(invalid(
            "ip",
            format!("{ip:?} is not an IP address: give an IPv4 or IPv6 address."),
        ));
    }
    Ok(ip)
}

/// Fails if another Service Instance (other than `except`) of this Service
/// on this Host already has this Instance Name, blaming the input `field`.
async fn unused_identity(
    db: &mut impl toasty::Executor,
    field: &'static str,
    service_id: i64,
    host_id: i64,
    name: &str,
    except: Option<i64>,
) -> Result<(), Error> {
    let taken = models::ServiceInstance::filter(
        models::ServiceInstance::fields()
            .service_id()
            .eq(service_id)
            .and(models::ServiceInstance::fields().host_id().eq(host_id))
            .and(models::ServiceInstance::fields().name().eq(name)),
    )
    .exec(&mut *db)
    .await?
    .iter()
    .any(|other| Some(other.id) != except);
    if taken {
        // Names for the message; the Service or Host may be gone (cli/si).
        let service = models::Service::filter_by_id(service_id)
            .first()
            .exec(&mut *db)
            .await?
            .map_or(format!("Service {service_id}"), |s| s.name);
        let host = models::Host::filter_by_id(host_id)
            .first()
            .exec(&mut *db)
            .await?
            .map_or(format!("Host {host_id}"), |h| h.name);
        return Err(invalid(
            field,
            format!("{host} already has a Service Instance of {service} named {name}."),
        ));
    }
    Ok(())
}
