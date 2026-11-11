Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns
Issue-Key: apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns|_http#33|Timeout|with urllib.request.urlopen(req, timeout=timeout) as resp: # noqa: S310 - fixed https
Issue-Commit: 5776b1d0bb94f52c23a7c9dceb3e487542215018

# apigateway__cloudwatch__lambda: a REST API stage's 5xx alarm never leaves OK, so the alarm's SNS -> SQS notification is never delivered (the check spins to its 600s budget)

> _Note: this issue was identified against the code base at commit `5776b1d0bb94f52c23a7c9dceb3e487542215018`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: an alarm over a deployed REST API stage's AWS/ApiGateway 5XXError series never transitions OK -> ALARM, so the alarm action (SNS publish) is never executed and nothing reaches the subscribed SQS queue. The check therefore never reaches a terminal assertion and is cut off by the harness after 600.0s.

AWS: a deployed REST API stage publishes the AWS/ApiGateway 5XXError series under the dimensions {ApiName, Stage} (and per-method series when detailed metrics are enabled). An alarm created with PutMetricAlarm over that series with Namespace=AWS/ApiGateway, MetricName=5XXError, Statistic=Sum, Period=60, EvaluationPeriods=1, ComparisonOperator=GreaterThanOrEqualToThreshold and a Threshold below the number of 500s served settles at StateValue 'OK' under TreatMissingData=notBreaching, and once the stage serves 500s it crosses into 'ALARM' within the evaluation period. Smithy (cloudwatch): StateValue is an enum of exactly OK | ALARM | INSUFFICIENT_DATA, DescribeAlarms returns MetricAlarms[].StateValue, and DescribeAlarmHistory returns the StateUpdate items whose HistoryData records oldState/newState with a 'Threshold Crossed ...' stateReason. On the transition the configured AlarmActions are executed, so an SNS topic named in AlarmActions publishes one message whose Message body is the alarm JSON (NewStateValue 'ALARM', OldStateValue 'OK', Trigger.MetricName '5XXError', Trigger.Namespace 'AWS/ApiGateway', Trigger.Dimensions the alarm's dimensions) and an SQS queue subscribed to that topic receives it. The transition itself and the action execution are recorded behavior only - the model describes the shapes, not the evaluator.

Emulator: the check first proves the SNS -> SQS wiring with a canary publish, then drives bursts of integration 500s through GET /boom on the deployed stage while polling DescribeAlarms. The alarm never reports StateValue 'ALARM', so the burst loop never exits and the run ends with 'Error: Check timed out after 600.0s' with no further diagnosis than the expired budget. No OK -> ALARM StateUpdate appears in DescribeAlarmHistory and no notification is delivered to the queue. Hypothesis: the stage never publishes the 5XXError datapoints into the CloudWatch metric store, so the evaluator has nothing to breach on - this project's suite reports the same silence for every other AWS/ApiGateway series under this heading.

Why: a client that pages on API 5xx rate is never paged. The whole alarm -> SNS -> SQS chain is inert for API Gateway metrics even though every component answers its own control-plane calls, so any monitoring or auto-remediation built on stage metrics silently never fires, and a caller that waits for the transition hangs rather than being refused.

Fix: the API Gateway data plane (services/service-com-amazonaws-apigateway-backplanecontrolservice) must emit the deployed stage's 5XXError/Count datapoints under {ApiName, Stage} into the CloudWatch metric store, and the CloudWatch alarm evaluator (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) must then move the alarm's StateValue, record the StateUpdate in DescribeAlarmHistory with a 'Threshold Crossed' stateReason, and execute the configured AlarmActions.

Not covered by an existing open issue as the suite reports it: the open issue 20261110T162302Z-apigateway-cloudwatch-lambda-a-rest-api-5xx-alarm-never-transitions-ok.md describes this same cause and names this same check in its Issue-Trace-Ids, but the replay report did not mark this failure OWNED by it (its Issue-Key pins a run/frame signature that no longer joins), so this run re-files the finding with the current failing check id; a fixer should close both together. The open issues 20261109T152222Z-apigateway-cloudwatch-lambda-updatestage-refuses-a-real-method.md (carries the other failures of this heading, not this key) and 20261109T152222Z-apigateway-cloudwatch-aws-apigateway-count-metrics-are-never.md (filed under a different service heading) do not carry this key either.

Traces: apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns.

## Evidence
Expected: after a burst of requests the Lambda integration answers 500 (body 'LAMBDA-500'), DescribeAlarms reports MetricAlarms[0].StateValue == 'ALARM' for the 5XXError {ApiName, Stage} alarm; DescribeAlarmHistory contains a StateUpdate with oldState 'OK', newState 'ALARM' and a stateReason beginning 'Threshold Crossed'; the SQS queue subscribed to the alarm's SNS topic receives one message whose Message decodes to an alarm body with NewStateValue 'ALARM', OldStateValue 'OK', Trigger.MetricName '5XXError', Trigger.Namespace 'AWS/ApiGateway' and Trigger.Dimensions [{ApiName: <api name>}, {Stage: 'live'}] (the names/ids are taken from the request, not fixed text). The paired control API that only ever answers 200 must stay 'OK' and deliver nothing.

Actual: the canary SNS -> SQS publish arrives, both alarms reach 'OK', GET /boom returns the integration's 500 as expected, but DescribeAlarms never reports 'ALARM'; the burst loop never exits and the suite reports 'Error: Check timed out after 600.0s'. No OK -> ALARM StateUpdate and no SQS notification were ever observed.

## Steps to Reproduce
1. Create a python3.12 Lambda function whose handler returns {"statusCode": 500} for a path ending in /boom and {"statusCode": 200} otherwise.
2. Create a REGIONAL REST API with GET /boom -> AWS_PROXY to that function and GET /ready -> MOCK 200, add the lambda:InvokeFunction permission for apigateway.amazonaws.com, and CreateDeployment to stage 'live'.
3. Create an SQS queue and an SNS topic, set the queue policy to allow sns.amazonaws.com:SendMessage from that topic, and Subscribe the queue to the topic; verify with a canary Publish that the message reaches the queue.
4. PutMetricAlarm with Namespace=AWS/ApiGateway, MetricName=5XXError, Dimensions=[{Name:ApiName,Value:<api name>},{Name:Stage,Value:live}], Statistic=Sum, Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator=GreaterThanOrEqualToThreshold, TreatMissingData=notBreaching, AlarmActions=[<topic ARN>]. Wait for DescribeAlarms to report StateValue 'OK'.
5. Issue repeated GET <stage url>/boom requests (each answered 500) and poll DescribeAlarms for StateValue 'ALARM' -> it stays 'OK' indefinitely; DescribeAlarmHistory shows no OK -> ALARM StateUpdate and ReceiveMessage on the queue returns no alarm notification.
