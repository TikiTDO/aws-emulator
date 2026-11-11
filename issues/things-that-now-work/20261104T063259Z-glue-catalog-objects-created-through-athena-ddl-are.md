Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-ddl-identifier-syntax-mistakes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-ddl-identifier-syntax-mistakes athena-cats-traj-output-location-mistakes athena-cats-traj-table-before-database-and-ddl-retries athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket athena-cats-traj-drop-database-cleanup
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: glue|GetDatabase|2xx->4xx|$.Database
Issue-Key: glue|GetDatabase|2xx->4xx|status
Issue-Key: glue|DeleteDatabase|2xx->4xx|status
Issue-Key: glue|GetTable|2xx->4xx|$.Table
Issue-Key: glue|GetTable|2xx->4xx|status
Issue-Key: glue|GetTables|2xx->4xx|$.TableList
Issue-Key: glue|GetTables|2xx->4xx|status
Issue-Key: glue|DeleteDatabase|4xx->2xx|status
Issue-Commit: fb394b2a45eb301a39bf46320599094d7846a70d

# Glue catalog objects created through Athena DDL are absent, and DeleteDatabase accepts an absent database (5 causes)

> _Note: this issue was identified against the code base at commit `fb394b2a45eb301a39bf46320599094d7846a70d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDatabase

## Description
The Glue Data Catalog and the Athena query engine do not share state in the emulator: a database or table that Athena's DDL created is not present in Glue, so every Glue read or delete of it is refused 400 where AWS answers 200. Separately, DeleteDatabase does not refuse an absent database. 5 causes (causes 1-4 share the shared-catalog root; cause 5 is independent).

Cause 1 of 5: GetDatabase cannot see a database created by an Athena CREATE DATABASE statement.
AWS: Athena DDL writes into the same Glue Data Catalog, so GetDatabase answers HTTP 200 with the Database object — Name, CatalogId, CreateTime, DatabaseId, and CreateTableDefaultPermissions carrying the IAM_ALLOWED_PRINCIPALS grant Glue seeds (CatalogId/CatalogIdentifier are the account id; the name comes from the DDL statement).
Emulator: HTTP 400 and no Database member — the database does not exist in Glue at all.
Why: the standard Athena-then-Glue workflow (create with DDL, inspect with the catalog API) is broken: a client cannot discover, describe or manage anything Athena created.
Fix: the glue crate's database state must be the same catalog the athena crate's DDL writes to (shared catalog store), and the GetDatabase handler must read it.
Traces: athena-cats-traj-ddl-identifier-syntax-mistakes#task#10 (also task#11, task#12), athena-cats-traj-output-location-mistakes#task#9.

Cause 2 of 5: DeleteDatabase cannot delete a database created by Athena DDL.
AWS: HTTP 200 (empty body) when the named database exists.
Emulator: HTTP 400 — the database is absent from Glue, so teardown fails.
Why: a client cannot clean up resources it created, and a teardown step that AWS completes fails instead.
Fix: as cause 1 — shared catalog state behind the glue crate's DeleteDatabase handler.
Traces: athena-cats-traj-ddl-identifier-syntax-mistakes#teardown#13 (also teardown#14), athena-cats-traj-output-location-mistakes#teardown#10, athena-cats-traj-table-before-database-and-ddl-retries#teardown#20, athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#teardown#14.

Cause 3 of 5: GetTable cannot see a table created by an Athena CREATE EXTERNAL TABLE statement.
AWS: HTTP 200 with the Table object the DDL produced — DatabaseName, Name, Owner 'hadoop', TableType 'EXTERNAL_TABLE', CreatedBy the caller's IAM ARN, Parameters (EXTERNAL=TRUE plus the statement's table properties), and a StorageDescriptor carrying the columns, the S3 Location from the statement, the Hive input/output formats and the SerDe parameters.
Emulator: HTTP 400 and no Table member.
Why: a client that uses Athena DDL to register a table and then reads its schema through Glue gets nothing.
Fix: as cause 1, behind the glue crate's GetTable handler.
Traces: athena-cats-traj-table-before-database-and-ddl-retries#task#19, athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#12.

Cause 4 of 5: GetTables refuses a listing for a database created by Athena DDL instead of returning an empty list.
AWS: HTTP 200 with TableList present and empty when the database exists but holds no tables.
Emulator: HTTP 400, TableList absent.
Why: a client cannot enumerate a database's tables, and cannot distinguish 'no tables' from 'no database'.
Fix: as cause 1, behind the glue crate's GetTables handler.
Traces: athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#9.

Cause 5 of 5: DeleteDatabase answers 200 for a database that does not exist.
AWS: a delete of an absent database is refused with HTTP 400 EntityNotFoundException (the same classification GetDatabase uses for an absent database, whose recorded message is 'Database <name> not found.').
Emulator: HTTP 200, as though the delete succeeded.
Why: a client cannot tell a real delete from a no-op, so cleanup verification and idempotency checks both pass against a catalog that never held the resource.
Fix: the glue crate's DeleteDatabase handler (existence check before the mutation).
Traces: athena-cats-traj-drop-database-cleanup#teardown#16.

Related open issues that do NOT cover these: 20261102T132146Z-glue-answers-200-for-absent-entities-and-an.md owns the GetDatabase 200-for-absent-entity divergence in this same fixture (replay reports it as owned) but not the DeleteDatabase delete path of cause 5; 20261102T132146Z-glue-pagination-emits-a-raw-name-nexttoken-ignores.md owns the GetTables pagination divergence, not the 400 refusal of cause 4; 20261101T211008Z-glue-gettable-reports-an-absent-database-with-the.md concerns GetTable's message text, not the missing catalog state of cause 3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.
Cause 1 — athena-cats-traj-ddl-identifier-syntax-mistakes#task#10, GetDatabase: expected status 200 with $.Database={'CatalogId': '<account>', 'CatalogIdentifier': '<account>', 'CreateTableDefaultPermissions': [{'__type': 'PrincipalPermissions', 'Permissions': ['ALL'], 'Principal': {'DataLakePrincipalIdentifier': 'IAM_ALLOWED_PRINCIPALS'}}], 'CreateTime': ..., 'DatabaseId': ..., 'Name': '<ddl database name>'}; actual 400 with $.Database missing (cluster of 4 steps).
Cause 2 — athena-cats-traj-ddl-identifier-syntax-mistakes#teardown#13, DeleteDatabase: expected 200; actual 400 (cluster of 5 steps).
Cause 3 — athena-cats-traj-table-before-database-and-ddl-retries#task#19, GetTable: expected 200 with the full $.Table (TableType EXTERNAL_TABLE, Owner 'hadoop', StorageDescriptor with the three DDL columns, S3 Location, TextInputFormat/HiveIgnoreKeyTextOutputFormat and the LazySimpleSerDe parameters); actual 400 with $.Table missing (2 steps).
Cause 4 — athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#9, GetTables: expected 200 with $.TableList=[]; actual 400 with $.TableList missing.
Cause 5 — athena-cats-traj-drop-database-cleanup#teardown#16, DeleteDatabase: expected status 400; actual 200.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect the listed fixture#step pairs, representative athena-cats-traj-ddl-identifier-syntax-mistakes#task#10.
4. Minimal manual repro: run Athena StartQueryExecution with 'CREATE DATABASE <db>' and then 'CREATE EXTERNAL TABLE <db>.items (...) LOCATION 's3://<bucket>/products'', wait for SUCCEEDED, then call Glue GetDatabase(<db>), GetTables(<db>), GetTable(<db>, items) and finally DeleteDatabase(<db>) — all must answer 200. Then call DeleteDatabase(<db>) again: it must answer 400 EntityNotFoundException, not 200.
