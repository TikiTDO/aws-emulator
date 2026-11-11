Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3.glue-crawler-role-denied-s3-read-catalogues-no-table glue__s3.glue-crawler-combine-compatible-schemas-yields-one-table-not-two
Issue-Key: glue__s3.glue-crawler-role-denied-s3-read-catalogues-no-table|run#26|AssertionError|assert tables_after_denied == [], (
Issue-Key: glue__s3.glue-crawler-combine-compatible-schemas-yields-one-table-not-two|run#27|AssertionError|assert len(grouped_tables) < len(default_tables), (
Issue-Commit: 41e8aa3f6ef6702105d7e1e39f8f06aa9334e793

# glue__s3: 2 net-new causes — a crawl under a role denied s3:GetObject still catalogues a table, and the crawler always publishes one table per include path, ignoring schema-similarity splitting and the CombineCompatibleSchemas grouping policy

> _Note: this issue was identified against the code base at commit `41e8aa3f6ef6702105d7e1e39f8f06aa9334e793`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCrawler

## Description
Two net-new root causes (Cause 1 of 2, Cause 2 of 2) reached through the glue__s3 behavioral checks. Each cause has its own failing check id; both must be closed before this issue is submitted.

Cause 1 of 2 — a crawler whose role is denied s3:GetObject on the target prefix still publishes a table. Failing check: glue__s3.glue-crawler-role-denied-s3-read-catalogues-no-table.
AWS: recorded behavior only — Glue's model expresses no authorization outcome. A crawler whose role is trusted by glue.amazonaws.com but has no read grant on the target bucket returns to State=READY with LastCrawl.Status SUCCEEDED and LastCrawl.ErrorMessage absent (AWS does not report an error for an unreadable target) yet catalogues NOTHING: GetTables for the database returns no table with the crawler's TablePrefix. An identical crawler whose role can read the same prefix infers the full schema and publishes its table.
Emulator: the denied crawl catalogues a table — the replay reports 'the failed crawl still catalogued [\'denied_rows\']; the database must be untouched'.
Why: the crawler is a privileged reader; relying on an S3 read denial to keep a prefix out of the catalog works on AWS and leaks here, so data an unauthorized role must not see becomes catalog metadata and is queryable downstream.
Fix: the crawler execution path in the Glue crate must evaluate the crawl role's effective S3 permissions per target before listing/reading and leave the catalog untouched for a target it cannot read.
Traces: the check above; services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs and the crawler worker/state in that crate.

Cause 2 of 2 — the crawler publishes exactly one table named after the include-path leaf folder regardless of the child prefixes under it, so neither the default table-grouping behavior nor Configuration.Grouping.TableGroupingPolicy has any effect. Failing check: glue__s3.glue-crawler-combine-compatible-schemas-yields-one-table-not-two.
AWS: Smithy: CreateCrawler.Configuration is a CrawlerConfiguration (a JSON document, so the model does not constrain its content); recorded/documented behavior supplies the semantics. Over one include path s3://<bucket>/data/ with two child prefixes whose schemas are data-compatible but not schema-similar (part1's id is integral → BIGINT, part2's is decimal → DOUBLE, plus an added column), the DEFAULT policy splits them into two tables (one per child prefix), while Configuration {"Version":1.0,"Grouping":{"TableGroupingPolicy":"CombineCompatibleSchemas"}} folds them into ONE table with the widened schema. The requirement is the relation: the grouped crawler's table count must be strictly smaller than the default crawler's over the identical include path.
Emulator: both crawlers produce the same single table ['data'] — one table named after the include path's leaf folder. The Configuration is stored and ignored, and the default policy never splits per child prefix. The same single-table-per-include-path behavior is visible in glue__sqs.glue-crawler-sqs-event-queue-triggers-target-positive, where two child prefixes tablea/ and tableb/ yield only ['data'].
Why: table layout is the crawler's whole product. A client whose catalog, Athena queries and ETL scripts are written against per-prefix tables (or against a deliberately combined table) gets one differently named table instead, so every downstream name and schema is wrong, and the grouping policy cannot be exercised at all.
Fix: the crawler's table-inference/grouping stage — derive candidate tables per child prefix of each include path, apply schema-similarity splitting by default, and honor Configuration.Grouping.TableGroupingPolicy=CombineCompatibleSchemas by folding compatible candidates into one table.
Traces: the check above (plus the corroborating glue__sqs check named in its own issue); the crawler worker/table-inference in services/service-com-amazonaws-glue-awsglue/src.

## Evidence
Cause 1 — expected (AWS): LastCrawl.Status 'SUCCEEDED' with no ErrorMessage AND no table catalogued under the crawler's TablePrefix. Actual (emulator):
  [mismatch] glue__s3.glue-crawler-role-denied-s3-read-catalogues-no-table
  AssertionError: the failed crawl still catalogued ['denied_rows']; the database must be untouched

Cause 2 — expected: over the same include path, the CombineCompatibleSchemas crawler's table count is strictly smaller than the default crawler's (AWS: 1 vs 2). Actual:
  [mismatch] glue__s3.glue-crawler-combine-compatible-schemas-yields-one-table-not-two
  AssertionError: CombineCompatibleSchemas produced 1 tables (['data']) and the default policy produced 1 (['data']) over the identical include path 's3://<bucket>/data/'. The grouping policy must fold compatible child folders, so the grouped count must be strictly smaller; equal counts mean the Configuration was stored and ignored

## Steps to Reproduce
Cause 1: put rows/people.csv in a bucket; create two Glue-trusted roles, one WITHOUT any read grant on the bucket and one with s3:GetObject/s3:ListBucket; create one database and two crawlers over s3://<bucket>/rows/ with TablePrefix 'denied_' and 'control_'; run the denied crawler to READY; glue.get_tables — observe a 'denied_*' table instead of none.
Cause 2: put data/part1/rows.csv (id integral) and data/part2/rows.csv (id decimal, extra column) in a bucket; create two databases and two otherwise identical crawlers over s3://<bucket>/data/, one with Configuration={"Version":1.0,"Grouping":{"TableGroupingPolicy":"CombineCompatibleSchemas"}} and one without; run both; compare glue.get_tables counts — observe one table named 'data' in each, instead of 2 (default) vs 1 (grouped).
