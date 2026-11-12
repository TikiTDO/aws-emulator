Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.parameter-group-known-parameter-value-validation redshift.snapshot-duplicate-and-ungranted-revoke-fail
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.parameter-group-known-parameter-value-validation|run#18|AssertionError|assert message == "Invalid JSON for wlm_json_configuration", message
Issue-Key: redshift.snapshot-duplicate-and-ungranted-revoke-fail|run#22|AssertionError|assert revoke_message == (
Issue-Commit: afa022acdc4a1642d61c8c481d867f57e8f43f54

# Redshift: 2 net-new refusal-message causes — ModifyClusterParameterGroup's malformed-wlm_json_configuration InvalidParameterValue uses the emulator's wire-location template, and RevokeSnapshotAccess's AccessToSnapshotDenied uses the model's documentation sentence

> _Note: this issue was identified against the code base at commit `afa022acdc4a1642d61c8c481d867f57e8f43f54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyClusterParameterGroup

## Description
Two net-new root causes in the redshift (awsQuery) control plane. Both are refusal-MESSAGE divergences: the HTTP status and the error code already match AWS, so the divergence is invisible to code that branches only on Error.Code and shows up only in the text clients surface, snapshot-test or pattern-match.

Cause 1 of 2: ModifyClusterParameterGroup given a malformed JSON value for the known parameter wlm_json_configuration is refused with the right code but an emulator-authored message built out of the query-serialization member path.
Failing check: redshift.parameter-group-known-parameter-value-validation
AWS: ModifyClusterParameterGroup on a redshift-1.0 parameter group, with Parameters=[{ParameterName:'wlm_json_configuration', ParameterValue:'{not-valid-json'}], is refused with HTTP 400 and the service-common awsQuery code InvalidParameterValue, message `Invalid JSON for wlm_json_configuration` — the whole string is FIXED text for this error class; the only variable part is the parameter name (`wlm_json_configuration`), taken from the request's ParameterName. The message does NOT name the member's position in the query serialization, does not quote the rejected value and does not describe the expected document shape. Smithy (com.amazonaws.redshift#RedshiftServiceVersion20121201, ModifyClusterParameterGroup) models only ClusterParameterGroupNotFoundFault (awsQueryError ClusterParameterGroupNotFound, httpError 404) and InvalidClusterParameterGroupStateFault (awsQueryError InvalidClusterParameterGroupState, httpError 400), so the parameter-value check is the service-common InvalidParameterValue and this pairing is recorded behavior only; the recording shows code InvalidParameterValue with exactly the text above.
Emulator: code InvalidParameterValue (correct) with the message `Invalid value for Parameters.Parameter.<n>.ParameterValue: the value of '<ParameterName>' must be a JSON list of workload management queues.` — different fixed text, prefixed with the wire member path `Parameters.Parameter.<n>.ParameterValue` (`<n>` is the member's index in the submitted Parameters list, a serialization detail absent from AWS's message) and ending in a prose explanation AWS does not emit.
Why: the message is part of the refusal contract. A client that matches or displays the text — CLI transcript tests, error-mapping layers that separate a JSON-parse failure from an allowed-value or range failure, log scrapers, IaC drift handlers — diverges from AWS, and the emulator additionally leaks the query-wire index of the member instead of speaking in the operator's vocabulary.
Fix: the redshift ModifyClusterParameterGroup parameter-value validation and the serializer that renders its InvalidParameterValue — the wlm_json_configuration JSON check must render AWS's per-class template (`Invalid JSON for ` + parameter name) rather than the generic "Invalid value for <wire member path>: ..." formatter. This is the SAME generic formatter the open issue 20261106T225316Z-redshift-modifyclusterparametergroup-boolean-value-refusal-invalidparametervalue-message-is-the.md describes for the boolean path (that path's wording now passes in this check, and the suite did not mark this failure owned, so the JSON path is filed here and that issue is named for context); the open issue 20261103T040731Z-redshift-parameter-group-validation-and-the-parameter-catalog.md covers other parameter-group causes and likewise does not carry this check's replay key. Because the check stops at this assertion, its later legs — the integer-type message, the whole-batch atomicity of a mixed valid/invalid modify, the out-of-range statement_timeout message, the applied-value/Source round-trip and ResetClusterParameterGroup — were never reached and are unverified rather than passing.
Traces: redshift.parameter-group-known-parameter-value-validation.

Cause 2 of 2: RevokeSnapshotAccess for an account that holds no restore grant returns AccessToSnapshotDenied with the Smithy documentation sentence of the fault instead of AWS's refusal message.
Failing check: redshift.snapshot-duplicate-and-ungranted-revoke-fail
AWS: RevokeSnapshotAccess(SnapshotIdentifier=<existing manual snapshot>, AccountWithRestoreAccess=<account that was never authorized>) is refused with HTTP 400 and awsQueryError code AccessToSnapshotDenied (Smithy: RevokeSnapshotAccess errors AccessToSnapshotDeniedFault @error("client"), httpError 400, awsQueryError AccessToSnapshotDenied). The message is `The account <AccountWithRestoreAccess> is not authorized to access the snapshot <SnapshotIdentifier>` — fixed text `The account ` + ` is not authorized to access the snapshot `, no trailing period, with exactly two variable parts, both taken from the request: the account id and the snapshot identifier, in that order.
Emulator: the same code (AccessToSnapshotDenied) but the message `The owner of the specified snapshot <SnapshotIdentifier> has not authorized your account <AccountWithRestoreAccess> to access the snapshot.` — this is the fault shape's Smithy documentation prose ("The owner of the specified snapshot has not authorized your account to access the snapshot.") with the identifiers interpolated, i.e. different fixed text, the two variable parts in the opposite order, and a trailing period.
Why: a client that matches AWS's wording to distinguish "this account was never granted" from the not-found and authorization-not-found refusals on the same operation sees an unrecognized sentence; documentation prose is not a wire message, so every consumer of the refusal text diverges.
Fix: the RevokeSnapshotAccess handler's grant-lookup refusal path and the error serializer that renders AccessToSnapshotDeniedFault — render AWS's template from the request's AccountWithRestoreAccess and SnapshotIdentifier instead of the shape's documentation string. The open issue 20261029T192501Z-redshift-snapshot-refusal-messages-and-batch-snapshot-error.md covers RevokeSnapshotAccess's ClusterSnapshotNotFound wording, a different refusal class on the same operation, and does not carry this check's replay key. The check's remaining legs after this assertion (that the denied revoke leaves the granted account's grant intact and that revoking the granted account then succeeds) were not reached.
Traces: redshift.snapshot-duplicate-and-ungranted-revoke-fail.

## Evidence
Replay of simparity-behavioral-aws-v1; both checks verdict=mismatch, service heading `### redshift`.

