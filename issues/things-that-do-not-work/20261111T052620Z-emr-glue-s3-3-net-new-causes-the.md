Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cli-glue-crawler-missing-role-permission-error-negative emr__glue__s3.glue-crawler-missing-s3-list-permission-rejected emr__glue__s3.cfn-deploy-glue-crawler-invalid-s3-path-negative emr__glue__s3.eventbridge-glue-crawler-completion-triggers-downstream-glue-job-positive
Issue-Key: emr__glue__s3.cli-glue-crawler-missing-role-permission-error-negative|run#17|AssertionError|assert glue.get_tables(DatabaseName=db_name)["TableList"] == [], (
Issue-Key: emr__glue__s3.glue-crawler-missing-s3-list-permission-rejected|run#18|AssertionError|assert glue.get_tables(DatabaseName=db_name)["TableList"] == []
Issue-Key: emr__glue__s3.cfn-deploy-glue-crawler-invalid-s3-path-negative|run#23|AssertionError|assert invalid_deploy["status"] in {"ROLLBACK_COMPLETE", "CREATE_FAILED"}, invalid_deploy
Issue-Key: emr__glue__s3.eventbridge-glue-crawler-completion-triggers-downstream-glue-job-positive|_poll_until#32|Timeout|time.sleep(interval_s)
Issue-Commit: 41e8aa3f6ef6702105d7e1e39f8f06aa9334e793

# emr__glue__s3: 3 net-new causes — the Glue crawler ignores its role's S3 permissions, CreateCrawler accepts an S3 target in a nonexistent bucket (no 404 S3-target validation, so CloudFormation reports CREATE_COMPLETE), and crawler completion drives no EventBridge rule

> _Note: this issue was identified against the code base at commit `41e8aa3f6ef6702105d7e1e39f8f06aa9334e793`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCrawler

## Description
Three net-new root causes (Cause 1 of 3 .. Cause 3 of 3) reached through the emr__glue__s3 behavioral checks. Each cause has its own failing check ids; all three must be closed before this issue is submitted.

Cause 1 of 3 — a crawler whose role cannot list/read the target bucket still catalogues a table. Failing checks: emr__glue__s3.cli-glue-crawler-missing-role-permission-error-negative, emr__glue__s3.glue-crawler-missing-s3-list-permission-rejected.
AWS: recorded behavior only — the Glue Smithy model does not express authorization outcomes. A crawler whose role is trusted by glue.amazonaws.com and carries only arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole (which grants S3 only on aws-glue-* prefixed buckets) returns to State=READY with LastCrawl.Status SUCCEEDED or FAILED and publishes NO table: GetTables returns an empty TableList, and GetTable for the would-be table answers EntityNotFoundException with the message 'Entity Not Found' (fixed text, no variable parts). The sibling crawler whose role does grant s3:ListBucket and s3:GetObject on the same bucket publishes exactly one table — the graded boundary is catalog state, not the crawl status string.
Emulator: both crawls publish a table; GetTables for the unauthorized crawler's database is non-empty, so the crawl read objects its role may not read.
Why: the IAM/S3 boundary around a privileged catalog reader does not hold — data an unauthorized role may not read becomes catalog metadata (and from there is queryable), so least-privilege validation passes here and fails on AWS.
Fix: the crawler execution path in the Glue crate must evaluate the crawl role's effective S3 permissions on each S3 target before listing/reading and leave the catalog untouched for a target it cannot read.
Traces: the two checks above; services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs and the crawler worker/state in that crate.

