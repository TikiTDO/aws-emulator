Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: braket__cloudformation.deploy-braket-spendinglimit-round-trip-and-delete braket__cloudformation.deploy-braket-spendinglimit-simulator-device-rejected
Issue-Key: braket__cloudformation|CreateStack|ValidationError
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# braket__cloudformation: CreateStack refuses AWS::Braket::SpendingLimit as an unrecognized resource type, so no Braket spending-limit stack can deploy, roll back, or be read back (1 cause)

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template resource-type registry does not contain AWS::Braket::SpendingLimit, so every CreateStack whose template declares that type is refused outright at template parse time. Both braket__cloudformation checks die on their very first CreateStack and never reach any Braket assertion, so the whole SpendingLimit deploy contract (create, Ref/GetAtt, delete, and the simulator refusal path) is unverifiable.

Failing check ids: braket__cloudformation.deploy-braket-spendinglimit-round-trip-and-delete, braket__cloudformation.deploy-braket-spendinglimit-simulator-device-rejected.

AWS: AWS::Braket::SpendingLimit is a supported CloudFormation resource type backed by the Braket spending-limit API. Smithy (service-com-amazonaws-braket-braket): CreateSpendingLimit POST /spending-limit, HTTP 201, required input members clientToken (@idempotencyToken, @length 1..64), deviceArn (DeviceArn, @length 1..256) and spendingLimit (@pattern ^\d+(\.\d{1,2})?$), optional timePeriod and tags; output spendingLimitArn is required, @pattern ^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:spending-limit/.*$; modeled errors include ValidationException (400), DeviceRetiredException (410), AccessDeniedException (403). SearchSpendingLimits POST /spending-limits, HTTP 200, paginated on nextToken/maxResults with required output member spendingLimits. Recorded behavior only for the CloudFormation side: on real AWS the recording shows CreateStack ACCEPTING the template below and the stack reaching CREATE_COMPLETE, with Ref resolving to the spending-limit ARN, GetAtt SpendingLimitArn and GetAtt CreatedAt resolving, the limit appearing in SearchSpendingLimits filtered by deviceArn, and DeleteStack removing it from that listing. For a SIMULATOR DeviceArn the recording shows the template is NOT refused by early template validation (DescribeEvents reports no VALIDATION_ERROR rows); the resource handler fails and the stack ends ROLLBACK_COMPLETE with the Limit resource's CREATE_FAILED ResourceStatusReason of the form: Resource handler returned message: "Spending limits can not be created for simulator devices. (Service: Braket; Operation: CreateSpendingLimit; Status Code: 400; Error Code: ValidationException; Request ID: <request id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest). In that reason the fixed text is "Spending limits can not be created for simulator devices.", the "(Service: Braket; Operation: CreateSpendingLimit; Status Code: 400; Error Code: ValidationException; Request ID: ...)" envelope and the "(RequestToken: ..., HandlerErrorCode: InvalidRequest)" suffix; the request id and request token are per-run values, not part of the requirement.

Emulator: CreateStack is refused immediately with HTTP 400, error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::Braket::SpendingLimit]". That wording is AWS's wording for a type it genuinely does not know; the defect is that the emulator issues it for a type AWS supports. Because the call raises, no stack id is ever returned and nothing downstream runs: no stack events, no Outputs, no Ref/GetAtt resolution, no rollback, and the simulator-rejection handler path is never exercised either.

Why: a client deploying Braket cost controls as infrastructure-as-code cannot deploy at all against the emulator — the template is rejected before any resource is attempted, which is indistinguishable from a template authoring error. Clients that branch on ValidationError/"Unrecognized resource types" to detect typos or unsupported regions will conclude the type does not exist, and the entire rollback/refusal contract (a simulator DeviceArn must produce ROLLBACK_COMPLETE with an InvalidRequest handler reason, never a template-format error) is replaced by a parse-time failure of the wrong class and at the wrong phase.

Fix: CloudFormation's template resource-type registry/parser (the component that resolves a Resources[*].Type to a provisioning handler) must recognize AWS::Braket::SpendingLimit, plus a resource handler for it that maps Properties DeviceArn and SpendingLimit onto the Braket CreateSpendingLimit/DeleteSpendingLimit state, returns the spending-limit ARN for Ref and exposes SpendingLimitArn and CreatedAt as GetAtt attributes, and surfaces the Braket ValidationException for a SIMULATOR deviceArn as a CREATE_FAILED resource reason (HandlerErrorCode InvalidRequest) driving ROLLBACK_COMPLETE rather than an early template validation error. The Braket side must keep the created limit visible through SearchSpendingLimits filtered by deviceArn and remove it on stack delete. Note: this is distinct from the already-open CloudFormation outputs issue 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md, which concerns Outputs/OutputValue for types the emulator already recognizes; here the type is rejected before any stack exists.

