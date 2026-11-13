Issue-Source: trace-sweep-divergence
Issue-Trace: lambdacore-create-validation-rejections
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdacore-create-validation-rejections lambdacore-identifier-and-list-validation lambda-core-list-network-connectors-ambient
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core|CreateNetworkConnector|4xx->4xx|$.message
Issue-Key: lambda-core|GetNetworkConnector|4xx->4xx|$.Message
Issue-Key: lambda-core|DeleteNetworkConnector|4xx->2xx|status
Issue-Key: lambda-core|ListNetworkConnectors|4xx->4xx|$.message
Issue-Related: 20261023T022015Z-lambda-core-admits-requests-aws-refuses-and-misclassifies.md
Issue-Commit: 87f40be06f8bd7b2972565eaaefcbb318e4a8aaf

# lambda-core network-connector refusals diverge from AWS: wrong status, wrong error class and emulator-authored messages (6 causes)

> _Note: this issue was identified against the code base at commit `87f40be06f8bd7b2972565eaaefcbb318e4a8aaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkConnector

## Description
Six net-new root causes on the network-connector refusal arms of the lambdacore API (com.amazonaws.lambdacore#LambdaCoreApiService). In three of them the emulator returns a different HTTP status and a different modeled error than AWS; in three of them the status is right but the error/not-found message is emulator-authored rather than the text AWS serves for that error class. Each cause below is independent and must be closed separately.

Cause 1 of 6 - CreateNetworkConnector reports a name conflict (409) instead of refusing an unresolvable subnet reference (400).
AWS: Smithy CreateNetworkConnector (POST /2026-04-04/network-connectors, success code 202) models InvalidParameterValueException (httpError 400) and ResourceConflictException (httpError 409); input member Configuration -> NetworkConnectorConfiguration.VpcEgressConfiguration.SubnetIds (@length 1..16) names EC2 subnets. Recorded behavior: a Configuration naming a subnet that does not exist is refused with HTTP 400, $.Type = 'User' and a message that reports the downstream EC2 call. Its fixed text is "Error occurred while DescribeSubnets. EC2 Error Code: InvalidSubnetID.NotFound. EC2 Error Message: The subnet ID '<subnetId>' does not exist"; the only variable part is <subnetId>, taken verbatim from the request. ResourceConflictException (409) is modeled for a duplicate connector name, not for an EC2 reference that cannot be resolved.
Emulator: answers HTTP 409 with x-amzn-errortype: ResourceConflictException and the message 'A network connector named <name> already exists in this account and Region', and omits $.Type. The conflict exists only because the preceding create in the same fixture - which AWS refused - was persisted anyway, so the next attempt collides on the connector name.
Why: a client is told its chosen name is taken when its real problem is a non-existent subnet, and 409 is a different retry/error class from 400, so SDK error handling branches wrongly; a ghost connector also now occupies the name.
Fix: CreateNetworkConnector's handler must validate the EC2 references in Configuration (and emit InvalidParameterValueException) before any connector state is written; the duplicate-name conflict check stays for genuine name collisions.
Traces: lambdacore-create-validation-rejections#task#5. The open issue 20261023T022015Z-lambda-core-admits-requests-aws-refuses-and-misclassifies.md describes this step but does not carry this replay key (service=lambda-core|CreateNetworkConnector|status=400->409|keys=$.Type,$.message,$headers.x-amzn-errortype,status), so the divergence is still live.

Cause 2 of 6 - GetNetworkConnector on a well-formed cross-account ARN answers 404 absence instead of a 403 access refusal.
AWS: GetNetworkConnector (GET /2026-04-04/network-connectors/{Identifier}) takes NetworkConnectorIdentifier, which accepts an id, a name or a network-connector ARN, and models InvalidParameterValueException (400) and ResourceNotFoundException (404). Recorded behavior: for a syntactically valid network-connector ARN whose account id is not the caller's, the service refuses with HTTP 403 and $.Message = 'Cross-account access is not allowed.' - that message is entirely fixed text with no variable part.
Emulator: answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and 'Network connector not found: <arn>'.
Why: the account-ownership boundary is not modeled at all, so a caller probing another account's ARNs receives an existence answer instead of an authorization denial, and cannot distinguish 'not mine' from 'does not exist'; tests that assert the cross-account denial pass for the wrong reason.
Fix: an account-ownership check in GetNetworkConnector's identifier resolution that returns the 403 access refusal before the not-found path is reached.
Traces: lambdacore-identifier-and-list-validation#task#3. Related: 20261023T022015Z-lambda-core-admits-requests-aws-refuses-and-misclassifies.md (same step, different replay key: that issue carries the $.message key, the live divergence is on $.Message/status).

Cause 3 of 6 - DeleteNetworkConnector on an identifier that does not exist answers 202 success instead of 404 ResourceNotFoundException.
AWS: Smithy DeleteNetworkConnector (DELETE /2026-04-04/network-connectors/{Identifier}, success code 202) models ResourceNotFoundException with httpError 404, documented as 'The specified network connector does not exist. Verify the identifier (ID, name, or ARN) and Region.' Recorded behavior: deleting an identifier that is not present answers HTTP 404 with the ResourceNotFoundException envelope.
Emulator: answers HTTP 202 - the operation's success code - as though a connector had been scheduled for deletion.
Why: a client cannot tell a real delete from a no-op, so cleanup logic and 'delete then assert gone' flows report success against connectors that were never there, masking identifier bugs.
Fix: DeleteNetworkConnector's handler must resolve the identifier against state and raise ResourceNotFoundException when it is absent, instead of returning the 202 acknowledgement unconditionally. Related resolved issue 20261027T060836Z-lambda-core-deletenetworkconnector-is-not-idempotent-once-deletion.md covered the post-deletion idempotency path; this is the absent-identifier path and is not covered by it.
Traces: lambda-core-list-network-connectors-ambient#teardown#14.

Cause 4 of 6 - the ResourceNotFoundException message names the emulator's internally generated connector id instead of the identifier from the request.
AWS: the not-found message's fixed text is 'Network connector not found: ' followed by the identifier exactly as the caller supplied it on the request path - NetworkConnectorIdentifier accepts an id, a name or a full network-connector ARN, and whichever form was sent is the form echoed back.
Emulator: substitutes its own generated connector id for the supplied identifier (the replayed run shows 'Network connector not found: nc-0000000000000001' where the request addressed the connector by its ARN), so the echoed value is one the caller never sent and cannot correlate.
Why: the identifier is the one actionable datum in the refusal; echoing an internal id makes the error unusable for diagnosing a wrong ARN, region or account, and breaks clients that parse the identifier back out of the message.
Fix: the shared not-found formatter behind GetNetworkConnector must interpolate the raw request identifier, not the resolved/stored connector id. The resolved issue 20261023T022015Z-lambda-core-rejection-messages-diverge-from-aws-not.md fixed this class of defect on another path; this GetNetworkConnector path still echoes the wrong identifier.
Traces: lambda-core-list-network-connectors-ambient#task#12.

Cause 5 of 6 - CreateNetworkConnector's enum-constraint validation message carries the wrong value set for AssociatedComputeResourceTypes and drops the not-null clause.
AWS: recorded behavior, HTTP 400, with the standard AWS validation-error message. Its fixed text is "1 validation error detected: Value '[<value>]' at 'configuration.vpcEgressConfiguration.associatedComputeResourceTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [WebFunction, MicroVm, AppSync], Member must not be null]" - the member path, the value set with its published ordering [WebFunction, MicroVm, AppSync] and the trailing 'Member must not be null' clause are all fixed text; the only variable part is <value>, the rejected member echoed from the request. Note the on-disk model's ComputeResourceType enumerates only [MicroVm, WebFunction]; the recorded service's set is wider, so the recording is the requirement here.
Emulator: emits the same prefix but ends "...Member must satisfy constraint: [Member must satisfy enum value set: [MicroVm, WebFunction]]" - model declaration order, AppSync absent, and the 'Member must not be null' clause missing.
Why: clients and operators read the accepted value set out of this message; an emulator that advertises a narrower set in a different order teaches the wrong contract, and consumers that assert on the message text diverge.
Fix: the enum value-set renderer used by CreateNetworkConnector's configuration validation must emit AWS's published value list and ordering for ComputeResourceType and append the 'Member must not be null' clause for a list-member constraint.
Traces: lambdacore-create-validation-rejections#task#3.

Cause 6 of 6 - ListNetworkConnectors' State enum-constraint message lists the value set in model declaration order instead of AWS's order.
AWS: ListNetworkConnectors (GET /2026-04-04/network-connectors) binds State to the httpQuery parameter 'State' targeting NetworkConnectorState, and models InvalidParameterValueException (400). Recorded behavior, HTTP 400, fixed text "1 validation error detected: Value '<value>' at 'state' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, DELETING, INACTIVE, DELETE_FAILED, PENDING, FAILED]", variable part <value> echoed from the query string. That ordering is part of the fixed text.
Emulator: renders the same sentence with the set as [PENDING, ACTIVE, INACTIVE, FAILED, DELETING, DELETE_FAILED], i.e. the Smithy NetworkConnectorState declaration order.
Why: same as cause 5 - the message is the documented contract for the parameter, and a reordered set is a visible text divergence for every client that validates or displays it.
Fix: the same shared enum value-set renderer, given AWS's ordering for NetworkConnectorState rather than the model's declaration order. Causes 5 and 6 are both renderer-ordering defects but on different enums and different operations, so both must be closed.
Traces: lambdacore-identifier-and-list-validation#task#4.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout (score 72.7%, 12 mismatched steps, bootstrap_signal=false, emulator_up=true).

Cause 1 - lambdacore-create-validation-rejections#task#5, CreateNetworkConnector: status expected 400, got 409; $.message expected "Error occurred while DescribeSubnets. EC2 Error Code: InvalidSubnetID.NotFound. EC2 Error Message: The subnet ID 'subnet-0123456789abcdef0' does not exist", got 'A network connector named sim2real-probe-lc-reject-0001 already exists in this account and Region'; $headers.x-amzn-errortype got 'ResourceConflictException'; $.Type missing (expected 'User'). divergence_key service=lambda-core|CreateNetworkConnector|status=400->409|keys=$.Type,$.message,$headers.x-amzn-errortype,status.

Cause 2 - lambdacore-identifier-and-list-validation#task#3, GetNetworkConnector: status expected 403, got 404; $.Message expected 'Cross-account access is not allowed.', got 'Network connector not found: arn:aws:lambda:us-west-2:111122223333:network-connector:nc-11111111-2222-3333-4444-555555555555'; $headers.x-amzn-errortype got 'ResourceNotFoundException'. divergence_key service=lambda-core|GetNetworkConnector|status=403->404|keys=$.Message,$headers.x-amzn-errortype,status.

Cause 3 - lambda-core-list-network-connectors-ambient#teardown#14, DeleteNetworkConnector: status expected 404, got 202. divergence_key service=lambda-core|DeleteNetworkConnector|status=404->202|keys=status.

Cause 4 - lambda-core-list-network-connectors-ambient#task#12, GetNetworkConnector: status 404 on both sides; $.Message value_mismatch, expected 'Network connector not found: arn:aws:lambda:us-west-2:000000000000:network-connector:sim2real-lnc-2d4d1732' (the identifier as requested), got 'Network connector not found: nc-0000000000000001' (an emulator-generated id). divergence_key service=lambda-core|GetNetworkConnector|status=404->404|keys=$.Message.

Cause 5 - lambdacore-create-validation-rejections#task#3, CreateNetworkConnector: status 400 on both sides; $.message value_mismatch, expected "1 validation error detected: Value '[Sim2RealProbe]' at 'configuration.vpcEgressConfiguration.associatedComputeResourceTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [WebFunction, MicroVm, AppSync], Member must not be null]", got "1 validation error detected: Value '[Sim2RealProbe]' at 'configuration.vpcEgressConfiguration.associatedComputeResourceTypes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [MicroVm, WebFunction]]".

Cause 6 - lambdacore-identifier-and-list-validation#task#4, ListNetworkConnectors: status 400 on both sides; $.message value_mismatch, expected "1 validation error detected: Value 'SIM2REALPROBE' at 'state' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, DELETING, INACTIVE, DELETE_FAILED, PENDING, FAILED]", got the same sentence with [PENDING, ACTIVE, INACTIVE, FAILED, DELETING, DELETE_FAILED].

Model references used (services/service-com-amazonaws-lambdacore-lambdacoreapiservice/smithy-model/com-amazonaws-lambdacore-lambdacoreapiservice.json): CreateNetworkConnector errors InvalidParameterValueException/400, ResourceConflictException/409, http code 202; GetNetworkConnector errors InvalidParameterValueException/400, ResourceNotFoundException/404; DeleteNetworkConnector errors ResourceNotFoundException/404, http code 202; ListNetworkConnectors State -> NetworkConnectorState via httpQuery, errors InvalidParameterValueException/400; ComputeResourceType values [MicroVm, WebFunction]; NetworkConnectorState values [PENDING, ACTIVE, INACTIVE, FAILED, DELETING, DELETE_FAILED]; ResourceNotFoundException doc 'The specified network connector does not exist. Verify the identifier (ID, name, or ARN) and Region.'

## Steps to Reproduce
1. git_clone, then replay_wire_corpus() to replay simparity-wire-aws-v1.
2. Cause 1: fixture lambdacore-create-validation-rejections, step task#5 - POST /2026-04-04/network-connectors with a VpcEgressConfiguration naming a subnet that does not exist; observe 409 ResourceConflictException on the connector name instead of 400 InvalidParameterValueException reporting the DescribeSubnets failure.
3. Cause 2: fixture lambdacore-identifier-and-list-validation, step task#3 - GET /2026-04-04/network-connectors/<network-connector ARN in another account>; observe 404 ResourceNotFoundException instead of 403 'Cross-account access is not allowed.'
4. Cause 3: fixture lambda-core-list-network-connectors-ambient, step teardown#14 - DELETE /2026-04-04/network-connectors/<identifier that does not exist>; observe 202 instead of 404 ResourceNotFoundException.
5. Cause 4: fixture lambda-core-list-network-connectors-ambient, step task#12 - GET /2026-04-04/network-connectors/<network-connector ARN that does not exist>; observe the 404 message echoing an emulator-generated connector id instead of the ARN supplied on the path.
6. Cause 5: fixture lambdacore-create-validation-rejections, step task#3 - POST /2026-04-04/network-connectors with AssociatedComputeResourceTypes = [an out-of-enum value]; compare the validation message's value set and trailing clause.
7. Cause 6: fixture lambdacore-identifier-and-list-validation, step task#4 - GET /2026-04-04/network-connectors?State=<out-of-enum value>; compare the ordering of the enum value set in the validation message.
