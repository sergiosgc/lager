# Variable values through table-driven SQL

Lager uses Toasty for data access (ADR 0001), with one exception: the four Variable value tables (`host_variable`, `host_group_variable`, `service_variable`, `service_instance_variable`). They have the same shape (`variable`, an entity column, `value`, with the primary key on the first two), and every rule about them — validating, reading, upserting, deleting — is the same for all four. Mapping each to its own Toasty model would mean four copies of each operation, or a trait implemented four times over generated query types.

Instead, each kind of entity names its value table and entity column, and the reads, the upsert (`INSERT … ON CONFLICT (variable, <entity>) DO UPDATE`) and the deletes are plain SQL built from those names. The names are compile-time constants, never input, and the values are bound parameters.

## Consequences

- Raw SQL is used for Variable values, alongside Toasty for every other table.
- Adding a kind of entity with Variables means one more entry naming its table and column, not another model and set of queries.
- `models::HostVariable` stays for the Server Index lookup across all Hosts, which reads `host_variable` by Variable rather than by entity.