Cause 2 of 3 — CreateCrawler accepts an S3 target path inside a bucket that does not exist, so a CloudFormation stack that AWS rolls back reaches CREATE_COMPLETE. Failing check: emr__glue__s3.cfn-deploy-glue-crawler-invalid-s3-path-negative.
AWS: Smithy: CreateCrawler declares InvalidInputException (client error, HTTP 400) among AlreadyExistsException / OperationTimeoutException / ResourceNumberLimitExceededException. Beyond the model, recorded behavior: AWS validates each S3 target at CreateCrawler time and refuses a path whose bucket does not exist, with a 400 InvalidInputException whose message names the S3 target and the underlying 404 from S3 (the check matches on the failure reason containing both 'S3 target' and '404'; the fixed text is that S3-target-validation wording and the variable parts are the offending s3:// path and the S3 status code). An AWS::Glue::Crawler resource over such a path therefore fails to create and the stack ends ROLLBACK_COMPLETE/CREATE_FAILED with that reason on the crawler resource, and the AWS::Glue::Database the stack would have owned is rolled back too — GetDatabase then answers EntityNotFoundException with the message 'Database <name> not found.' ('Database ' + the requested name + ' not found.').
Emulator: CreateCrawler accepts the target with no validation, so the stack settles CREATE_COMPLETE with an empty failure list: {'status': 'CREATE_COMPLETE', 'failures': []}.
Why: a typo'd or not-yet-created bucket in a crawler target is reported as a healthy deployment; the client only discovers it much later (or never), and templates written to be refused at deploy time are instead accepted, so rollback-on-bad-input behavior cannot be exercised.
Fix: CreateCrawler's input validation in the Glue crate — resolve every S3Targets[].Path against S3 and refuse a nonexistent bucket with InvalidInputException naming the S3 target and the 404, which is what lets CloudFormation mark the crawler resource CREATE_FAILED and roll the stack back.
Traces: the check above; services/service-com-amazonaws-glue-awsglue/src/ops/create_crawler.rs.

Cause 3 of 3 — a crawl that finishes Succeeded drives no EventBridge rule, so a downstream Lambda never starts the next Glue job. Failing check: emr__glue__s3.eventbridge-glue-crawler-completion-triggers-downstream-glue-job-positive.
AWS: recorded behavior only — on the crawler's RUNNING → READY transition Glue publishes one envelope on the default event bus with source 'aws.glue' (fixed text), detail-type 'Glue Crawler State Change' (fixed text) and detail carrying crawlerName (from the request) and state 'Succeeded'; a rule matching source/detail-type/detail.state invokes its Lambda target, which starts the downstream job, observed reaching SUCCEEDED.
Emulator: the check never completes — the replay reports 'Check timed out after 60.0s' and nothing downstream is observed. Hypothesis: the same missing crawler state-change emission that the athena__glue__s3 crawler-event check observes directly (there the archived envelope never appears within 240s) leaves the rule unmatched, so the Lambda is never invoked.
Why: crawler-completion chaining — the standard 'crawl then transform' pattern — silently never fires, with every component reporting healthy and no refusal anywhere.
Fix: the crawler state machine's event emission (publish the aws.glue / 'Glue Crawler State Change' envelope at crawl completion) and the EventBridge delivery path for that envelope.
Related but NOT covering this check: open issue 20261103T101621Z-athena-glue-s3-glue-publishes-no-glue-crawler-state-change-eventbridge.md is grounded in the athena__glue__s3 crawler-event check only, and the replay did not attribute this check to it.
Traces: the check above.

## Evidence
Cause 1 — expected (AWS): glue.get_tables(DatabaseName=<db>)['TableList'] == [] after the unauthorized crawl, with glue.get_table answering EntityNotFoundException / 'Entity Not Found'. Actual (emulator):
  [mismatch] emr__glue__s3.cli-glue-crawler-missing-role-permission-error-negative
  AssertionError: no table should be cataloged when the crawler role lacks S3 object-read access
  [mismatch] emr__glue__s3.glue-crawler-missing-s3-list-permission-rejected
  AssertionError (assert glue.get_tables(DatabaseName=db_name)['TableList'] == [])

Cause 2 — expected: the stack with Targets={'S3Targets':[{'Path':'s3://<never-created-bucket>/data/'}]} ends ROLLBACK_COMPLETE or CREATE_FAILED, with a crawler-resource failure reason naming the S3 target and a 404, and GetDatabase for the stack's database answering EntityNotFoundException 'Database <name> not found.'. Actual:
  [mismatch] emr__glue__s3.cfn-deploy-glue-crawler-invalid-s3-path-negative
  AssertionError: {'stack_name': 'sim2real-xsvc-stk-...', 'status': 'CREATE_COMPLETE', 'failures': []}

Cause 3 — expected: the 'Glue Crawler State Change' / state=Succeeded event invokes the rule's Lambda target, which starts the downstream Glue job, observed SUCCEEDED. Actual:
  [mismatch] emr__glue__s3.eventbridge-glue-crawler-completion-triggers-downstream-glue-job-positive
  Error: Check timed out after 60.0s  (nothing downstream observed)

## Steps to Reproduce
Cause 1: create a bucket with data/sample.csv; create a role trusted by glue.amazonaws.com with only arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole; create a database and a crawler over s3://<bucket>/data/ with that role; start it and poll get_crawler until READY; call get_tables — observe a table instead of [].
Cause 2: glue.create_crawler (or deploy an AWS::Glue::Database + AWS::Glue::Crawler stack) with Targets={'S3Targets':[{'Path':'s3://<a-bucket-that-was-never-created>/data/'}]} and a role trusted by glue.amazonaws.com — observe the call succeeding and the stack reaching CREATE_COMPLETE instead of a 400 InvalidInputException naming the S3 target's 404 and a stack rollback.
Cause 3: create a crawler with read access over a populated prefix; events.put_rule on {'source':['aws.glue'],'detail-type':['Glue Crawler State Change'],'detail':{'state':['Succeeded']}} with a Lambda target that calls glue.start_job_run; glue.start_crawler; poll get_job_runs for the downstream job — observe no run is ever started.
