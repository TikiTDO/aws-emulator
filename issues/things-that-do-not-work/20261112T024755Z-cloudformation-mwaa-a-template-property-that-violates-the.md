Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mwaa.deploy-mwaa-environment-network-configuration-single-subnet-rejected
Issue-Key: cloudformation__mwaa.deploy-mwaa-environment-network-configuration-single-subnet-rejected|run#16|AssertionError|assert reason == want_reason, f"reason {reason!r}, expected {want_reason!r}"
Issue-Commit: 4123aa4e43568a1d6639765fc2c5592729132419

# cloudformation__mwaa: a template property that violates the resource type's schema is handed to the resource handler instead of failing CloudFormation's own property validation, so the CREATE_FAILED reason is the relayed handler message, not "Properties validation failed for resource <LogicalId>"

> _Note: this issue was identified against the code base at commit `4123aa4e43568a1d6639765fc2c5592729132419`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation does not validate a resource's Properties against the registered resource type's schema before invoking the resource handler, so a template value the schema alone already rejects is reported with the downstream service's API validation message rather than CloudFormation's pre-handler property-validation reason.

Failing check ids for this cause: cloudformation__mwaa.deploy-mwaa-environment-network-configuration-single-subnet-rejected

AWS: An AWS::MWAA::Environment whose NetworkConfiguration.SubnetIds names fewer than the schema's minimum number of subnets never reaches the MWAA resource handler. CloudFormation validates the resource's Properties against the resource type's JSON schema first, fails the resource at that stage, and writes a CREATE_FAILED event whose ResourceStatusReason is, recorded behavior (the CFN resource-type schema is not a Smithy model):

  Properties validation failed for resource <LogicalResourceId> with message:\n#<JSON pointer of the offending property>: expected minimum item count: <schema minimum>, found: <count supplied by the template>

Fixed text: "Properties validation failed for resource ", " with message:", the newline, "expected minimum item count: ", ", found: ". Variable parts, all taken from the request/template and the resource schema: the logical resource id (here MwaaEnvironment), the JSON pointer into the resource's Properties (here /NetworkConfiguration/SubnetIds), the schema's minimum item count and the number of items the template actually supplied. The stack then rolls back to ROLLBACK_COMPLETE and the environment is never created. The underlying bound is real — Smithy: mwaa NetworkConfiguration.SubnetIds targets SubnetList with @length min 2, max 2 — but at AWS it is enforced by CloudFormation's property validation before any handler call, so the handler's own wording never appears.

Emulator: the stack does roll back to ROLLBACK_COMPLETE, but the MwaaEnvironment CREATE_FAILED reason is the relayed handler envelope:

  Resource handler returned message: "1 validation error detected: Value '1 items' at 'networkConfiguration.subnetIds' failed to satisfy constraint: Member must have length greater than or equal to 2" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)

i.e. the emulator forwarded the schema-invalid Properties to the MWAA CreateEnvironment handler and surfaced MWAA's API-level constraint message (lower-camel wire member path, "1 validation error detected", HandlerErrorCode: InvalidRequest) instead of CloudFormation's own "Properties validation failed for resource ..." reason. No property-schema validation stage runs at all.

Why: a client that reads DescribeStackEvents to classify a deploy failure cannot distinguish a template/property defect from a service-side rejection. Tooling and CI that match CloudFormation's "Properties validation failed for resource <LogicalId> with message:\n#/<pointer>: ..." reason — the form CDK, pipelines and change-review automation parse to point at the offending template property — sees an unrelated string, loses the JSON pointer to the bad property, and gets a HandlerErrorCode for a failure AWS never attributes to a handler. The same gap means every property-schema rule (minItems/maxItems, required, enum, pattern) is silently delegated to whatever the downstream service happens to check, so schema violations the owning API does not police would be accepted outright.

Fix: CloudFormation's resource-provisioning path — add the property-schema validation stage that runs against the registered resource type's schema before the resource handler is invoked, and have the CREATE_FAILED event writer emit CloudFormation's property-validation reason (with the resource's logical id and the JSON pointer of the failing property) rather than the handler-relay envelope for failures detected at that stage.

Traces: cloudformation__mwaa.deploy-mwaa-environment-network-configuration-single-subnet-rejected (assertion at check line 331: reason != want_reason).

## Evidence
Check: cloudformation__mwaa.deploy-mwaa-environment-network-configuration-single-subnet-rejected (mismatch)

AssertionError at check line 331:
  reason 'Resource handler returned message: "1 validation error detected: Value \'1 items\' at \'networkConfiguration.subnetIds\' failed to satisfy constraint: Member must have length greater than or equal to 2" (RequestToken: d04ddef0-9d7f-44ba-69ca-81aadd601900, HandlerErrorCode: InvalidRequest)',
  expected 'Properties validation failed for resource MwaaEnvironment with message:\n#/NetworkConfiguration/SubnetIds: expected minimum item count: 2, found: 1'

Expected (AWS, recorded): CREATE_FAILED ResourceStatusReason for logical resource MwaaEnvironment =
  "Properties validation failed for resource MwaaEnvironment with message:\n#/NetworkConfiguration/SubnetIds: expected minimum item count: 2, found: 1"
(the request token and the generated stack/environment names in the recording are per-run values, not part of the requirement)

Actual (emulator): the MWAA CreateEnvironment handler was invoked and its API validation message was relayed as the resource status reason, with HandlerErrorCode: InvalidRequest.

Preceding assertions passed: the two-subnet control change set reached CREATE_COMPLETE with no validation errors and planned Add AWS::MWAA::Environment, and the one-subnet stack reached ROLLBACK_COMPLETE. Only the reason text diverges.

Smithy (mwaa, com-amazonaws-mwaa-amazonmwaa.json): NetworkConfiguration.SubnetIds -> SubnetList, @length min 2 max 2; member SubnetId pattern ^subnet-[a-zA-Z0-9\-._]+$. The bound is real; at AWS it is enforced by CloudFormation property validation before the handler runs.

## Steps to Reproduce
1. CreateStack with a template declaring AWS::MWAA::Environment (plus the supporting IAM role, S3 bucket, VPC/subnet/security-group resources) where Properties.NetworkConfiguration.SubnetIds lists exactly ONE subnet id, with Capabilities=["CAPABILITY_IAM"].
2. Poll DescribeStacks(StackName=<StackId>) until terminal: the stack reaches ROLLBACK_COMPLETE (as AWS does).
3. Page DescribeStackEvents(StackName=<StackId>) and read the ResourceStatusReason of the CREATE_FAILED event for LogicalResourceId "MwaaEnvironment".
4. Observed: 'Resource handler returned message: "1 validation error detected: Value \'1 items\' at \'networkConfiguration.subnetIds\' failed to satisfy constraint: Member must have length greater than or equal to 2" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'.
   Expected: 'Properties validation failed for resource MwaaEnvironment with message:\n#/NetworkConfiguration/SubnetIds: expected minimum item count: 2, found: 1'.
5. Control (passes today, keep it green): the same template with TWO subnet ids, submitted as a ChangeSetType=CREATE change set, reaches CREATE_COMPLETE with no validation errors and plans ("Add", "AWS::MWAA::Environment").
