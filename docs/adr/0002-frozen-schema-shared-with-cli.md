# Frozen schema, shared with `cli/si`, foreign keys off

Lager edits an existing SQLite inventory database whose schema is frozen. The same file is read by `cli/si`, which remains the dynamic inventory script Ansible calls for `--list`/`--host`; Lager only writes to it. Lager never creates or migrates tables, and maps its models onto the existing table and column names. Setting up the database is out of Lager's scope: if the file is missing, or `db_version` doesn't hold the expected version (currently `1`), Lager refuses to start.

The schema has two foreign keys that point at tables that don't exist (`host_group.parent → host_groups`, `host_group_variable."group" → "group"`). Since the schema can't change, Lager opens the database with foreign-key enforcement off and keeps referential integrity itself: deletes cascade (after the user confirms) inside one transaction, and child Groups and child Services have their parent cleared instead of being deleted.
