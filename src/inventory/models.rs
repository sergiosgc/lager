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