Cause 1 — redshift.parameter-group-known-parameter-value-validation.
Setup (passed): CreateClusterParameterGroup(ParameterGroupFamily='redshift-1.0'); DescribeClusterParameters exposes require_ssl (boolean, Source='engine-default'), wlm_json_configuration and statement_timeout (integer); ModifyClusterParameterGroup(Parameters=[{require_ssl:'maybe'}]) is already refused with InvalidParameterValue and the expected message 'parameter value is not a valid boolean:maybe'.
Failing call: ModifyClusterParameterGroup(ParameterGroupName=<group>, Parameters=[{ParameterName:'wlm_json_configuration', ParameterValue:'{not-valid-json'}]).
Error.Code: 'InvalidParameterValue' — matches AWS.
Error.Message expected: 'Invalid JSON for wlm_json_configuration'
Error.Message actual:   "Invalid value for Parameters.Parameter.1.ParameterValue: the value of 'wlm_json_configuration' must be a JSON list of workload management queues."
Replay assertion: AssertionError at check_redshift_parameter_group_known_parameter_value_validation.py line 147 — `assert message == "Invalid JSON for wlm_json_configuration"`.
Fixed text of the requirement: 'Invalid JSON for '. Variable part: the parameter name from the request. The '1' in the emulator's text is the member index in the submitted Parameters list and the trailing sentence is emulator prose — neither appears in AWS's message.

