Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__events__iam__sqs.xsvc-iam-1-iam-role-events-route-by-event-name-and-only-in-us-east-1
Issue-Key: cloudtrail__events__iam__sqs.xsvc-iam-1-iam-role-events-route-by-event-name-and-only-in-us-east-1|_collect#86|Timeout|msgs = sqs.receive_message(
Issue-Commit: a0165563333d7f6521b0bb32e0431915bfb4444e

# cloudtrail__events__iam__sqs: an EventBridge rule on an IAM CloudTrail event never delivers to its SQS target, so the leader-region routing check never completes

> _Note: this issue was identified against the code base at commit `a0165563333d7f6521b0bb32e0431915bfb4444e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1: a CloudTrail-sourced IAM management event is never published onto the EventBridge default bus, so a rule matching it never delivers and the check runs out of time instead of reaching any assertion.
AWS: IAM is a global service — its management events are recorded by CloudTrail, and delivered by EventBridge, in the partition leader region (us-east-1) only. A multi-region trail that is logging makes each accepted IAM write (e.g. CreateRole/TagRole/DeleteRole) available on the us-east-1 default event bus as an event with source 'aws.iam' and detail-type 'AWS API Call via CloudTrail', whose detail carries the CloudTrail record verbatim (including detail.eventName). A rule whose event pattern names those fields delivers the rendered InputTransformer body to its SQS target in that region, and no bus in any other region ever sees the same event. Recorded behavior only: this delivery path is not expressed in any Smithy model; the recording shows the first delivery arriving well inside the check's own cold budget.
Emulator: nothing is ever delivered. The check is cut off by the runner with 'Check timed out after 60.0s' — it blocks on the first wait for a delivered message and never returns a verdict, even though the emulator answers every control-plane call instantly (the check's own budgets — 1000 s cold, 240 s warm — exist for real AWS's trail warm-up, so against an emulator a live channel should deliver in seconds). Hypothesis: the CloudTrail-to-EventBridge bridge does not exist for iam.amazonaws.com events, so the rule has nothing to match; the adjacent open issues 20261112T213312Z-cloudtrail-cognito-idp-events-sqs-no-cloudtrail-sourced.md (no CloudTrail-sourced 'AWS API Call via CloudTrail' event ever reaches an EventBridge rule, for cognito-idp) and 20261030T182917Z-cloudtrail-lookupevents-never-serves-the-iam-amazonaws-com.md (LookupEvents serves no iam.amazonaws.com record in the leader region) describe the same shape for other services/surfaces but do not carry this check.
Why: a client that builds audit-driven automation — reacting to IAM writes by routing the CloudTrail event to a queue, and relying on that traffic existing only in the leader region — observes an empty queue forever and cannot distinguish 'no such event' from 'routing broken'. The region-scoping half of the contract (the same rule in another region must receive nothing) is also unobservable, so a client cannot validate its single-region deployment.
Fix: the CloudTrail-to-EventBridge publication path (management events recorded for global services in the partition leader region must be published onto that region's default bus as source 'aws.iam', detail-type 'AWS API Call via CloudTrail') and the event-pattern match/target delivery for that detail-type.
Traces: cloudtrail__events__iam__sqs.xsvc-iam-1-iam-role-events-route-by-event-name-and-only-in-us-east-1.

## Evidence
Expected: within the check's delivery budget, the SQS target of the us-east-1 rule receives one message per matching IAM CloudTrail event, bodies rendered by the InputTransformer template '{"rule": "<aws.events.rule-name>", "event": <aws.events.event.json>}', with detail-type 'AWS API Call via CloudTrail'; the comparison region's twin rule receives nothing. Actual: Error: Check timed out after 60.0s — no delivery, no assertion reached.

## Steps to Reproduce
In us-east-1: create an S3 bucket with the CloudTrail service policy, create and start a multi-region trail writing to it, create an SQS queue and an EventBridge rule whose pattern matches source 'aws.iam' / detail-type 'AWS API Call via CloudTrail' / detail.eventName of an IAM write, with the queue as target and the InputTransformer template above. Perform the IAM writes, then long-poll the queue for the delivered bodies; also poll a twin rule+queue in another region and expect nothing there.
