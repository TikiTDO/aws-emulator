Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.glue-crawler-sqs-event-queue-triggers-target-positive glue__sqs.glue-crawler-state-change-to-sqs-positive glue__sqs.cfn-deploy-glue-crawler-eventbridge-sqs-stack-positive glue__sqs.deploy-glue-crawler-sqs-dlq-configured-positive
Issue-Key: glue__sqs.glue-crawler-sqs-event-queue-triggers-target-positive|run#17|AssertionError|assert "tablea" in tables and "tableb" in tables, f"expected 2 tables, got {list(tables)}"
Issue-Key: glue__sqs.glue-crawler-state-change-to-sqs-positive|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: glue__sqs.cfn-deploy-glue-crawler-eventbridge-sqs-stack-positive|_wait_for_matching_message#15|Timeout|resp = sqs.receive_message(
Issue-Key: glue__sqs.deploy-glue-crawler-sqs-dlq-configured-positive|_poll_until#25|Timeout|last = probe()
Issue-Commit: 41e8aa3f6ef6702105d7e1e39f8f06aa9334e793

# glue__sqs: 3 net-new causes — an event-mode crawler publishes one table named after the include path instead of one per child prefix, no Glue Crawler State Change event reaches an EventBridge→SQS rule, and a crawler with a DlqEventQueueArn never drains its event queue

> _Note: this issue was identified against the code base at commit `41e8aa3f6ef6702105d7e1e39f8f06aa9334e793`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCrawler

## Description
Three net-new root causes (Cause 1 of 3 .. Cause 3 of 3) reached through the glue__sqs behavioral checks. Each cause has its own failing check ids; all three must be closed before this issue is submitted.

Cause 1 of 3 — the crawler publishes a single table named after the include path's leaf folder instead of one table per child prefix. Failing check: glue__sqs.glue-crawler-sqs-event-queue-triggers-target-positive.
AWS: recorded behavior only — Glue's model types CrawlerTargets/S3Target but not the table layout a crawl produces. The first run of an event-mode crawler (RecrawlBehavior=CRAWL_EVENT_MODE) is a full listing crawl; over include path s3://<bucket>/data/ with the child prefixes data/tablea/dt=.../ and data/tableb/dt=.../, whose schemas differ, it publishes one table per child prefix, named after that prefix — 'tablea' and 'tableb' (the names are the child folder names from the data layout, not from the request).
Emulator: the crawl publishes exactly one table, 'data' — the include path's leaf folder — so neither child prefix becomes a table.
Why: every downstream name is wrong: Athena queries, partition updates and the per-table event-mode behavior the check then exercises all address tables that do not exist, while a differently named table appears in their place.
Fix: the crawler's table-inference stage must derive candidate tables from the child prefixes of each include path (and name them after those prefixes), not emit one table per include path.
Traces: the check above; the crawler worker/table-inference in services/service-com-amazonaws-glue-awsglue/src.

Cause 2 of 3 — a crawl that finishes Succeeded publishes no 'Glue Crawler State Change' event, so an EventBridge rule targeting SQS receives nothing. Failing checks: glue__sqs.glue-crawler-state-change-to-sqs-positive, glue__sqs.cfn-deploy-glue-crawler-eventbridge-sqs-stack-positive.
AWS: recorded behavior only — Glue models no eventing. At the crawler's RUNNING → READY transition Glue publishes one envelope on the account's default event bus whose 'source' is the fixed text 'aws.glue', whose 'detail-type' is the fixed text 'Glue Crawler State Change', and whose 'detail' carries crawlerName (the crawler Name from the request) and state='Succeeded'; a rule matching that pattern, with an SQS target whose queue policy allows events.amazonaws.com, delivers the raw envelope as a queue message. This holds both for a hand-wired rule and for the same wiring deployed as a CloudFormation stack.
Emulator: neither check ever observes a message — both end 'Check timed out after 60.0s' while draining the queue for an envelope with detail.crawlerName == the crawler. The same absence is observed directly in athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive, where the archived envelope never appears within 240 s.
Why: crawler-completion notification and fan-out silently never fire; the rule, the target and the queue all report healthy, so a client gets no event and no refusal to debug.
Fix: the crawler state machine must emit the aws.glue / 'Glue Crawler State Change' envelope onto the default bus at crawl completion, and that envelope must traverse the EventBridge rule → SQS target path.
Related but NOT covering these checks: open issue 20261103T101621Z-athena-glue-s3-glue-publishes-no-glue-crawler-state-change-eventbridge.md is grounded only in the athena__glue__s3 crawler-event check, and the replay did not attribute these two to it.
Traces: the two checks above.

