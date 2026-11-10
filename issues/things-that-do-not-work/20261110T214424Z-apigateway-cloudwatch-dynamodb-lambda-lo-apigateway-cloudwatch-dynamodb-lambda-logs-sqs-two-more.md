Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn-alarm-echoes-declared-period
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config|_wait_stack#4|AssertionError|raise AssertionError(
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn-alarm-echoes-declared-period|_wait_stack#4|AssertionError|raise AssertionError(
Issue-Commit: 8c5ca24b773ca2762f91097a2e2db632761ff691

# apigateway__cloudwatch__dynamodb__lambda__logs__sqs: two more order-service deployments roll back — the CFN-generated Lambda::Permission StatementId exceeds 100 chars, and a numeric AWS::Lambda::Function property is refused as a string (2 causes)

> _Note: this issue was identified against the code base at commit `8c5ca24b773ca2762f91097a2e2db632761ff691`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Both causes below are the same root causes already described for two SIBLING checks by open issue 20261109T143515Z-apigateway-cloudwatch-dynamodb-lambda-lo-cloudformation-deployed-cdk-sam-app-rolls-back-generated.md, but that issue's replay keys name only .xflavor-order-service-cdk-assetfree and .xflavor-order-service-sam. The two checks below are not covered by it, so they are filed here; fixing either cause should be verified by re-replaying this issue's trace_ids as well.

Cause 1 of 2: CloudFormation generates a physical id for AWS::Lambda::Permission that is longer than Lambda's StatementId maximum, so AddPermission refuses the create and the whole stack rolls back.
AWS: Smithy (lambda) AddPermission.StatementId -> com.amazonaws.lambda#StatementId, @length min 1 max 100, @pattern ^([a-zA-Z0-9-_]+)$. Real AWS deploys this CDK app to CREATE_COMPLETE, so the statement id CloudFormation derives from <stack-name>-<logical-id>-<suffix> is always shortened to fit that 100-character maximum; CloudFormation never hands Lambda a value Lambda's own model forbids.
Emulator: the generated statement id concatenates the full stack name, the full CDK logical id and a 12-character suffix (over 120 characters for a CDK ApiPermissionTest logical id), Lambda refuses it with HTTP 400 and the message '1 validation error detected: Value '<generated statement id>' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100', and the stack ends ROLLBACK_COMPLETE with resource status reason 'Resource handler returned message: "<that message> (Service: Lambda, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. Fixed text: the validation sentence and its constraint clause; variable parts: the statement id (from CFN's own generation), the request id and the request token (per-run values).
Why: no CDK application that attaches an API Gateway method permission to a Lambda function can be deployed at all — the stack rolls back, so nothing downstream of the deploy (API serving, queue delivery, structured logs, metrics, alarm period) can be exercised.
Fix: the physical-id generation of the AWS::Lambda::Permission resource handler in the CloudFormation crate — it must shorten/hash the generated statement id to the modeled 100-character maximum before calling AddPermission.
Traces: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config

Cause 2 of 2: an AWS::Lambda::Function resource whose numeric properties arrive in their template (string) form is refused with 'expected number, got string'.
AWS: recorded behavior only — the same CloudFormation template reaches CREATE_COMPLETE in real AWS. CloudFormation template property values are carried as JSON strings, and the AWS::Lambda::Function handler coerces the numeric properties (MemorySize -> com.amazonaws.lambda#MemorySize, integer @range 128..32768; Timeout -> com.amazonaws.lambda#Timeout, integer @range 1..5400) before calling CreateFunction. It does not refuse the string form, and it never surfaces a deserializer message as a resource-status reason.
Emulator: the create of the IntakeFunction resource fails and the stack ends ROLLBACK_COMPLETE with resource status reason 'Resource handler returned message: "expected number, got string (Service: Lambda, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. Fixed text: 'expected number, got string'; the request id and request token are per-run values. The message names no member and no value, so a client cannot tell which property was rejected.
Why: a plain CloudFormation (and SAM-rendered) Lambda function cannot be deployed at all; the emulator's own request deserializer rejects a shape AWS accepts, so the application never exists and no end-to-end leg of the scenario runs.
Fix: the property deserialization of the AWS::Lambda::Function resource handler in the CloudFormation crate (numeric properties must accept the string form a template carries, then range-validate them), and the Lambda CreateFunction input deserializer's error wording if it is reached at all.
Traces: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn-alarm-echoes-declared-period

## Evidence
Cause 1 — expected: the CDK asset-free order-service stack reaches CREATE_COMPLETE. Actual: AssertionError: stack <stack-name> ended in ROLLBACK_COMPLETE: ApiordersPOSTApiPermissionTestOrderServiceStackApiDF2F37BDPOSTordersC0D5751F: Resource handler returned message: "1 validation error detected: Value '<stack-name>-ApiordersPOSTApiPermissionTestOrderServiceStackApiDF2F37BDPOSTordersC0D5751F-<12-char suffix>' at 'statementId' failed to satisfy constraint: Member must have length less than or equal to 100 (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest).

Cause 2 — expected: the CloudFormation order-service stack reaches CREATE_COMPLETE. Actual: AssertionError: stack <stack-name> ended in ROLLBACK_COMPLETE: IntakeFunction: Resource handler returned message: "expected number, got string (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest).

## Steps to Reproduce
1. Cause 1: synthesize the CDK order-intake app (REST API POST /orders -> Lambda, DynamoDB table, SQS queue, log group, CloudWatch alarm) asset-free, CreateStack it and wait: the stack ends ROLLBACK_COMPLETE on the AWS::Lambda::Permission resource whose generated StatementId is longer than 100 characters.
2. Cause 2: CreateStack the equivalent plain CloudFormation template (order_service_cfn.yaml) and wait: the stack ends ROLLBACK_COMPLETE on IntakeFunction with 'expected number, got string'.
3. Re-replay apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config and apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn-alarm-echoes-declared-period; both must reach CREATE_COMPLETE and then run their wiring/worker/logs/metrics legs.
