Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-cancel-task-notfound-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-cancel-task-notfound-error-paths ec2-local-gateway-create-error-paths ec2-local-gateway-route-table-not-found
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CancelImportTask|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CancelImportTask|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateLocalGatewayRouteTable|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# EC2 refusals use emulator-authored error codes and message wording for unknown ids (2 causes)

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelImportTask

## Description
Two refusal paths return the right HTTP status but not AWS's error code or message text, so clients that branch on Code or parse Message do not recognise the failure. 2 causes.

Cause 1 of 2: CancelImportTask with an id that is not a valid import-task id (fixture ec2-cancel-task-notfound-error-paths, step task#3).
AWS: HTTP 400 with Response.Errors.Error.Code 'InvalidID' and Message "The ID '<import-task-id>' is not valid". Fixed text: "The ID '" ... "' is not valid"; the only variable part is the id copied from the request's ImportTaskId. (Smithy: CancelImportTask models no errors - errors: [] - so this is recorded behavior only; the EC2 Query protocol carries the code/message in the Response.Errors.Error envelope.)
Emulator: HTTP 400, but Code 'InvalidConversionTaskId' and Message 'Invalid id: "<id>"' - an emulator-authored code that is not an EC2 error code for this operation, and a message that neither uses AWS's wording nor its quoting.
Why: a client with a retry/branch table keyed on 'InvalidID' (or on AWS's message text) treats the response as an unknown failure; nothing maps 'InvalidConversionTaskId' to AWS behavior.
Fix: the CancelImportTask handler's id validation - it must raise the InvalidID error class with AWS's message template rather than a per-operation conversion-task code.
Traces: ec2-cancel-task-notfound-error-paths#task#3.

Cause 2 of 2: CreateLocalGatewayRouteTable with an unknown local-gateway id (fixture ec2-local-gateway-create-error-paths, step task#2).
AWS: HTTP 400 with Message "The LocalGateway ID '<lgw-id>' does not exist". Fixed text: "The LocalGateway ID '" ... "' does not exist"; the variable part is the LocalGatewayId from the request. The error Code already matches; only the wording of the message diverges.
Emulator: Message "The local-gateway ID '<lgw-id>' does not exist" - the resource name is spelled in hyphenated lower case instead of AWS's 'LocalGateway'.
Why: callers that match on AWS's message text (and humans reading logs against AWS docs) see a message AWS never emits; the recorded cluster also covers the route-table lookup path, so the same wrong wording is served on two flows.
Fix: the shared not-found message formatting for local-gateway ids used by the CreateLocalGatewayRouteTable handler's validation.
Traces: ec2-local-gateway-create-error-paths#task#2 (representative; cluster also covers ec2-local-gateway-route-table-not-found#task#1).

## Evidence
Cause 1 - fixture ec2-cancel-task-notfound-error-paths step task#3, operation CancelImportTask, expected 400 / actual 400:
  $.Response.Errors.Error.Code: expected 'InvalidID', got 'InvalidConversionTaskId'
  $.Response.Errors.Error.Message: expected "The ID 'import-fffffff0' is not valid", got 'Invalid id: "import-fffffff0"'
  (the id 'import-fffffff0' is the request value filling the template; the fixed text is "The ID '...' is not valid")

Cause 2 - fixture ec2-local-gateway-create-error-paths step task#2, operation CreateLocalGatewayRouteTable, expected 400 / actual 400:
  $.Response.Errors.Error.Message: expected "The LocalGateway ID 'lgw-0123456789abcdef0' does not exist", got "The local-gateway ID 'lgw-0123456789abcdef0' does not exist"
  cluster affected: ec2-local-gateway-create-error-paths#task#2, ec2-local-gateway-route-table-not-found#task#1

Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model): CancelImportTask errors [] (no modeled error shapes); error code/message are carried in the EC2 Query Response.Errors.Error envelope.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Cause 1: replay fixture ec2-cancel-task-notfound-error-paths step task#3 (CancelImportTask with a malformed import-task id). Observe Code 'InvalidConversionTaskId' and Message 'Invalid id: "..."' where AWS returns Code 'InvalidID' and Message "The ID '<id>' is not valid".
3. Cause 2: replay fixture ec2-local-gateway-create-error-paths step task#2 (CreateLocalGatewayRouteTable with an unknown lgw id). Observe the message naming the resource as 'local-gateway' instead of AWS's 'LocalGateway'.