Cause 2 — redshift.snapshot-duplicate-and-ungranted-revoke-fail.
Setup (passed): CreateCluster(single-node, Encrypted=true) -> available; CreateClusterSnapshot(<snap>) -> available; the duplicate CreateClusterSnapshot(<same snap id>) is already refused with ClusterSnapshotAlreadyExists and the expected message 'Cannot create the snapshot because a snapshot with the identifier <snap> already exists.' and does not replace SnapshotCreateTime; AuthorizeSnapshotAccess(AccountWithRestoreAccess='555566667777') succeeds and is visible in AccountsWithRestoreAccess.
Failing call: RevokeSnapshotAccess(SnapshotIdentifier=<snap>, AccountWithRestoreAccess='444455556666') — an account that holds no grant.
Error.Code: 'AccessToSnapshotDenied' — matches AWS.
Error.Message expected: 'The account 444455556666 is not authorized to access the snapshot <snap>'
Error.Message actual:   'The owner of the specified snapshot <snap> has not authorized your account 444455556666 to access the snapshot.'
Replay assertion: AssertionError at check_redshift_snapshot_duplicate_and_ungranted_revoke_fail.py line 347 (the message comparison following the already-satisfied `assert revoke_code == "AccessToSnapshotDenied"`).
Fixed text of the requirement: 'The account ' + ' is not authorized to access the snapshot '. Variable parts: the request's AccountWithRestoreAccess and SnapshotIdentifier, in that order. The account id and snapshot id shown come from the run (the snapshot id is generated per run); the wording, the argument order and the absence of a trailing period are the requirement.

## Steps to Reproduce
Cause 1 (redshift.parameter-group-known-parameter-value-validation):
1. redshift CreateClusterParameterGroup(ParameterGroupName=<new>, ParameterGroupFamily='redshift-1.0', Description=<any>).
2. ModifyClusterParameterGroup(ParameterGroupName=<same>, Parameters=[{ParameterName:'wlm_json_configuration', ParameterValue:'{not-valid-json'}]).
3. Read the refusal: expect HTTP 400, Error.Code 'InvalidParameterValue', Error.Message exactly 'Invalid JSON for wlm_json_configuration'.
4. Re-read DescribeClusterParameters(ParameterGroupName=<same>): wlm_json_configuration must still hold its engine default with Source='engine-default'.
Replay: redshift.parameter-group-known-parameter-value-validation.

Cause 2 (redshift.snapshot-duplicate-and-ungranted-revoke-fail):
1. redshift CreateCluster(single-node, any orderable NodeType) and wait for 'available'.
2. CreateClusterSnapshot(SnapshotIdentifier=<snap>, ClusterIdentifier=<cluster>) and wait for Status 'available'.
3. AuthorizeSnapshotAccess(SnapshotIdentifier=<snap>, AccountWithRestoreAccess='555566667777') and confirm it appears in AccountsWithRestoreAccess.
4. RevokeSnapshotAccess(SnapshotIdentifier=<snap>, AccountWithRestoreAccess='444455556666') — an account with no grant.
5. Read the refusal: expect HTTP 400, Error.Code 'AccessToSnapshotDenied', Error.Message exactly 'The account 444455556666 is not authorized to access the snapshot <snap>'.
6. Re-read DescribeClusterSnapshots(SnapshotIdentifier=<snap>): '444455556666' must be absent and '555566667777' still present; then RevokeSnapshotAccess for '555566667777' must succeed.
Replay: redshift.snapshot-duplicate-and-ungranted-revoke-fail.
