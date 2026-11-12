Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-enabled-then-disabled-by-stack-update cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-missing-invoke-permission-rejected
Issue-Key: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-enabled-then-disabled-by-stack-update|run#39|AssertionError|assert described["RotationRules"] == expected_rules, (
Issue-Key: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-missing-invoke-permission-rejected|run#11|AssertionError|assert got == _REJECTED, (
Issue-Commit: a0165563333d7f6521b0bb32e0431915bfb4444e

# cloudformation__iam__lambda__secretsmanager: 2 net-new causes — DescribeSecret omits the AutomaticallyAfterDays derived from a rate() rotation schedule, and the RotationSchedule CREATE_FAILED reason drops the service/status/request-id trailer

> _Note: this issue was identified against the code base at commit `a0165563333d7f6521b0bb32e0431915bfb4444e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
Two distinct net-new root causes in the stack-driven Secrets Manager rotation surface. Both checks now get PAST the two causes the open issue 20261109T163411Z-cloudformation-iam-lambda-secretsmanager-2-net-new-causes.md describes (RotationEnabled is now true after the RotationSchedule is created, and the stack with no invoke permission now rolls back); that issue does not carry these two checks, and the failures below are the next divergence each check reaches.

Cause 1 of 2: DescribeSecret echoes only the rotation schedule's ScheduleExpression and never derives AutomaticallyAfterDays from it.
AWS: Smithy: secretsmanager#RotationRulesType.AutomaticallyAfterDays (long, @range min 1 max 1000) is documented "In DescribeSecret and ListSecrets, this value is calculated from the rotation schedule after every successful rotation", and ScheduleExpression accepts a day-granularity rate() expression. So after a schedule is attached with RotationRules.ScheduleExpression = rate(<N> days), DescribeSecret serves RotationRules = {"ScheduleExpression": "rate(<N> days)", "AutomaticallyAfterDays": <N>} — the day count in the rate() expression is the derived value; the expression string itself is whatever the request set.
Emulator: DescribeSecret serves RotationRules containing only the echoed ScheduleExpression; AutomaticallyAfterDays is absent (recorded run: got {'ScheduleExpression': 'rate(10 days)'} where AWS serves {'ScheduleExpression': 'rate(10 days)', 'AutomaticallyAfterDays': 10}).
Why: a client or compliance tool that reads RotationRules.AutomaticallyAfterDays to prove a secret rotates at most every N days sees no value at all and reports the secret as having no rotation interval, even though rotation is configured. Code that normalises both forms of schedule through AutomaticallyAfterDays silently loses the interval.
Fix: the rotation-schedule state and DescribeSecret's (and ListSecrets') RotationRules serializer — derive AutomaticallyAfterDays from a day-granularity rate()/cron() schedule when the schedule is stored or served, instead of echoing only the expression.
Traces: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-enabled-then-disabled-by-stack-update.

Cause 2 of 2: the AWS::SecretsManager::RotationSchedule CREATE_FAILED reason carries the bare service sentence, without the SDK exception trailer AWS wraps into a resource-handler reason.
AWS: a registry resource handler reports an underlying service refusal as `Resource handler returned message: "<service message> (Service: <ServiceId>, Status Code: <http status>, Request ID: <request id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: <code>)`. For this refusal the fixed text is the Secrets Manager sentence `Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com.` followed by the fixed trailer words `(Service: SecretsManager, Status Code: 400, Request ID: ...) (SDK Attempt Count: ...)`; the variable parts are the request id and the attempt count of that call (and the RequestToken of the stack operation). HandlerErrorCode is AccessDenied. Recorded behavior only for the trailer's presence — the status 400 and code AccessDenied are taken from the recorded response.
Emulator: the DescribeStackEvents reason for the RotationSchedule resource reads `Resource handler returned message: "Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com." (RequestToken: <token>, HandlerErrorCode: AccessDenied)` — the service sentence is right and HandlerErrorCode is right, but the `(Service: SecretsManager, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)` trailer is missing entirely.
Why: deploy-time tooling (and a human reading a rollback) parses the handler reason to recover which service refused, with what HTTP status and under which request id, in order to correlate the failure with the service's own logs or a support case; with the trailer absent that correlation is impossible, and reason-equality assertions against AWS fail.
Fix: the failure-reason formatting of the AWS::SecretsManager::RotationSchedule provisioner (the CloudFormation resource-handler error wrapper): render the underlying service exception with its Service/Status Code/Request ID and SDK Attempt Count trailer before wrapping it in the handler reason.
Traces: cloudformation__iam__lambda__secretsmanager.deploy-secretsmanager-rotationschedule-missing-invoke-permission-rejected.

## Evidence
Cause 1 — Expected: DescribeSecret RotationRules {'ScheduleExpression': 'rate(10 days)', 'AutomaticallyAfterDays': 10}. Actual: AssertionError: expected RotationRules {'ScheduleExpression': 'rate(10 days)', 'AutomaticallyAfterDays': 10}, got {'ScheduleExpression': 'rate(10 days)'} (check file line 363; the positive control of the sibling check likewise expects RotationRules['AutomaticallyAfterDays'] == 30 for an AutomaticallyAfterDays-declared schedule).
Cause 2 — Expected handler reason/code (request id and attempt count masked by the check): ('Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com. (Service: SecretsManager, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'AccessDenied'). Actual: ('Secrets Manager cannot invoke the specified Lambda function. Ensure that the function policy grants access to the principal secretsmanager.amazonaws.com.', 'AccessDenied').

## Steps to Reproduce
Cause 1: CreateStack (Capabilities CAPABILITY_IAM) with an AWS::SecretsManager::Secret, a python3.12 rotation Lambda plus its AWS::Lambda::Permission for secretsmanager.amazonaws.com, and an AWS::SecretsManager::RotationSchedule whose RotationRules declare only ScheduleExpression 'rate(10 days)'. After CREATE_COMPLETE call DescribeSecret on the secret ARN and read RotationRules.
Cause 2: CreateStack with the same template but WITHOUT the AWS::Lambda::Permission; wait for ROLLBACK_COMPLETE, then read the single CREATE_FAILED DescribeStackEvents row for the RotationSchedule logical id and inspect the message inside 'Resource handler returned message: "..."'.
