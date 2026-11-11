Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__cognito-idp.sign-in-attempts-and-successes-count-per-app-client
Issue-Key: cloudwatch__cognito-idp.sign-in-attempts-and-successes-count-per-app-client|run#20|AssertionError|assert units == {"None"}, (
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# cloudwatch__cognito-idp: AWS/Cognito SignInSuccesses datapoints are served with Unit "Count" instead of no unit ("None")

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the unit the emulator attaches to the AWS/Cognito sign-in metrics is wrong.
AWS: recorded behavior only — the metric's unit is not something the CloudWatch Smithy model constrains per metric; CloudWatch's StandardUnit enum simply includes both 'Count' and 'None'. Cognito publishes AWS/Cognito SignInSuccesses per (UserPool, UserPoolClient) with NO unit, so GetMetricStatistics/GetMetricData datapoints for it come back with Unit 'None' — the metric counts every sign-in request as a sample and only the successful ones in its Sum, but it is unit-less. (The Sum/SampleCount values the emulator reports are correct; only the unit diverges.)
Emulator: every SignInSuccesses datapoint carries Unit 'Count'.
Why: a caller that passes Unit='None' to GetMetricStatistics (the unit AWS stores, and the one the console/alarm definitions carry) gets zero datapoints back from the emulator, because a unit filter only matches datapoints stored with that unit. Dashboards and alarms copied from a real account therefore read empty, and code that asserts on or renders the datapoint's unit shows a unit AWS never reported.
Fix: the unit recorded when the cognito-idp sign-in metrics are emitted into the cloudwatch metric store (the AWS/Cognito metric-emission path), and consequently the Unit the cloudwatch GetMetricStatistics datapoint projection serves and filters on.
Traces: cloudwatch__cognito-idp.sign-in-attempts-and-successes-count-per-app-client.

## Evidence
AssertionError: "SignInSuccesses datapoints for UserPoolClient=74d1438396f94d01ab046417ff carry Unit(s) {'Count'}, expected exactly {'None'}". Expected: the set of Unit values across the window's datapoints is exactly {'None'}. Actual: {'Count'}. The preceding assertion on (Sum, SampleCount) == (4, 7) for 4 successful and 3 refused sign-ins passed, so the counting behaviour is right and the unit is the only divergence. (The client id in the message is a per-run generated value, not part of the requirement.)

## Steps to Reproduce
1) CreateUserPool and two app clients on it; AdminCreateUser + AdminSetUserPassword(Permanent=true) so the user is CONFIRMED.
2) Through one app client, make 3 InitiateAuth USER_PASSWORD_AUTH calls with a wrong password (each refused NotAuthorizedException) and 4 with the correct password.
3) GetMetricStatistics for Namespace=AWS/Cognito, MetricName=SignInSuccesses, Dimensions UserPool=<poolId> and UserPoolClient=<clientId>, Statistics Sum and SampleCount over the last 10 minutes.
4) Inspect each returned datapoint's Unit: it is 'Count'; AWS reports 'None'. Equivalently, re-issue the same call with Unit='None' and the emulator returns no datapoints.