Cause 3 of 3 — a crawler configured with a DlqEventQueueArn never drains the unparseable message off its event queue. Failing check: glue__sqs.deploy-glue-crawler-sqs-dlq-configured-positive.
AWS: recorded behavior only. A CloudFormation-deployed crawler whose S3 target carries an EventQueueArn and a DlqEventQueueArn consumes the messages on its event queue during a crawl: a message it cannot parse as an S3 event is deleted on the first receive and discarded — it is NOT routed to the DLQ and does not fail the crawl — so after the crawl the event queue is empty, the DLQ is empty, and the message's receive count never grows.
Emulator: the check never completes — the replay reports 'Check timed out after 60.0s', with no observation of the queue being drained. Hypothesis: the crawl never consumes its configured event queue at all, so the message stays on the queue and the check waits forever; this is consistent with Cause 2's absent crawl-driven messaging.
Why: a client that wires a crawler event queue plus DLQ to control incremental crawls sees messages accumulate on the queue (and a DLQ that never fills), so the crawl never advances and the DLQ-vs-discard contract cannot be relied on.
Fix: the crawler's event-mode queue consumer — receive from the configured EventQueueArn during a crawl, delete a message it cannot parse on first receive, and leave the DlqEventQueueArn queue untouched.
Traces: the check above.

## Evidence
Cause 1 — expected (AWS): after the first (full listing) crawl, glue.get_tables(DatabaseName=<db>) contains 'tablea' and 'tableb'. Actual (emulator):
  [mismatch] glue__sqs.glue-crawler-sqs-event-queue-triggers-target-positive
  AssertionError: expected 2 tables, got ['data']

Cause 2 — expected: an SQS message whose JSON body has source=='aws.glue', detail-type=='Glue Crawler State Change', detail.crawlerName==<crawler> and detail.state=='Succeeded'. Actual:
  [mismatch] glue__sqs.glue-crawler-state-change-to-sqs-positive
  Error: Check timed out after 60.0s
  [mismatch] glue__sqs.cfn-deploy-glue-crawler-eventbridge-sqs-stack-positive
  Error: Check timed out after 60.0s
(No envelope is delivered; the corroborating athena__glue__s3 crawler-event check reports 'archived Glue Crawler event object in S3 not met within 240s. Last result: None'.)

Cause 3 — expected: the unparseable event-queue message is deleted on first receive, the event queue and the DLQ both end empty, and the crawl is unaffected. Actual:
  [mismatch] glue__sqs.deploy-glue-crawler-sqs-dlq-configured-positive
  Error: Check timed out after 60.0s  (no draining observed)

## Steps to Reproduce
Cause 1: put data/tablea/dt=2024-01-01/data.csv and data/tableb/dt=2024-01-01/data.csv (different schemas) in a bucket; create an SQS queue; create a crawler with Targets={'S3Targets':[{'Path':'s3://<bucket>/data/','EventQueueArn':<queue arn>}]}, RecrawlPolicy={'RecrawlBehavior':'CRAWL_EVENT_MODE'}; start it and wait for READY/SUCCEEDED; glue.get_tables — observe ['data'] instead of 'tablea' and 'tableb'.
Cause 2: create a crawler over a readable prefix; events.put_rule with EventPattern {'source':['aws.glue'],'detail-type':['Glue Crawler State Change'],'detail':{'crawlerName':[<crawler>],'state':['Succeeded']}}; create an SQS queue, allow events.amazonaws.com in its policy, put_targets at the queue ARN; glue.start_crawler; receive_message in a loop — observe no envelope ever arrives even after the crawl reports SUCCEEDED. Repeat with the same wiring deployed as a CloudFormation stack.
Cause 3: deploy a stack with a bucket, an event queue, a DLQ, a Glue role and a crawler whose S3 target carries EventQueueArn and DlqEventQueueArn; send a message that is not a valid S3 event onto the event queue; put an object under data/dt=.../ and start the crawler; poll both queues — observe the message is never consumed/deleted.
