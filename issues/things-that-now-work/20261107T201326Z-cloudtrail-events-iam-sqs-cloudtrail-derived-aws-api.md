Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__events__iam__sqs.xsvc-iam-1-iam-role-events-route-by-event-name-and-only-in-us-east-1
Issue-Key: cloudtrail__events__iam__sqs.xsvc-iam-1-iam-role-events-route-by-event-name-and-only-in-us-east-1|_collect#49|Timeout|return
Issue-Commit: 47509ee38907103620e4b84df19d39bf76901d44

# cloudtrail__events__iam__sqs: CloudTrail-derived "AWS API Call via CloudTrail" events are never published to the event bus, so no EventBridge rule on aws.iam or aws.sqs ever fires (check hangs until the harness ceiling)

> _Note: this issue was identified against the code base at commit `47509ee38907103620e4b84df19d39bf76901d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
One root cause.

Cause 1 of 1: a logging multi-region trail with IncludeGlobalServiceEvents=true and a WriteOnly management-event selector produces no CloudTrail-derived events on the default EventBridge bus, so rules whose pattern matches detail-type "AWS API Call via CloudTrail" never deliver anything to their SQS targets. Failing check: cloudtrail__events__iam__sqs.xsvc-iam-1-iam-role-events-route-by-event-name-and-only-in-us-east-1.

AWS: recorded/documented behavior only — neither the EventBridge nor the CloudTrail Smithy model describes event generation. Smithy (eventbridge) states only the matching surface used here: PutRule.EventPattern (@length 0..4096, error InvalidEventPatternException) and PutTargets.Targets (@length 1..100), both of which the emulator accepts. In AWS, once a trail is logging write management events, every recorded management call is also published to the default bus in the account/region as an event with source "aws.<service>" (e.g. aws.iam, aws.sqs), detail-type "AWS API Call via CloudTrail", envelope `account` = the caller's account, envelope `region` and detail.awsRegion = the region the event belongs to, and a `detail` that is the CloudTrail record verbatim (eventSource, eventName, readOnly=false for a write, requestParameters, responseElements). IAM is a global service, so its records are published in the partition leader region us-east-1 regardless of the region the call was signed in, while a regional call such as sqs:TagQueue is published only in its own region. A rule whose pattern pins detail.eventName to one value matches only that event name, so CreateRole reaches the CreateRole rule and DeleteRole reaches the DeleteRole rule, on the same queue.

Emulator: nothing is ever delivered. The check creates the S3 bucket and its CloudTrail bucket policy, creates the multi-region trail, sets a WriteOnly/IncludeManagementEvents selector, starts logging (GetTrailStatus reports IsLogging=true and the selector round-trips, so those assertions pass), creates the two us-east-1 rules on an SQS target, and a comparison-region rule, then does iam:CreateRole and sqs:TagQueue and long-polls both queues. Nothing arrives: the delivery loop never satisfies its break condition and the check is cut by the harness ceiling — "Error: Check timed out after 180.0s" — before it can even reach its own DATAPLANE_UNIMPLEMENTED assertion (its cold budget is 1000 s). The queues stay empty for both the global aws.iam event and the regional aws.sqs event, so neither the positive legs nor the region-scoping negatives can be evaluated.

Why: a client that wires audit-driven automation — an EventBridge rule on "AWS API Call via CloudTrail" fanning IAM role creation/deletion or SQS tagging to a queue, Lambda or Step Functions — sees its rule silently never fire against the emulator. Nothing is refused and nothing reports a failure: PutRule, PutTargets, CreateTrail and StartLogging all succeed and GetTrailStatus claims IsLogging=true, so the integration looks healthy while every event is lost. Region-scoping logic (global IAM events in us-east-1 only, regional events in their own region) and eventName-level rule selectivity are likewise untestable, and any test that waits for such a delivery hangs until its own ceiling instead of failing fast.

Fix: the CloudTrail recording path must publish each recorded management event to the EventBridge default bus of the account/region (us-east-1 for global services such as iam.amazonaws.com), building the envelope (source, detail-type, account, region, time) and carrying the audit record as `detail`; EventBridge's rule-matching/dispatch path must then evaluate the stored patterns against those events and deliver to SQS targets. This is the same class of defect as open issue 20261025T115007Z-events-lambda-rds-sqs-an-eventbridge-rule-on.md (an aws.rds CloudTrail event never fires a rule) and is downstream of the recording gaps in 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md and 20261030T182917Z-cloudtrail-lookupevents-never-serves-the-iam-amazonaws-com.md; neither of those open issues carries this check's replay key, and this check is reported as not owned.

Traces: cloudtrail__events__iam__sqs.xsvc-iam-1-iam-role-events-route-by-event-name-and-only-in-us-east-1.

## Evidence
Expected (AWS): after StartLogging on a multi-region trail with IncludeGlobalServiceEvents=true and EventSelectors=[{ReadWriteType: WriteOnly, IncludeManagementEvents: true}]:
  - the us-east-1 rule with pattern {"source":["aws.iam"],"detail-type":["AWS API Call via CloudTrail"],"detail":{"eventSource":["iam.amazonaws.com"],"eventName":["CreateRole"]}} delivers one message to its SQS target whose envelope has source="aws.iam", detail-type="AWS API Call via CloudTrail", account=<caller account>, region="us-east-1", and detail with eventSource="iam.amazonaws.com", eventName="CreateRole", readOnly=false, requestParameters.roleName=<the role created> and responseElements carrying the RoleId CreateRole returned;
  - the DeleteRole-pinned rule delivers the DeleteRole event and the CreateRole-pinned rule does not;
  - the comparison-region (non-us-east-1) rule delivers the sqs:TagQueue event with envelope region = that region and delivers no aws.iam event for this run's roles.

Actual (emulator): zero messages on either queue. ReceiveMessage long-polls return empty for the whole run, so the delivery loop never breaks and the harness kills the check: "Error: Check timed out after 180.0s" — before the check's own cold-delivery budget (1000 s) elapses and before its DATAPLANE_UNIMPLEMENTED assertion can report counts. The preceding control-plane calls all succeeded: CreateTrail returned a TrailARN, PutEventSelectors/GetEventSelectors round-tripped [("WriteOnly", True)], GetTrailStatus returned IsLogging=true, PutRule/PutTargets and the SQS queue policy were accepted.

(Names, role ids, queue URLs, markers and timings are per-run values and are not part of the requirement; the requirement is that the events are published and matched at all, with the source/detail-type/region/detail fields above.)

## Steps to Reproduce
1. In us-east-1: create an S3 bucket and put the standard CloudTrail bucket policy on it (s3:GetBucketAcl on the bucket, s3:PutObject on AWSLogs/<account>/* with s3:x-amz-acl=bucket-owner-full-control for cloudtrail.amazonaws.com).
2. cloudtrail:CreateTrail with IsMultiRegionTrail=true, IncludeGlobalServiceEvents=true; cloudtrail:PutEventSelectors with [{ReadWriteType: "WriteOnly", IncludeManagementEvents: true}]; cloudtrail:StartLogging. Confirm GetTrailStatus reports IsLogging=true.
3. sqs:CreateQueue in us-east-1; events:PutRule with pattern {"source":["aws.iam"],"detail-type":["AWS API Call via CloudTrail"],"detail":{"eventSource":["iam.amazonaws.com"],"eventName":["CreateRole"]}} and a second rule identical but eventName ["DeleteRole"]; events:PutTargets pointing both at the queue with InputTransformer template {"rule": "<aws.events.rule-name>", "event": <aws.events.event.json>}; allow the rule ARNs in the queue policy.
4. sqs:CreateQueue in another region (e.g. us-west-2) with a rule matching sources aws.iam and aws.sqs and eventNames CreateRole/DeleteRole/TagQueue.
5. iam:CreateRole (any trust policy) and sqs:TagQueue on the comparison-region queue.
6. Long-poll both queues. Observed: both stay empty indefinitely — no CreateRole event in us-east-1, no TagQueue event in the comparison region — and the check is terminated by the 180 s harness ceiling. Expected: the CreateRole event on the CreateRole rule only, the DeleteRole event (after iam:DeleteRole) on the DeleteRole rule only, the TagQueue event in its own region, and no aws.iam event outside us-east-1.
