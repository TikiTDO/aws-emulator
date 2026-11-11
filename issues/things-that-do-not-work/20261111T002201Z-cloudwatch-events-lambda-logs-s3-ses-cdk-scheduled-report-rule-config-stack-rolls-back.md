Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-rule-config-workflow
Issue-Key: cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-rule-config-workflow|_deploy_cfn#8|AssertionError|raise AssertionError(
Issue-Commit: f28dfafa5fc9e548e0b0df8415a880c6df425385

# CDK scheduled-report rule-config stack rolls back: the CloudFormation-generated AWS::Lambda::Permission StatementId exceeds Lambda's 100-character maximum

> _Note: this issue was identified against the code base at commit `f28dfafa5fc9e548e0b0df8415a880c6df425385`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Cause 1 of 1: CloudFormation generates a statement id for AWS::Lambda::Permission that is longer than the modelled maximum, so Lambda refuses the AddPermission call and the whole stack rolls back.

NOTE: the open issue 20261109T143515Z-cloudwatch-events-lambda-logs-s3-ses-cdk-scheduled-report-stack-rolls-back-the-generated.md describes the same root cause, but it carries the replay key of a DIFFERENT check (cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-workflow). The failing check filed here (…cdk-scheduled-report-email-rule-config-workflow) is not owned by it, so it is filed with its own replay key; closing both together is expected. Related, same mechanism in other suites: 20261105T132157Z-cloudformation-8-net-new-root-causes-in-the-684719.md and 20261109T143515Z-apigateway-cloudwatch-dynamodb-lambda-lo-cloudformation-deployed-cdk-sam-app-rolls-back-generated.md.

AWS: Smithy (lambda) AddPermission.StatementId -> com.amazonaws.lambda#StatementId, required, @length min 1 max 100, @pattern ^([a-zA-Z0-9-_]+)$; AddPermission answers 400 InvalidParameterValueException for an input that violates a member constraint. Real AWS deploys this CDK app to CREATE_COMPLETE, so the statement id real CloudFormation generates for an AWS::Lambda::Permission (the resource type has no StatementId template property — CloudFormation synthesizes it) always fits inside that 100-character limit, however long the stack name and CDK logical id are.

Emulator: the generated statement id is the plain concatenation <stack-name>-<full CDK logical id>-<12 hex chars> with no truncation, which is 104+ characters for this stack, and Lambda (correctly) refuses it. The AWS::Lambda::Permission resource therefore fails, the stack ends ROLLBACK_COMPLETE, and DescribeStackEvents reports the resource status reason 'Resource handler returned message: "1 validation error detected: Value \'<generated statement id>\' at \'statementId\' failed to satisfy constraint: Member must have length less than or equal to 100 (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. Fixed text: the "1 validation error detected: Value ... at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100" sentence and the wrapping handler-message/RequestToken/HandlerErrorCode frame; per-run values: the generated id, the stack name, the request id and the request token.

Why: every CDK or CloudFormation app that grants a principal (here events.amazonaws.com, for a rate(1 minute) EventBridge rule target) permission to invoke a Lambda function is undeployable whenever the stack name plus logical id is long — the stack rolls back instead of reaching CREATE_COMPLETE, so none of the downstream behaviour (scheduled invocation, S3 report object, SES send, structured log, metric datapoint, alarm, drift re-apply, delete) can be exercised at all.

Fix: physical-id/statement-id generation in the CloudFormation crate's AWS::Lambda::Permission resource handler — the synthesized id must be shortened (truncate the stack-name/logical-id components) so it always satisfies Lambda's modelled 100-character StatementId maximum.

Traces: cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-rule-config-workflow.

## Evidence
Expected (real AWS): the synthesized CDK stack reaches CREATE_COMPLETE and exposes outputs BucketName, FunctionName, LogGroupName, RuleName, AlarmName, MetricNamespace.
Actual (emulator): AssertionError: stack <stack-name> ended in ROLLBACK_COMPLETE: ScheduleAllowEventRuleScheduledReportStackFn<hash>=Resource handler returned message: "1 validation error detected: Value '<stack-name>-ScheduleAllowEventRuleScheduledReportStackFn<hash>-<12 hex>' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100 (Service: Lambda, Status Code: 400, Request ID: <id>)" — the generated id is 104 characters where Lambda's StatementId limit is 100.

## Steps to Reproduce
Replay cloudwatch__events__lambda__logs__s3__ses.cdk-scheduled-report-email-rule-config-workflow: `cdk synth` the asset-free scheduled_report_cdk app and CreateStack the template, which contains an AWS::Lambda::Permission granting events.amazonaws.com lambda:InvokeFunction on the report function. Poll DescribeStacks: the stack ends ROLLBACK_COMPLETE and DescribeStackEvents names the Permission resource with the over-100-character statementId validation failure.
