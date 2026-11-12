Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series|_ledger#23|Timeout|now = read()
Issue-Commit: 8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f

# cloudwatch__lambda__stepfunctions: AWS/States Lambda-task and Executions* metric series for optimized lambda:invoke vs bare-function-ARN Tasks are never published, so the metric ledger never settles and the check is killed at its budget

> _Note: this issue was identified against the code base at commit `8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the metric series that distinguish an optimized `arn:aws:states:::lambda:invoke` Task from a legacy bare-function-ARN Task are never readable from CloudWatch, so a consumer polling for them blocks forever instead of reading the attributed datapoints.

AWS: recorded behavior only for the publishing semantics — CloudWatch's Smithy model defines GetMetricStatistics (Namespace/MetricName/Dimensions/StartTime/EndTime/Period/Statistics -> Datapoints) but no publisher rules, and the Step Functions model states nothing about metric emission. The recording shows that for the SAME Lambda work driven two different ways, AWS attributes the metrics as follows, and the two attributions never cross: a Task whose Resource is the BARE function ARN (legacy) is accounted in namespace AWS/States under Dimensions exactly [{Name: LambdaFunctionArn, Value: <that function's ARN>}] as LambdaFunctionsScheduled / LambdaFunctionsStarted / LambdaFunctionsSucceeded, one count per execution of that Task, plus one LambdaFunctionRunTime sample per invocation whose Minimum is at least the function's own sleep duration; a Task whose Resource is `arn:aws:states:::lambda:invoke` (optimized integration) publishes NOTHING in the [LambdaFunctionArn] family for the invoked function — LambdaFunctionsScheduled/Started/Succeeded/Failed under that dimension stay absent (sum 0) — while the invocations themselves still appear in AWS/Lambda Invocations for that function name, and the execution outcomes appear in AWS/States ExecutionsSucceeded / ExecutionsFailed under Dimensions [{Name: StateMachineArn, Value: <state machine ARN>}] (succeeded for the permitted machine, failed for the machine whose role lacks the grant). The counts are a function of how many executions the client ran in the window; the ARNs, names and the sleep duration come from the request and from the created resources, not from one run. The recording also shows the denied optimized call ending the execution FAILED with error `Lambda.AWSLambdaException` and a cause that names the missing `lambda:InvokeFunction` permission.

Emulator: none of those series becomes readable. GetMetricStatistics keeps returning no datapoints for the polled AWS/States metrics (both the [LambdaFunctionArn] family and ExecutionsSucceeded/ExecutionsFailed under [StateMachineArn]) and for AWS/Lambda Invocations, so the check's ledger never reaches its expected reading and the run is killed: 'Error: Check timed out after 60.0s'. Because the poll never settles, no verdict on either dimension family, on the AWS/Lambda Invocations count, or on the denied execution's Lambda.AWSLambdaException error/cause can be obtained.

Why: every alarm, dashboard and autoscaling rule built on the AWS/States Lambda-task counters and on ExecutionsSucceeded/ExecutionsFailed sees nothing. A client cannot tell a workflow whose Lambda task failed from one that never dispatched a task, and it cannot rely on the attribution rule AWS guarantees — task counters keyed by LambdaFunctionArn only for direct-ARN Tasks, execution counters keyed by StateMachineArn — which is exactly what capacity and cost models are built on. Worse, a consumer that polls for a datapoint (rather than tolerating zero datapoints) blocks indefinitely.

Fix: the Step Functions execution path that dispatches Lambda Tasks must publish AWS/States LambdaFunctionsScheduled/Started/Succeeded/Failed and LambdaFunctionRunTime under the LambdaFunctionArn dimension for direct-ARN Tasks only, and ExecutionsSucceeded/ExecutionsFailed under the StateMachineArn dimension for every execution; the Lambda invoke path must publish AWS/Lambda Invocations; and the CloudWatch metric store must serve all of them to GetMetricStatistics. Related open issue 20261107T044335Z-cloudwatch-lambda-stepfunctions-aws-states-lambda-task-metric.md covers the same metric families for the sibling check but does not carry this check's replay key, so this failure is filed separately and should be closed together with it.

Traces: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series.

## Evidence
Failing check: cloudwatch__lambda__stepfunctions.obs-sfn-1-optimized-lambda-invoke-denied-and-ok-never-populate-function-arn-series [mismatch].

Expected (AWS): after 3 executions of a state machine whose Task is `arn:aws:states:::lambda:invoke` on function M, 2 executions of the same definition under a role without lambda:InvokeFunction, and 2 executions of a state machine whose Task Resource is the bare ARN of function L, GetMetricStatistics returns — AWS/States under Dimensions [{LambdaFunctionArn: L}]: LambdaFunctionsScheduled 2, LambdaFunctionsStarted 2, LambdaFunctionsSucceeded 2, LambdaFunctionRunTime SampleCount 2 with Minimum >= the function's sleep; AWS/States under Dimensions [{LambdaFunctionArn: M}]: Scheduled/Started/Succeeded/Failed all 0 (no datapoints); AWS/Lambda Invocations for M: 3; AWS/States ExecutionsSucceeded under [{StateMachineArn: optimized machine}]: 3; AWS/States ExecutionsFailed under [{StateMachineArn: denied machine}]: 2. The denied executions end FAILED with error 'Lambda.AWSLambdaException' and a cause naming 'lambda:InvokeFunction'.

Actual (emulator): 'Error: Check timed out after 60.0s' — the polled ledger never reaches that reading because the series are never published, so GetMetricStatistics returns empty Datapoints for every read and the poll loop runs to the harness budget. (The counts above are the shape of the requirement for the executions the client ran; they are not literal per-run values.)

## Steps to Reproduce
1. Create a Lambda execution role and two python3.12 functions M and L whose handler sleeps ~1.5s and returns {'echo': event['n']}; create an alias on L.
2. Create a states.amazonaws.com role GRANT with lambda:InvokeFunction on M, M:*, L, L:* and a second states role with no policy (DENY).
3. Create state machines: OPT = single Task Resource 'arn:aws:states:::lambda:invoke' with Parameters {FunctionName: <M ARN>, 'Payload.$': '$'} under GRANT; DENY = same definition under the bare role; LEG = single Task whose Resource is the bare L function ARN under GRANT.
4. StartExecution OPT three times (each must SUCCEED with output wrapping the handler return in Payload), DENY twice (each must FAIL with error 'Lambda.AWSLambdaException' and a cause naming 'lambda:InvokeFunction'), LEG twice (each must SUCCEED).
5. Poll GetMetricStatistics for AWS/States LambdaFunctionsScheduled/Started/Succeeded/Failed and LambdaFunctionRunTime with Dimensions=[{LambdaFunctionArn: L}] and with [{LambdaFunctionArn: M}], AWS/Lambda Invocations with [{FunctionName: M}], and AWS/States ExecutionsSucceeded/ExecutionsFailed with [{StateMachineArn: OPT}] / [{StateMachineArn: DENY}].
6. Against the emulator every read stays empty, so the ledger never settles and the check is killed at its budget.