Traces: braket__cloudformation.deploy-braket-spendinglimit-round-trip-and-delete (template with a single "Limit" resource of Type AWS::Braket::SpendingLimit, Properties DeviceArn=<first ONLINE QPU ARN from SearchDevices>, SpendingLimit="5.00", Outputs LimitRef={Ref: Limit}, SpendingLimitArn={Fn::GetAtt: [Limit, SpendingLimitArn]}, CreatedAt={Fn::GetAtt: [Limit, CreatedAt]}); braket__cloudformation.deploy-braket-spendinglimit-simulator-device-rejected (the same template once with a SIMULATOR DeviceArn, expected to roll back, and once with a QPU DeviceArn as the positive control).

## Evidence
Check braket__cloudformation.deploy-braket-spendinglimit-round-trip-and-delete and braket__cloudformation.deploy-braket-spendinglimit-simulator-device-rejected, both [error], identical failure:

  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Braket::SpendingLimit]

Expected (recorded AWS behavior):
- round-trip check: CreateStack returns a StackId; DescribeStacks reaches StackStatus CREATE_COMPLETE; Outputs carry LimitRef (the spending-limit ARN), SpendingLimitArn (== the ARN SearchSpendingLimits reports) and CreatedAt (ISO-8601 timestamp equal to the summary's createdAt); SearchSpendingLimits with filter {name: deviceArn, values: [<qpu arn>], operator: EQUAL} lists exactly one entry with deviceArn == the templated ARN and spendingLimit == 5.00; after DeleteStack the entry is gone.
- rejection check: the SIMULATOR stack reaches ROLLBACK_COMPLETE with NO VALIDATION_ERROR operation events, and the "Limit" resource's single CREATE_FAILED ResourceStatusReason parses to message "Spending limits can not be created for simulator devices. (Service: Braket; Operation: CreateSpendingLimit; Status Code: 400; Error Code: ValidationException; Request ID: <id>)" with HandlerErrorCode "InvalidRequest"; SearchSpendingLimits lists no limit for the simulator; the QPU control stack reaches CREATE_COMPLETE and its Ref appears in SearchSpendingLimits with spendingLimit 5.00.

Actual (emulator): CreateStack raises ValidationError / "Template format error: Unrecognized resource types: [AWS::Braket::SpendingLimit]" before any stack is created, so no StackId, no stack events, no Outputs, no rollback and no Braket state change ever occur. Both checks abort at the first CreateStack.

## Steps to Reproduce
1. Point a botocore client at the emulator's cloudformation and braket endpoints in us-east-1.
2. braket.search_devices(filters=[]) and pick the first ONLINE device with deviceType == "QPU" (and, for the second check, the first ONLINE "SIMULATOR").
3. Build the template:
   {"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Limit": {"Type": "AWS::Braket::SpendingLimit", "Properties": {"DeviceArn": "<device arn>", "SpendingLimit": "5.00"}}}, "Outputs": {"LimitRef": {"Value": {"Ref": "Limit"}}, "SpendingLimitArn": {"Value": {"Fn::GetAtt": ["Limit", "SpendingLimitArn"]}}, "CreatedAt": {"Value": {"Fn::GetAtt": ["Limit", "CreatedAt"]}}}}
4. cloudformation.create_stack(StackName=<unique>, TemplateBody=<template>, Tags=[{"Key": "sim2real", "Value": <unique>}]).
   Observed: ClientError ValidationError "Template format error: Unrecognized resource types: [AWS::Braket::SpendingLimit]".
   Expected: a StackId, then DescribeStacks -> CREATE_COMPLETE (QPU) or ROLLBACK_COMPLETE with the InvalidRequest handler reason above (SIMULATOR).
5. For the QPU stack, poll DescribeStacks to CREATE_COMPLETE, read Outputs, and cross-check braket.search_spending_limits(filters=[{"name": "deviceArn", "values": ["<qpu arn>"], "operator": "EQUAL"}]); then delete_stack and confirm the limit no longer lists.
