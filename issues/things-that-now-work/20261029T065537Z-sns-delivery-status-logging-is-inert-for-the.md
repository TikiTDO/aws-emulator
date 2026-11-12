Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.iam-role-trusts-sns-service-assumed
Issue-Key: events__lambda__sns.iam-role-trusts-sns-service-assumed|_poll_for_log_events#22|Timeout|time.sleep(2)
Issue-Commit: 6c829b3943454ada5df5dfe2630a7a4c6694ce1c

# SNS delivery-status logging is inert for the lambda protocol too: a topic with LambdaSuccessFeedbackRoleArn/SampleRate=100 writes no delivery-status entry into sns/<region>/<account>/<topic>

> _Note: this issue was identified against the code base at commit `6c829b3943454ada5df5dfe2630a7a4c6694ce1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One root cause, grounded in one failing behavioral check.

Cause 1 of 1: SNS never assumes a topic's delivery-status feedback role and never emits a delivery-status log entry, so a successful lambda-protocol delivery leaves the feedback log group empty forever.

AWS: Smithy (com.amazonaws.sns): SetTopicAttributes(TopicArn, AttributeName required; AttributeValue) -> no output, refusals AuthorizationError(403) / InvalidParameter(400) / NotFound(404) / InvalidSecurity(403); the delivery-status attributes LambdaSuccessFeedbackRoleArn, LambdaFailureFeedbackRoleArn and LambdaSuccessFeedbackSampleRate are set through it. Beyond the attribute round-trip the effect is an out-of-band log write, so: recorded behavior only. The recording shows that once a topic carries LambdaSuccessFeedbackRoleArn pointing at an IAM role whose trust policy allows sts:AssumeRole for sns.amazonaws.com (with logs:CreateLogGroup/CreateLogStream/PutLogEvents) and LambdaSuccessFeedbackSampleRate '100', SNS itself assumes that role and writes one delivery-status record per delivery into the CloudWatch Logs log group named sns/<region>/<account-id>/<topic-name>. The record names the delivery endpoint (here the subscribed Lambda function's ARN) and reports "status":"SUCCESS" for a delivery the endpoint accepted. That log write is the only observable proof the trust policy was exercised by the service rather than merely persisted. The log group NAME TEMPLATE and the presence of the endpoint plus the status field are the requirement; the timestamps, request ids, generated role/function/topic names and the sampled record count on any one run are not.

Emulator: SetTopicAttributes accepts all three feedback attributes, the Lambda subscription is created and a Publish is accepted, but no such log group, log stream or delivery-status record ever appears. The check polls CloudWatch Logs for it (its own budget is 180 s) and produces no verdict - it is killed by the harness: `Check timed out after 60.0s`. Nothing is refused; the feature is simply inert for the lambda protocol, exactly as it already is for sqs.

Why: a client that relies on SNS delivery-status logging for observability - confirming that a notification was delivered, auditing which role SNS assumed, alarming on failure feedback, or verifying that a service-principal trust policy is actually exercised - sees an empty log group forever, with no error to indicate the feature does nothing. It reads as "no deliveries happened", which is the opposite of the truth.

Fix: SNS's subscription delivery path - the delivery-status logging step must assume the topic's configured Lambda{Success,Failure}FeedbackRoleArn at the configured LambdaSuccessFeedbackSampleRate and emit the delivery-status record (naming the endpoint and the SUCCESS/FAILURE status) into the CloudWatch Logs group sns/<region>/<account-id>/<topic-name>, creating the group and stream as that role - rather than only storing the feedback attributes.

Related open issue that does not carry this replay key: 20261025T064350Z-lambda-sns-sqs-a-cfn-deployed-sns-deliverystatuslogging.md files the same inert delivery-status logging for an SQS-protocol subscription configured through CloudFormation, and carries only the check id lambda__sns__sqs.cfn-deploy-iam-role-trust-sns-positive. This check configures the feedback role directly through SetTopicAttributes for the LAMBDA protocol and is not covered by that issue's replay key, so it is filed here; a fixer should treat both as the same delivery-status path.

Traces: events__lambda__sns.iam-role-trusts-sns-service-assumed.

Service attribution: the check id carries the compound token 'events__lambda__sns' (its SERVICES are events, lambda, sns); the diverging operation is SNS's Publish/delivery-status path, so this is filed under service 'sns'.

## Evidence
Expected (AWS): create an IAM role trusting sns.amazonaws.com for sts:AssumeRole with logs:CreateLogGroup/CreateLogStream/PutLogEvents; create a topic; AddPermission on a Lambda function for sns.amazonaws.com with SourceArn=<topic>; Subscribe(Protocol='lambda', Endpoint=<function arn>); SetTopicAttributes LambdaSuccessFeedbackRoleArn=<role arn>, LambdaSuccessFeedbackSampleRate='100', LambdaFailureFeedbackRoleArn=<role arn>; Publish. Within the poll budget, CloudWatch Logs group sns/<region>/<account-id>/<topic-name> exists and contains a delivery-status record naming the function ARN with "status":"SUCCESS".

Actual (emulator): the attributes are accepted and the publish returns 200, but the log group sns/<region>/<account-id>/<topic-name> never comes into existence and no delivery-status record is ever written; the check produces no verdict and is killed at the harness cap.

[mismatch] events__lambda__sns.iam-role-trusts-sns-service-assumed
  Error: Check timed out after 60.0s

(The check's own wait for the feedback log is 180 s, longer than the 60 s harness cap, so the kill is the consequence of a log that never appears rather than a separate defect. The generated role, function and topic names are per-run values; the log-group name template and the endpoint/status fields are the requirement.)

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the built emulator (region us-west-2).
2. iam.create_role with a trust policy allowing sts:AssumeRole for Service 'sns.amazonaws.com', attaching logs:CreateLogGroup/CreateLogStream/PutLogEvents.
3. lambda.create_function for a trivial handler; sns.create_topic.
4. lambda.add_permission(StatementId='sns-invoke', Action='lambda:InvokeFunction', Principal='sns.amazonaws.com', SourceArn=<topic arn>); sns.subscribe(TopicArn=<topic arn>, Protocol='lambda', Endpoint=<function arn>).
5. sns.set_topic_attributes for LambdaSuccessFeedbackRoleArn=<role arn>, LambdaSuccessFeedbackSampleRate='100', LambdaFailureFeedbackRoleArn=<role arn> (all accepted).
6. sns.publish(TopicArn=<topic arn>, Message='trust-policy-probe').
7. Poll logs.describe_log_groups / get_log_events for log group sns/<region>/<account-id>/<topic-name>. AWS: a delivery-status record naming the function ARN with "status":"SUCCESS". Emulator: the log group never exists and no record is ever written.
8. Re-replay events__lambda__sns.iam-role-trusts-sns-service-assumed to confirm the fix.
