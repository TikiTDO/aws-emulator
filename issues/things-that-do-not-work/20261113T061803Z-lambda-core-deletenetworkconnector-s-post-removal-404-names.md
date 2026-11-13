Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-core.network-connector-delete-transitions-to-removed
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core.network-connector-delete-transitions-to-removed|run#15|AssertionError|assert redelete_message == f"Network connector not found: {created['Arn']}", (
Issue-Commit: 4b113afbdbe9419578fc08977456c20d7d6eb702

# lambda-core DeleteNetworkConnector's post-removal 404 names the bare connector Id instead of the ARN that Id determines

> _Note: this issue was identified against the code base at commit `4b113afbdbe9419578fc08977456c20d7d6eb702`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNetworkConnector

## Description
One net-new root cause on lambda-core's network-connector delete path (com.amazonaws.lambdacore#LambdaCoreApiService), grounded in the single failing lambda-core check that is neither ALREADY FILED (network-connector-dualstack-network-protocol-persists, blocked by the unimplemented EC2 CreateSubnet/AssociateVpcCidrBlock stub) nor OWNED (network-connector-create-nonexistent-subnet-rejected).

Cause 1 of 1 - DeleteNetworkConnector on an identifier whose connector has already been fully removed is now correctly refused with HTTP 404 ResourceNotFoundException, but the message echoes the request's identifier byte-for-byte, so a request addressed by the connector's bare Id is refused naming that Id where AWS names the ARN the Id determines.

AWS: Smithy DeleteNetworkConnector (DELETE /2026-04-04/network-connectors/{Identifier}, success 202, idempotent:true) takes Identifier -> NetworkConnectorIdentifier (httpLabel, required, length 1-140), "a flexible identifier that accepts a network connector ID, name, or ARN", and models ResourceNotFoundException at httpError 404. Recorded behavior: once the deletion has completed (GetNetworkConnector on the same identifier already answers ResourceNotFoundException), a repeat delete is refused 404 / ResourceNotFoundException with the message "Network connector not found: <connectorArn>". In that message "Network connector not found: " is FIXED text and the only variable part is the resource the identifier determines: the connector's unqualified ARN arn:aws:lambda:<region>:<account-id>:network-connector:<Id> (version-qualified when the request carried a version suffix). Which form that is follows from the identifier itself and not from the operation: an ARN is already one, and an Id IS an ARN's resource part (NetworkConnectorArn pattern ...:network-connector:[a-zA-Z0-9-_]+), so an Id-addressed request is refused naming the ARN it determines; a NAME determines no ARN and is stated exactly as supplied. This emulator already encodes exactly that rule in services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/state.rs (`connector_not_found_resource`, lines 618-640) - it is simply not the rule the delete path uses, and the doc comment there asserts the ARN form belongs only to the "reporting" operation, which the recording contradicts for delete.

Emulator: the delete handler's not-found arm calls `crate::state::connector_not_found(&input.identifier)` (services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/ops/delete_network_connector.rs lines 215-220, and again lines 257-259 for the version arm), and that helper formats NOT_FOUND_PREFIX + the identifier verbatim (src/state.rs lines 595-616). Replaying the check, the status (404) and the error code (ResourceNotFoundException) are right and the message reads 'Network connector not found: nc-0000000000000001' - the bare Id as supplied - where the required message is 'Network connector not found: arn:aws:lambda:<region>:<account-id>:network-connector:nc-0000000000000001'. (The connector id, region and account above are per-run values filling the template; the divergence is that the Id is not expanded to the ARN form at all.)

