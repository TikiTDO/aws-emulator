Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss
Issue-Key: cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss|_collect#42|Timeout|msgs = sqs.receive_message(
Issue-Commit: 0814fbf24ca8581d1be982874f66970e2423c484

# cloudtrail__dynamodb__events__sqs: no CloudTrail-sourced "AWS API Call via CloudTrail" event ever reaches an EventBridge rule, so DynamoDB control-plane routing never reaches a verdict

> _Note: this issue was identified against the code base at commit `0814fbf24ca8581d1be982874f66970e2423c484`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
One net-new root cause: the CloudTrail -> EventBridge management-event fan-out produces nothing, so no rule on a CloudTrail-sourced event can ever match.

Cause 1 of 1: a logging trail with management-event selectors publishes no "AWS API Call via CloudTrail" event to the default event bus. Failing check: cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss.

AWS:      recorded behavior only for the wire shape (no Smithy model describes this fan-out; DynamoDB emits no native EventBridge events, so CloudTrail's management-event fan-out is the only path from a DynamoDB control-plane call to the default bus). Once a trail is logging with an EventSelector of {ReadWriteType: WriteOnly, IncludeManagementEvents: true} and GetTrailStatus reports IsLogging true, every recorded write management event is also published to the account's default EventBridge bus as an envelope with detail-type "AWS API Call via CloudTrail" and source "aws.<service>" of the calling service ("aws.sqs" for sqs:TagQueue, "aws.dynamodb" for dynamodb:CreateTable/DeleteTable), envelope account = the recipient account id and region = the call's region, and a detail carrying the CloudTrail record: eventSource "<service>.amazonaws.com", eventName the operation, awsRegion, recipientAccountId, readOnly false, eventCategory "Management" and requestParameters of the call (requestParameters.tableName for CreateTable/DeleteTable). Rules whose event pattern names source/detail-type/detail.eventSource/detail.eventName then deliver exactly the matching events to their targets, and nothing else.
          Reference: https://docs.aws.amazon.com/awscloudtrail/latest/userguide/cloudtrail-aws-service-specific-topics.html
Emulator: nothing is ever published. The check stands up the whole channel against the emulator successfully — S3 trail bucket with the cloudtrail.amazonaws.com GetBucketAcl/PutObject policy, CreateTrail, PutEventSelectors (WriteOnly + IncludeManagementEvents), StartLogging with GetTrailStatus reporting IsLogging true, and three EventBridge rules with SQS targets (control on aws.sqs/TagQueue, subject on aws.dynamodb/CreateTable, near-miss on aws.dynamodb/DeleteTable) — then issues the control write sqs:TagQueue and blocks in its first delivery poll on the control queue. No envelope arrives; the check is cut off by the harness at 60 s still in that loop, before any DynamoDB call is made. Its own budget for this cold hop is 1800 s, so the replay shows only "no delivery", not a measured latency.
Why:      a client that wires auditing or auto-remediation to CloudTrail management events (EventBridge rule on detail-type "AWS API Call via CloudTrail", target SQS/Lambda/Step Functions) receives nothing on the emulator, so the rule silently never fires and no downstream automation runs. Because the fan-out is absent entirely, the graded behavior behind it is unreachable too: the positive match on this run's tableName, the near-miss rule that must take DeleteTable only and never a CreateTable, and the envelope/detail fields above are all unverifiable.
Fix:      the CloudTrail logging pipeline's EventBridge publication step — where a recorded management event (one admitted by the trail's EventSelectors) should be handed to the default event bus — together with the envelope construction that sets source "aws.<service>", detail-type "AWS API Call via CloudTrail", account/region and the detail projection of the CloudTrail record (eventSource, eventName, awsRegion, recipientAccountId, readOnly, eventCategory, requestParameters). Note: the open issues 20261112T213312Z-cloudtrail-cognito-idp-events-sqs-no-cloudtrail-sourced.md and 20261112T222923Z-cloudtrail-events-iam-sqs-an-eventbridge-rule-on.md report the same missing hop under other service headings; neither carries this check's replay key, so it is filed here.
Traces:   cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss.

## Evidence
Check cloudtrail__dynamodb__events__sqs.xsvc-dynamodb-3-create-table-reaches-only-its-rule-not-the-delete-table-near-miss, verdict mismatch, "Check timed out after 60.0s".

Where it is blocked: the check's leg-1 control loop, which polls the control rule's SQS queue until a CloudTrail-sourced envelope naming its sqs:TagQueue marker arrives. Its in-check failure text for that loop names the expectation:
  "DATAPLANE_UNIMPLEMENTED: the CloudTrail->EventBridge channel never went live: none of <n> sqs:TagQueue control writes (markers ...) reached the control rule within 1800s; expected >=1, actual 0 of 0 messages seen. No dynamodb call was made."

Expected (real AWS): after StartLogging on a trail whose EventSelectors are {ReadWriteType: WriteOnly, IncludeManagementEvents: true}, the sqs:TagQueue write is published to the default bus as source "aws.sqs", detail-type "AWS API Call via CloudTrail"; the subsequent dynamodb:CreateTable is published as source "aws.dynamodb", detail-type "AWS API Call via CloudTrail", detail.eventSource "dynamodb.amazonaws.com", detail.eventName "CreateTable", detail.readOnly false, detail.eventCategory "Management", detail.requestParameters.tableName the created table, detail.recipientAccountId and envelope account the account id, envelope region and detail.awsRegion the call's region. The CreateTable-only rule receives the create and never a DeleteTable; the DeleteTable-only rule receives the delete and never a CreateTable.

Actual (emulator): zero messages on the control queue; the trail reports IsLogging true but publishes no event to the bus. Nothing downstream of the fan-out is exercised — the check never issues CreateTable.

Setup calls that all succeeded against the emulator first (so the failure is isolated to the fan-out): s3:CreateBucket + PutBucketPolicy + PutBucketTagging, cloudtrail:CreateTrail, PutEventSelectors, StartLogging, GetTrailStatus (IsLogging true), sqs:CreateQueue x3, events:PutRule + PutTargets x3, sqs:TagQueue.

## Steps to Reproduce
1. Create an S3 bucket with a policy allowing cloudtrail.amazonaws.com s3:GetBucketAcl on the bucket and s3:PutObject on AWSLogs/<account>/* with s3:x-amz-acl bucket-owner-full-control.
2. cloudtrail:CreateTrail on that bucket, then PutEventSelectors with EventSelectors=[{ReadWriteType: "WriteOnly", IncludeManagementEvents: true}], then StartLogging; confirm GetTrailStatus reports IsLogging true.
3. Create three SQS queues and three EventBridge rules on the default bus, each with its queue as target and a policy admitting events.amazonaws.com:
   - control: {"source":["aws.sqs"],"detail-type":["AWS API Call via CloudTrail"],"detail":{"eventSource":["sqs.amazonaws.com"],"eventName":["TagQueue"]}}
   - subject: same envelope with source aws.dynamodb, detail.eventSource dynamodb.amazonaws.com, detail.eventName ["CreateTable"]
   - near-miss: same but detail.eventName ["DeleteTable"]
4. Issue sqs:TagQueue on the control queue and long-poll the control queue.
   Expected: an "AWS API Call via CloudTrail" envelope with source "aws.sqs" arrives. Actual: nothing arrives; the poll never terminates (the harness cuts the check at 60 s).
5. (Unreachable today) dynamodb:CreateTable, wait ACTIVE, and poll the subject queue for source "aws.dynamodb" / detail.eventName "CreateTable" / detail.requestParameters.tableName == the table; then DeleteTable and poll the near-miss queue; assert neither sink ever holds the other operation.
