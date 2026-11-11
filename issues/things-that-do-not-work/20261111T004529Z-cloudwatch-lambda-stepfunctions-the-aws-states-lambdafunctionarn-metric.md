Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series
Issue-Key: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series|_poll#22|Timeout|raise AssertionError(f"timed out after {_METRIC_BUDGET_S}s waiting for {what}; last {last!r}")
Issue-Commit: f28dfafa5fc9e548e0b0df8415a880c6df425385

# cloudwatch__lambda__stepfunctions: the AWS/States LambdaFunctionArn metric family never settles — the Lambda-task metric check exhausts its 600 s budget

> _Note: this issue was identified against the code base at commit `f28dfafa5fc9e548e0b0df8415a880c6df425385`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the per-Lambda-task AWS/States counters (and/or the AWS/Lambda counters they are read alongside) never reach the values two executions produce, so the check is cut off at its budget.
AWS: recorded behavior only for the emission timing; the series themselves are AWS's documented Step Functions execution metrics. A Task whose Resource is a function ARN is counted by AWS/States under exactly the dimension [LambdaFunctionArn]: after one succeeding and one failing execution the sums are LambdaFunctionsScheduled 2, LambdaFunctionsStarted 2, LambdaFunctionsSucceeded 1, LambdaFunctionsFailed 1, and LambdaFunctionRunTime carries 2 samples; the execution counters ExecutionsSucceeded 1 / ExecutionsFailed 1 appear under [StateMachineArn]; AWS/Lambda Errors is 1 and Invocations >= 2 under [FunctionName]. No family crosses dimensions (LambdaFunctions* must not appear under [StateMachineArn], ExecutionsSucceeded must not appear under [LambdaFunctionArn]). These are published within the recorded window, which is why the check polls with a 600 s budget.
Emulator: the check is killed by its budget — `Check timed out after 600.0s`. Its last action is a poll of GetMetricStatistics over those seven series until all expected sums are reached; both executions reached their asserted terminal states before the poll, so the un-settled quantity is the metric data itself: at least one of those series never reaches its value within 600 s. Hypothesis: the AWS/States [LambdaFunctionArn] family is not published at all (the emulator publishes execution-level counters only), which would make the barrier unreachable rather than slow.
Why: a client cannot see per-task Lambda integration health in CloudWatch — no dashboard, alarm or capacity decision based on LambdaFunctionsFailed/LambdaFunctionRunTime can ever fire, and a polling client hangs instead of getting an answer.
Fix: the Step Functions task-execution path's CloudWatch metric emission (the AWS/States publisher: LambdaFunctions{Scheduled,Started,Succeeded,Failed} and LambdaFunctionRunTime dimensioned by [LambdaFunctionArn]), and the CloudWatch metric store they land in. Several sibling metrics-states checks in the cloudwatch__stepfunctions service are owned by 20261109T183935Z-cloudwatch-stepfunctions-5-net-new-causes-aws-states.md; that issue does not carry this check's key, so this one is filed on its own.
Traces: cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series.

## Evidence
Check cloudwatch__lambda__stepfunctions.metrics-states-stream-lambda-task-metrics-attribute-to-the-function-arn-series, verdict mismatch:

Error: Check timed out after 600.0s

Expected (after one SUCCEEDED and one FAILED execution of a Task whose Resource is the function ARN): AWS/States sums under [{'Name':'LambdaFunctionArn','Value':<fn arn>}] of LambdaFunctionsScheduled=2, LambdaFunctionsStarted=2, LambdaFunctionsSucceeded=1, LambdaFunctionsFailed=1; AWS/States under [{'Name':'StateMachineArn','Value':<sm arn>}] ExecutionsSucceeded=1, ExecutionsFailed=1; AWS/Lambda Errors=1 and Invocations>=2 under [{'Name':'FunctionName','Value':<fn name>}]; LambdaFunctionRunTime SampleCount 2 under [LambdaFunctionArn].
Actual: the polling barrier never settles and the check is cut off at its 600 s metric budget (_METRIC_BUDGET_S = 600.0), i.e. at least one of those series never reaches its expected sum.

## Steps to Reproduce
1. Create a Lambda function whose handler returns {'echo': n} and raises ValueError for mode='boom'.
2. Create a state machine with a single Task whose Resource is the raw function ARN, under a role granting lambda:InvokeFunction.
3. Start one execution with {'mode':'ok'} (SUCCEEDED) and one with {'mode':'boom'} (FAILED, error ValueError).
4. Poll GetMetricStatistics for AWS/States LambdaFunctionsScheduled / LambdaFunctionsStarted / LambdaFunctionsSucceeded / LambdaFunctionsFailed with Dimensions=[{'Name':'LambdaFunctionArn','Value':<fn arn>}], Period=60, Statistics=['Sum'], plus LambdaFunctionRunTime SampleCount: the sums 2/2/1/1 and 2 samples are never reached and the poll runs to its budget.
