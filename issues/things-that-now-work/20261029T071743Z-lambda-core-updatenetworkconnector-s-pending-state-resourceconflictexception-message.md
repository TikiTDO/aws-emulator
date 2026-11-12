Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-core.pending-connector-rejects-update-and-delete
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core.pending-connector-rejects-update-and-delete|run#10|AssertionError|assert message == (
Issue-Commit: 87ef04f300fd59d35c2e48965ae7f4a968416820

# lambda-core: UpdateNetworkConnector's PENDING-state ResourceConflictException message uses the wrong rule (names the connector by Name, wrong sentence)

> _Note: this issue was identified against the code base at commit `87ef04f300fd59d35c2e48965ae7f4a968416820`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateNetworkConnector

## Description
One net-new root cause, grounded in the one surviving behavioral failure for lambda-core (every other lambda-core/cloudtrail/lambda-microvms failure in this run was marked ALREADY FILED on an EC2/lambda-microvms scaffold stub, or OWNED by 20261024T113459Z-lambda-core-network-connectors-clienttoken-reuse-never-refused.md). NOTE for the fixer: the open issue 20261024T113459Z-lambda-core-network-connectors-clienttoken-reuse-never-refused.md describes the same state-conflict wording divergence as its Cause 6, but the replay suite does NOT report this check as owned by it (its recorded replay key for this check no longer matches the failure), so the check is still failing and unowned and is filed here. If that issue's Cause 6 is fixed, this one closes with it.

Cause 1 of 1: the ResourceConflictException raised when UpdateNetworkConnector is called on a connector that is not ACTIVE is phrased by the emulator's own rule instead of the service's, and identifies the connector by its Name rather than by its service-assigned Id.

AWS: Smithy (com.amazonaws.lambdacore) UpdateNetworkConnector models ResourceConflictException at httpError 409, documented "The request could not be completed due to a conflict with the current state of the resource. For example, attempting to update a connector that is not in ACTIVE state"; the operation doc states "The connector must be in ACTIVE state to accept updates", and Identifier (NetworkConnectorIdentifier, len 1..140, @required) accepts "the connector ID, name, or full ARN". The model does not fix the message text, so this is recorded behavior: the service states the refusal as the two-sentence form "Network connector <Id> is in state Pending and cannot be updated. Only ACTIVE connectors can be updated." Fixed text: "Network connector ", " is in state ", " and cannot be updated. Only ACTIVE connectors can be updated." Variable parts: <Id> is the connector's service-assigned Id (the Id member the Create/Update/Get response returns, NOT the Name and NOT whatever identifier form the caller passed in Identifier), and the state token is the connector's current state rendered capitalised ('Pending' for the PENDING state). The behavioral check derives the expected sentence from the Id returned by CreateNetworkConnector, which is what pins the identifier to the Id.

Emulator: the status (409) and the error code (ResourceConflictException) are right, but services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/ops/update_network_connector.rs formats a different single sentence from the connector's Name and its raw uppercase state: "The network connector {name} is in {state} state; it must be ACTIVE to accept updates". Replay observed, verbatim: 'The network connector sim2real-behav-lc-pd-32f36c4affa2 is in PENDING state; it must be ACTIVE to accept updates' — the Name, not the Id, and none of AWS's fixed text. The sibling refusal on the delete path is built the same way (delete_network_connector.rs formats "The network connector {name} is in {state} state; it can be deleted once it is ACTIVE" where the service states the fixed sentence "Network connector is in PENDING state and cannot be deleted"); the check never reaches that assertion because the update assertion fails first, so fixing only the update path will leave the same check failing on the delete sentence.

Why: a client or operator holding the connector Id it got back from CreateNetworkConnector cannot correlate the 409 with the resource — the message names a different identity field, and on this emulator Name and Id already disagree. Clients and tests that branch on the refusal message (distinguishing a not-yet-ACTIVE conflict, which is retryable after the connector settles, from the other conflicts ResourceConflictException covers) match nothing and treat a retryable state gate as an unknown conflict.

