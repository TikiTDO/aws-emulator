Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.cli-glue-crawler-missing-s3-permission-rejected athena__glue__s3.glue-crawler-missing-s3-read-permission-negative athena__glue__s3.deploy-cfn-glue-crawler-missing-s3-bucket-policy-negative athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive
Issue-Key: athena__glue__s3.cli-glue-crawler-missing-s3-permission-rejected|run#15|AssertionError|assert tables == [], f"crawler created a table despite missing S3 permission: {tables}"
Issue-Key: athena__glue__s3.glue-crawler-missing-s3-read-permission-negative|run#22|AssertionError|assert bad_tables == [], (
Issue-Key: athena__glue__s3.deploy-cfn-glue-crawler-missing-s3-bucket-policy-negative|run#22|AssertionError|assert len(tables) == 1, tables
Issue-Key: athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl|run#20|AssertionError|assert csv_body.decode().splitlines() == ['"_col0"', f'"{len(rows)}"'], (
Issue-Key: athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive|_poll_until#31|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 41e8aa3f6ef6702105d7e1e39f8f06aa9334e793

# athena__glue__s3: 4 net-new causes — the Glue crawler ignores S3 authorization (role policy and bucket policy), Athena names an unaliased aggregate column after its SQL text instead of _col0, and no Glue Crawler State Change event is published

> _Note: this issue was identified against the code base at commit `41e8aa3f6ef6702105d7e1e39f8f06aa9334e793`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCrawler

## Description
Four net-new root causes (Cause 1 of 4 .. Cause 4 of 4), all reached through the athena__glue__s3 behavioral checks. Each cause has its own failing check ids; all four must be closed before this issue is submitted.

Cause 1 of 4 — a Glue crawler whose role cannot read the target bucket still catalogues a table (identity-policy leg). Failing checks: athena__glue__s3.cli-glue-crawler-missing-s3-permission-rejected, athena__glue__s3.glue-crawler-missing-s3-read-permission-negative.
AWS: recorded behavior only — the Glue Smithy model (CreateCrawler/StartCrawler/GetCrawler) models no authorization outcome, so the contract comes from the recording. A crawler whose role is trusted by glue.amazonaws.com but carries only arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole (which reaches only aws-glue-* prefixed buckets) runs and returns to State=READY; LastCrawl.Status is SUCCEEDED or FAILED, but the crawler reads nothing it is not allowed to read, so the target database is left with NO table: GetTables returns an empty TableList and GetTable for the would-be table answers EntityNotFoundException with the message 'Entity Not Found' (fixed text; no variable parts). An otherwise identical crawler whose role does grant s3:GetObject and s3:ListBucket on that bucket publishes exactly one table.
Emulator: the crawl under the unauthorized role publishes a full table anyway — GetTables returns one EXTERNAL_TABLE whose StorageDescriptor.Columns were inferred from the object contents and whose Parameters carry UPDATED_BY_CRAWLER/classification/objectCount/sizeKey, i.e. the crawler read the objects it must not have been able to read. The replay asserts 'crawler created a table despite missing S3 permission' / 'no table should be cataloged when the crawler role lacks S3 object-read access'.
Why: the crawler is a privileged reader. A client that relies on the IAM boundary to keep an unauthorized crawler out of a bucket gets the opposite result: data lands in the catalog (and from there is queryable through Athena) for a role that AWS would have denied, so least-privilege tests pass against the emulator and fail — or leak — against AWS.
Fix: the crawler execution path in the Glue crate — the crawl worker must evaluate the crawl role's effective S3 permissions on each S3 target before listing/reading, and publish no table (and mutate no catalog state) for a target it cannot read.
Traces: the two checks above; the crawler worker and the StartCrawler handler in services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs plus that crate's crawler state.

