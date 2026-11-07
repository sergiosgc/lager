# Inheritance resolved in Rust, ranked by Ansible's Group depth

ADR 0001 and ADR 0003 expected Effective Variables to come from recursive SQL queries. Lager instead loads the Groups, the memberships it needs and the relevant Variable values with ordinary queries, and walks the hierarchy in Rust. The data can already contain Parent Group cycles written by `cli/si` (#2), and an unguarded recursive query over a cycle never ends; a walk in Rust tracks what it has visited and stops. Inventories are small enough that loading the Groups up front costs nothing noticeable.

Groups are ranked the way Ansible ranks them, not by distance from the Host as #11 first described: the deeper a Group sits below the top of the hierarchy, the higher it ranks, so a child Group always wins over its Parent Group (#4, story 46); at the same depth the highest `ansible_group_priority` wins, read as Ansible's `int()` reads it; then the last by name. Ranking by distance from the Host could let a Parent Group win over its own child when a Host is directly in both, and disagreed with what Ansible actually applies.

## Consequences

- Recursive Inheritance queries do not go through raw SQL, despite ADR 0001's consequence line.
- Host and Group Effective Variables share the Group ranking above. A Service inherits from its Parent Services, nearest first, through the same cycle-tolerant walk up the hierarchy; there is no priority among Services.