Why: the refusal message is the only thing that identifies WHICH resource the service considers absent. A client or teardown assertion that compares the refusal against the Arn it was handed at create time (the ARN is the connector's canonical, account- and region-qualified name, and the only identifier form that proves the refusal is about the right account/Region) sees a mismatch on an otherwise correct refusal, so "delete, confirm gone, assert the dead identifier is refused" fails against a conforming emulator. Error-message parsers that pull the ARN out of the text to correlate with their own inventory get an unqualified id instead.

Fix: lambda-core's DeleteNetworkConnector handler not-found paths (both the removed-connector arm and the unknown-version arm) must build ResourceNotFoundException.Message through the same resource-naming rule GetNetworkConnector uses - i.e. the existing `connector_not_found_resource(region, identifier)` helper, which expands an Id to its ARN, keeps a version suffix, and leaves a NAME as supplied - instead of the verbatim-identifier `connector_not_found`. This is a refusal-message/serialization defect in the handler's error construction, not a state or lifecycle one: the 404 classification itself is now correct. The helper's doc comment, which claims the ARN form is exclusive to the reporting operation, needs correcting with it.

Relation to what is already filed: open issue 20261104T040654Z-lambda-core-deletenetworkconnector-answers-202-from-a-deletion.md carries the earlier, DIFFERENT failure of this same check (a 202 served from a deletion tombstone instead of any refusal at all) and its replay key is the `_error` assertion, not this message assertion - the emulator now reaches the refusal, and this is the next divergence on that path. The resolved wire issue 20261023T022015Z-lambda-core-rejection-messages-diverge-from-aws-not.md established the "Network connector not found: <identifier>" wording for name-addressed deletes; it does not cover the Id-to-ARN expansion asserted here. Neither issue carries this replay key.

Traces: lambda-core.network-connector-delete-transitions-to-removed.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 25/33 passed (75.8%), lambda-core 25/28 (89%), bootstrap_signal false. Of lambda-core's three failures, one is ALREADY FILED (network-connector-dualstack-network-protocol-persists, blocked by the ec2 CreateSubnet stub) and one is OWNED by 20261103T161149Z-lambda-core-a-created-network-connector-is-never.md; this check is the remaining one. Every other failing check in the report (cloudformation__ec2__lambda-core, cloudtrail__lambda-core, lambda-core__lambda-microvms) is marked OWNED.

[mismatch] lambda-core.network-connector-delete-transitions-to-removed
  File "check_lambda_core_network_connector_delete_transitions_to_removed.py", line 69, in run
    assert redelete_message == f"Network connector not found: {created['Arn']}", (
  AssertionError: DeleteNetworkConnector on the already-removed id: message 'Network connector not found: nc-0000000000000001'

The check reached this assertion having passed every earlier leg: the connector was created, observed ACTIVE, deleted, polled with GetNetworkConnector until that call answered ResourceNotFoundException ("REMOVED"), and the repeat DeleteNetworkConnector was then refused with Error.Code == 'ResourceNotFoundException' (that assertion, on line 65-67, held). Only the message assertion failed.

Expected (AWS): HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message == 'Network connector not found: ' + the connector's unqualified ARN, i.e. 'Network connector not found: arn:aws:lambda:<region>:<account-id>:network-connector:<Id>'. Fixed text: 'Network connector not found: '. Variable part: the ARN determined by the request's Identifier (region and account from the signed request, resource segment = the connector Id), in ARN form even though the request addressed the connector by bare Id.
Actual (emulator): HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message 'Network connector not found: nc-0000000000000001' - the identifier echoed verbatim, with no ARN expansion.

Code: services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/ops/delete_network_connector.rs lines 215-220 (removed-connector arm) and 257-259 (unknown-version arm) call crate::state::connector_not_found; services/.../src/state.rs lines 595-616 (verbatim formatter) vs lines 618-640 (connector_not_found_resource, the Id-to-ARN rule already implemented for GetNetworkConnector) and lines 575-586 (connector_arn).

## Steps to Reproduce
1. git_clone, then run_behavioral_suite() and look at check lambda-core.network-connector-delete-transitions-to-removed (it now fails on its message assertion, line 69, not on the earlier refusal assertion).
2. Or by hand against the booted emulator (region us-west-2):
   a. Create a VPC, a subnet in it and a security group, plus an IAM role trusting lambda.amazonaws.com with AWSLambdaNetworkConnectorOperatorPolicy attached.
   b. POST /2026-04-04/network-connectors with Name=<unique>, Configuration={"VpcEgressConfiguration":{"SubnetIds":[<subnetId>],"SecurityGroupIds":[<sgId>],"NetworkProtocol":"IPv4","AssociatedComputeResourceTypes":["MicroVm"]}}, OperatorRole=<roleArn>. Note the returned Id and Arn.
   c. Poll GET /2026-04-04/network-connectors/<Id> until State is ACTIVE.
3. DELETE /2026-04-04/network-connectors/<Id> (202, State=DELETING), then poll GET /2026-04-04/network-connectors/<Id> until it answers 404 ResourceNotFoundException - the removal has completed.
4. DELETE /2026-04-04/network-connectors/<Id> again. Expect HTTP 404 / ResourceNotFoundException with Message 'Network connector not found: arn:aws:lambda:<region>:<account-id>:network-connector:<Id>'. Observe Message 'Network connector not found: <Id>' (the bare id as supplied).
5. Contrast the two cases that must NOT change: DELETE /2026-04-04/network-connectors/<a name that never existed> must keep refusing with the name echoed verbatim ('Network connector not found: <name>'), and GET on the removed <Id> already returns the ARN form - so the delete path must adopt the same resolution rule, not a new one.