Fix: the state gate in UpdateNetworkConnector's handler (the non-ACTIVE branch that calls crate::state::connector_conflict) must emit the service's sentence and identify the connector by connector.id, with the state rendered as AWS renders it; apply the same correction to the matching state gate in DeleteNetworkConnector's handler, since the same check asserts that sentence next.

Traces: lambda-core.pending-connector-rejects-update-and-delete (AssertionError at check_lambda_core_pending_connector_rejects_update_and_delete.py:222, "unexpected refusal message for a PENDING update"). Code citations: services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/ops/update_network_connector.rs:262-272 (the conflict branch and its format! string), and crate::state::connector_conflict in services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/state.rs.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1: 5/24 passed; lambda-core 5/21 (24%), 16 failed, of which all but one are ALREADY FILED (EC2 CreateInternetGateway / AssociateVpcCidrBlock scaffold stubs) or OWNED by 20261024T113459Z-lambda-core-network-connectors-clienttoken-reuse-never-refused.md.

[mismatch] lambda-core.pending-connector-rejects-update-and-delete
CreateNetworkConnector returns State=PENDING (asserted and passing). UpdateNetworkConnector(Identifier=<Id from create>, Configuration=VpcEgressConfiguration over both subnets) is then refused.
  HTTP status: 409 — matches AWS.
  Error.Code: ResourceConflictException — matches AWS.
  Error.Message expected (AWS): "Network connector <Id> is in state Pending and cannot be updated. Only ACTIVE connectors can be updated." where <Id> is the Id the create response returned (e.g. an 'nc-...' id) and 'Pending' is the capitalised current state; the rest is fixed text.
  Error.Message actual (emulator): 'The network connector sim2real-behav-lc-pd-32f36c4affa2 is in PENDING state; it must be ACTIVE to accept updates' — the identifier shown is the connector Name generated by the check for that run (a per-run value, not part of the requirement), the state token is the raw uppercase 'PENDING', and the fixed text does not match AWS's at any point.
  Harness: AssertionError: unexpected refusal message for a PENDING update: 'The network connector sim2real-behav-lc-pd-32f36c4affa2 is in PENDING state; it must be ACTIVE to accept updates' (check_lambda_core_pending_connector_rejects_update_and_delete.py:222).

Not reached because of the above, but built by the same rule and asserted by the same check: the PENDING DeleteNetworkConnector refusal, where AWS states the fixed sentence "Network connector is in PENDING state and cannot be deleted" (no variable parts) and delete_network_connector.rs formats "The network connector <Name> is in <STATE> state; it can be deleted once it is ACTIVE".

## Steps to Reproduce
Against a booted emulator in us-west-2 (or re-replay the check id in trace_ids via run_behavioral_suite):
1. Create an EC2 VPC with two subnets in two available AZs and a security group in that VPC, and an IAM role whose trust policy names lambda.amazonaws.com with AWSLambdaVPCAccessExecutionRole attached.
2. POST /2026-04-04/network-connectors with Name=<label>, Configuration.VpcEgressConfiguration={SubnetIds:[subnet1], SecurityGroupIds:[sg], NetworkProtocol:'IPv4', AssociatedComputeResourceTypes:['MicroVm']}, OperatorRole=<role arn>. Record the Id from the response and confirm State == 'PENDING'.
3. Immediately (while State is still PENDING) PUT /2026-04-04/network-connectors/<Id> with Configuration.VpcEgressConfiguration.SubnetIds=[subnet1, subnet2].
   Expect: 409, Error.Code ResourceConflictException, Error.Message "Network connector <Id> is in state Pending and cannot be updated. Only ACTIVE connectors can be updated."
   Observe: 409, ResourceConflictException, Error.Message "The network connector <Name> is in PENDING state; it must be ACTIVE to accept updates".
4. GetNetworkConnector(<Id>) to confirm the connector is still PENDING (so the refusal above was genuinely against a PENDING connector) and that the refused update was not applied.
5. Then DELETE /2026-04-04/network-connectors/<Id> while still PENDING.
   Expect: 409 ResourceConflictException with the fixed message "Network connector is in PENDING state and cannot be deleted".
   Observe: 409 ResourceConflictException with "The network connector <Name> is in PENDING state; it can be deleted once it is ACTIVE".
