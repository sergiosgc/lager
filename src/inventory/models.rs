//! Toasty models mapped onto the frozen schema (ADR 0002, ADR 0003), using
//! the real table and column names. Nothing here creates or alters tables.

#[derive(Debug, toasty::Model)]
#[table = "host"]
pub struct Host {
    #[key]
    #[auto(increment)]
    pub id: i64,

    pub name: String,
}

/// A Variable name, shared by the four value tables.
#[derive(Debug, toasty::Model)]
#[table = "variable"]
pub struct Variable {
    #[key]
    #[auto(increment)]
    pub id: i64,

    pub name: String,
}

/// A Host's Own Variable: the value of one Variable on one Host.
#[derive(Debug, toasty::Model)]
#[table = "host_variable"]
#[key(variable_id, host_id)]
pub struct HostVariable {
    #[column("variable")]
    pub variable_id: i64,

    #[column("host")]
    pub host_id: i64,

    pub value: String,
}

/// A Group. `parent` is its Parent Group; the schema's foreign key on it
/// points at a table that doesn't exist (ADR 0003).
#[derive(Debug, toasty::Model)]
#[table = "host_group"]
pub struct Group {
    #[key]
    #[auto(increment)]
    pub id: i64,

    #[column("parent")]
    pub parent_id: Option<i64>,

    pub name: String,
}

/// A Host's direct membership of a Group.
#[derive(Debug, toasty::Model)]
#[table = "host_group_membership"]
#[key(host_id, group_id)]
pub struct Membership {
    #[column("host")]
    pub host_id: i64,

    #[column("group")]
    pub group_id: i64,
}