Cause 2 of 4 — the same blindness on the bucket-policy leg: a crawler whose role has S3 permissions but whose target bucket's policy denies that role still catalogues the table. Failing check: athena__glue__s3.deploy-cfn-glue-crawler-missing-s3-bucket-policy-negative.
AWS: recorded behavior only. In a single stack with two sibling crawlers over two prefixes of one bucket, the bucket policy admits only the 'good' role; the 'bad' crawler's prefix is unreadable to it. After both crawls, the database holds exactly ONE table — the one the authorized crawler published; the unauthorized crawler publishes nothing.
Emulator: the database holds TWO tables ('bad' and 'good'), both fully inferred ('bad' has Columns col0 bigint / col1 string, Location s3://<bucket>/bad/, Parameters UPDATED_BY_CRAWLER=<bad crawler name>), so the resource-policy denial is never consulted. The replay asserts len(tables) == 1 and reports both.
Why: resource-policy denials are the usual way a data owner fences a bucket off from a catalog pipeline it does not own; a client validating that fence sees it hold on AWS and leak here.
Fix: the same crawl-time authorization evaluation must also consult the target bucket's resource policy, not only the role's identity policy.
Traces: the check above.

Cause 3 of 4 — an Athena SELECT's result CSV names an unaliased expression column after its SQL text instead of _col0. Failing check: athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl.
AWS: recorded behavior only — the Athena Smithy model types ColumnInfo.Name as a string and fixes no value. Athena (Trino/Presto) names a result column that the query does not alias positionally: the first unaliased expression is '_col0', the second '_col1', and so on. The result object s3://<result-location>/<query-execution-id>.csv therefore holds the quoted header line "_col0" followed by one quoted value line per row; for SELECT count(*) over three rows that is the two lines "_col0" and "3". '_col' + ordinal is fixed text with the ordinal supplied by the column's position; the value line comes from the data.
Emulator: the CSV body is b'"count(*)"\n"3"\n' — the header is the literal SQL expression text 'count(*)' instead of '_col0'. The answer itself (3) is correct; only the column naming diverges.
Why: any consumer that reads Athena results by column name — a client calling GetQueryResults and keying on ResultSetMetadata, a downstream job parsing the CSV header, a BI tool mapping columns — binds to '_col0' on AWS and finds no such column here; worse, the emulator's name contains characters ('(', ')', '*') that AWS never puts in a result column name.
Fix: Athena's result-set naming in the query engine/serializer — assign positional '_colN' names to unaliased expression columns and use those names both in the CSV header it writes and in GetQueryResults' ResultSetMetadata.
Traces: the check above.

Cause 4 of 4 — a crawl that completes publishes no 'Glue Crawler State Change' event, so an EventBridge rule → Firehose → S3 archive pipeline receives nothing. Failing check: athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive.
AWS: recorded behavior only — Glue's model describes the control plane and no eventing. When a crawler returns to State=READY with LastCrawl.Status SUCCEEDED, Glue publishes one envelope on the account's default event bus whose 'source' is the fixed text 'aws.glue', whose 'detail-type' is the fixed text 'Glue Crawler State Change', and whose 'detail' carries crawlerName (the crawler name from the request) and state='Succeeded'. A rule matching that pattern delivers the raw envelope to its target, and a Firehose DirectPut target archives the JSON envelope as an object under its configured S3 prefix.
Emulator: the crawl now runs and publishes a table, but no envelope is ever delivered: the replay fails with 'archived Glue Crawler event object in S3 not met within 240s. Last result: None' — nothing is archived under the event prefix.
Why: crawler-completion fan-out (kick off ingestion, archive crawl outcomes, alarm on failures) silently never fires; the rule, target and stream all report healthy, so a client has no refusal and no event to debug.
Fix: the crawler state machine must emit the aws.glue / 'Glue Crawler State Change' envelope onto the default bus at the RUNNING → READY transition.
Related but NOT covering this check (the replay did not attribute it to either): open issue 20261103T101621Z-athena-glue-s3-glue-publishes-no-glue-crawler-state-change-eventbridge.md recorded this check when the crawler itself never reached READY, and open issue 20261030T063753Z-glue-startcrawler-refuses-every-crawl-501-notimplemented-so.md records a 501 refusal of StartCrawler; the crawler now runs and publishes tables, so what remains is only the absent event emission.
Traces: the check above.

## Evidence
Cause 1 — expected (AWS): after the unauthorized crawl, glue.get_tables(DatabaseName=<db>)['TableList'] == [] and glue.get_table for the would-be table answers EntityNotFoundException / 'Entity Not Found'. Actual (emulator):
  [mismatch] athena__glue__s3.cli-glue-crawler-missing-s3-permission-rejected
  AssertionError: crawler created a table despite missing S3 permission: [{'Name': 'crawler_data', ... 'Parameters': {'classification': 'csv', 'UPDATED_BY_CRAWLER': '<crawler>', 'objectCount': '1', ...}}]
  [mismatch] athena__glue__s3.glue-crawler-missing-s3-read-permission-negative
  AssertionError: crawler without S3 permission should not have published a table (status='SUCCEEDED'), got [{'Name': 'bad_data', ...}]

Cause 2 — expected: exactly one table (the authorized sibling's). Actual:
  [mismatch] athena__glue__s3.deploy-cfn-glue-crawler-missing-s3-bucket-policy-negative
  AssertionError: [{'Name': 'bad', ... 'UPDATED_BY_CRAWLER': '<bad crawler>'}, {'Name': 'good', ...}]  (assert len(tables) == 1)

Cause 3 — expected: the SELECT's result CSV body decodes to the lines ['"_col0"', '"3"']. Actual:
  [mismatch] athena__glue__s3.athena-writes-csv-and-metadata-for-a-select-and-txt-for-ddl
  AssertionError: the result CSV holds b'"count(*)"\n"3"\n'; the answer is 3

Cause 4 — expected: an object under the archive prefix whose JSON has source=='aws.glue', detail-type=='Glue Crawler State Change', detail.crawlerName==<crawler>, detail.state=='Succeeded'. Actual:
  [mismatch] athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive
  AssertionError: archived Glue Crawler event object in S3 not met within 240s. Last result: None

## Steps to Reproduce
Cause 1: create a bucket, put data/rows.csv; create a role trusted by glue.amazonaws.com with ONLY arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole attached; glue.create_database; glue.create_crawler with Targets={'S3Targets':[{'Path':'s3://<bucket>/data/'}]} and that role; glue.start_crawler; poll glue.get_crawler until State=='READY'; call glue.get_tables(DatabaseName=<db>) — observe a fully inferred table instead of [].
Cause 2: deploy one stack with a bucket, two crawler roles, a bucket policy that admits only the 'good' role, and two crawlers over prefixes good/ and bad/; run both; glue.get_tables — observe two tables instead of one.
Cause 3: create a bucket and a workgroup with ResultConfiguration pointing into it; CREATE EXTERNAL TABLE over a tab-delimited prefix; run 'SELECT count(*) FROM <db>.<table>'; s3.get_object on s3://<bucket>/athena-results/<query-execution-id>.csv — observe the header line "count(*)" instead of "_col0".
Cause 4: create a data bucket and an archive bucket; create a Glue role with read on the data bucket; create a database and a crawler over s3://<data>/data/; create a Firehose DirectPut stream writing to the archive bucket under glue-events/; events.put_rule with EventPattern {'source':['aws.glue'],'detail-type':['Glue Crawler State Change'],'detail':{'crawlerName':[<crawler>],'state':['Succeeded']}} and put_targets at the stream; glue.start_crawler; poll the archive prefix — observe it stays empty even after the crawl reports SUCCEEDED.
