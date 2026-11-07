//! The Server Index: a Host's sequential number, in its `server_index`
//! Variable.

use super::*;

impl Inventory {
    /// The Server Index a new Host gets by default: one more than the
    /// highest `server_index` among existing Hosts, counting only positive
    /// JSON integers, or `1` when there is none. Gaps are never refilled.
    pub async fn next_server_index(&self) -> Result<u64, Error> {
        next_server_index(&mut self.db.clone()).await
    }

    /// Things about a Host the operator should notice, such as another Host
    /// sharing its Server Index.
    pub async fn host_warnings(&self, host_id: i64) -> Result<Vec<String>, Error> {
        host_warnings(&mut self.db.clone(), host_id).await
    }
}

/// The `server_index` value of every existing Host, as stored. Values left
/// behind by deleted Hosts are not included.
pub(super) async fn server_indexes(
    db: &mut impl toasty::Executor,
) -> Result<Vec<(Host, String)>, Error> {
    let Some(variable) = variable_by_name(&mut *db, SERVER_INDEX).await? else {
        return Ok(Vec::new());
    };
    let hosts: std::collections::HashMap<i64, Host> = models::Host::all()
        .exec(&mut *db)
        .await?
        .into_iter()
        .map(|h| (h.id, Host::from(h)))
        .collect();
    let values =
        models::HostVariable::filter(models::HostVariable::fields().variable_id().eq(variable.id))
            .exec(&mut *db)
            .await?;
    Ok(values
        .into_iter()
        .filter_map(|v| hosts.get(&v.host_id).map(|h| (h.clone(), v.value)))
        .collect())
}

pub(super) async fn next_server_index(db: &mut impl toasty::Executor) -> Result<u64, Error> {
    let highest = server_indexes(db)
        .await?
        .iter()
        .filter_map(|(_, value)| parse_json(value)?.as_u64())
        .filter(|&n| n > 0)
        .max();
    // Saturating: an absurdly large stored value must not block new Hosts.
    Ok(highest.map_or(1, |n| n.saturating_add(1)))
}

pub(super) async fn host_warnings(
    db: &mut impl toasty::Executor,
    host_id: i64,
) -> Result<Vec<String>, Error> {
    let indexes = server_indexes(db).await?;
    let Some((_, own)) = indexes.iter().find(|(h, _)| h.id == host_id) else {
        return Ok(Vec::new());
    };
    let Some(own_json) = parse_json(own) else {
        return Ok(Vec::new());
    };
    let mut others: Vec<&str> = indexes
        .iter()
        .filter(|(h, value)| h.id != host_id && parse_json(value).as_ref() == Some(&own_json))
        .map(|(h, _)| h.name.as_str())
        .collect();
    if others.is_empty() {
        return Ok(Vec::new());
    }
    others.sort();
    Ok(vec![format!(
        "Server Index {} is also used by {}.",
        own.trim(),
        others.join(", ")
    )])
}
