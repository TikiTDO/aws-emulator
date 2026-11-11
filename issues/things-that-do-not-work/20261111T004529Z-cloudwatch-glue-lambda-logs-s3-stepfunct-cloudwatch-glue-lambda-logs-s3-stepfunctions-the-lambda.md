Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow-alarm-echoes-one-minute-period
Issue-Key: cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow-alarm-echoes-one-minute-period|verdict#19|AssertionError|raise AssertionError(self._summary(leg, evidence))
Issue-Commit: f28dfafa5fc9e548e0b0df8415a880c6df425385

# cloudwatch__glue__lambda__logs__s3__stepfunctions: the Lambda runtime's bundled AWS SDK resolves no 'glue' client, and the CDK-declared alarm is not echoed as declared (2 net-new causes)

> _Note: this issue was identified against the code base at commit `f28dfafa5fc9e548e0b0df8415a880c6df425385`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes in the CDK-bootstrapped Step Functions ETL workflow. The replay reports LEGS 4/12 PASS with bootstrap=PASS deploy=PASS assets=PASS wiring=FAIL logs=BLOCKED metrics=BLOCKED alarm=FAIL denial=BLOCKED drift=BLOCKED rebuild=BLOCKED teardown=BLOCKED toolkit=PASS, i.e. two legs FAILED on their own account (wiring, alarm) and the rest were blocked by wiring.

Cause 1 of 2: the Lambda execution environment's bundled AWS SDK knows only a handful of services, so a handler that builds a `glue` client cannot run.
AWS: recorded behavior only (no Smithy statement covers the guest runtime's SDK). AWS's managed Python Lambda runtime ships a boto3/botocore whose service data covers every AWS service, so `boto3.client("glue")` inside a function succeeds and the handler goes on to call Glue (here, to register/alter the curated partition). The recorded run shows the Step Functions execution driving that handler reaching SUCCEEDED and the Glue partition being curated.
Emulator: the Step Functions execution that drives the curating function ends FAILED and the function's error payload is `{"errorMessage":"Unknown service: 'glue'. Valid service names are: sqs, events, eventbridge, ..."}` — botocore's UnknownServiceError, raised in-guest because the runtime image carries service data for only a short whitelist of services. The fixed text of that message is botocore's own (`Unknown service: '<name>'. Valid service names are: <list>`); the variable parts are the requested service token and the list the emulator happens to bundle, so no literal from this run is a requirement — the requirement is that the client is resolvable at all.
Why: any workflow whose Lambda code calls a service outside that whitelist fails at import/client-construction time with an error AWS never produces. The failure is attributed to the user's handler (an unhandled exception payload), so a client debugging its own code is sent after a defect that does not exist, and every downstream observable of the workflow (logs, metric, denial, drift, rebuild, teardown legs here) becomes unobservable.
Fix: the Lambda service's execution-environment/runtime image construction — the guest runtime must expose a complete AWS SDK (full botocore service data), not a per-service whitelist. Related open issues name the same family for a different service token and a different missing client and so do not cover this one: 20261107T205257Z-events-lambda-sns-sqs-the-lambda-runtime-s.md and 20261106T133436Z-lambda-sns-sqs-in-function-aws-sdk-calls.md ('the guest SDK resolves no `sns` client').
Traces: cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow-alarm-echoes-one-minute-period (leg `wiring`).

Cause 2 of 2: the alarm the stack declares is not read back as declared.
AWS: PutMetricAlarm (https://docs.aws.amazon.com/AmazonCloudWatch/latest/APIReference/API_PutMetricAlarm.html) records exactly the alarm configuration it was given, and DescribeAlarms echoes it: for this stack's alarm on its metric-filter metric that is Namespace/MetricName as declared with an empty Dimensions list, Statistic=Sum, Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator=GreaterThanOrEqualToThreshold, TreatMissingData=missing and ActionsEnabled=true (the default, since the app sets no actions). Exactly one MetricAlarm is returned for the alarm name.
Emulator: the `alarm` leg FAILED. This leg is independent of the broken `wiring` leg (it has no `requires=`), it only calls DescribeAlarms on the deployed alarm name and compares those fields, so its failure is a divergence of the deployed alarm's stored/echoed configuration. The replay output surfaces only the FIRST failing leg's evidence line in the headline, so the specific field that diverged is not in the report — re-replaying this check prints the per-leg evidence (`[leg n/12] FAIL alarm: ...`).
Why: a client that declares an alarm in CloudFormation/CDK and reads it back to confirm the rollout sees a configuration it did not declare, so alarm period/threshold/missing-data semantics cannot be trusted and an alarm may never evaluate as intended.
Fix: the CloudWatch PutMetricAlarm/DescribeAlarms state and its CloudFormation AWS::CloudWatch::Alarm resource handler (property pass-through and defaulting of ActionsEnabled/TreatMissingData).
Traces: cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow-alarm-echoes-one-minute-period (leg `alarm`).

## Evidence
Check cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow-alarm-echoes-one-minute-period, verdict mismatch:

AssertionError: LEGS 4/12 PASS | bootstrap=PASS deploy=PASS assets=PASS wiring=FAIL logs=BLOCKED metrics=BLOCKED alarm=FAIL denial=BLOCKED drift=BLOCKED rebuild=BLOCKED teardown=BLOCKED toolkit=PASS
WORKFLOW FAILED at wiring: execution ended FAILED: {"errorMessage":"Unknown service: 'glue'. Valid service names are: sqs, events, eventbridg[...]

Cause 1 — expected: the curating Lambda handler's `boto3.client("glue")` succeeds and the Step Functions execution reaches SUCCEEDED with the Glue partition curated. Actual: the execution ends FAILED with the handler payload errorMessage "Unknown service: 'glue'. Valid service names are: sqs, events, eventbridge, ..." (botocore UnknownServiceError from the guest runtime).

Cause 2 — expected: DescribeAlarms(AlarmNames=[<stack alarm>]) returns exactly one MetricAlarm watching (declared namespace, declared metric name, []) with {Statistic: 'Sum', Period: 60, EvaluationPeriods: 1, Threshold: 1.0, ComparisonOperator: 'GreaterThanOrEqualToThreshold', TreatMissingData: 'missing', ActionsEnabled: True}. Actual: the `alarm` leg FAILED (its evidence line is not carried into the replay headline, which shows only the first failing leg).

## Steps to Reproduce
1. Boot the emulator and replay cloudwatch__glue__lambda__logs__s3__stepfunctions.cdk-bootstrapped-sfn-etl-workflow-alarm-echoes-one-minute-period.
2. Cause 1: after the CDK stack deploys, start the state machine over the raw S3 records and describe the execution — it ends FAILED with an errorMessage of "Unknown service: 'glue'. Valid service names are: ...". Equivalently, deploy any Python Lambda whose handler does `import boto3; boto3.client("glue")` and invoke it.
3. Cause 2: call DescribeAlarms with the stack output AlarmName and compare Namespace/MetricName/Dimensions and Statistic, Period, EvaluationPeriods, Threshold, ComparisonOperator, TreatMissingData, ActionsEnabled against the alarm the CDK app declares (Sum over one 60 s period, 1 evaluation period, >= 1, missing data missing, actions enabled).
