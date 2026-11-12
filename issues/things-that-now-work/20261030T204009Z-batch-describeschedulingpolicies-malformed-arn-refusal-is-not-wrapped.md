Issue-Source: trace-sweep-divergence
Issue-Trace: batch-scheduling-policy-standalone-lifecycle
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: batch-scheduling-policy-standalone-lifecycle
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch|DescribeSchedulingPolicies|4xx->4xx|$.message
Issue-Commit: f86146c901ffe616e4f6671efe850ee37d774551

# batch DescribeSchedulingPolicies malformed-ARN refusal is not wrapped in Batch's "Error executing request, Exception : ..., RequestId:" envelope

> _Note: this issue was identified against the code base at commit `f86146c901ffe616e4f6671efe850ee37d774551`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSchedulingPolicies

## Description
One net-new root cause on the AWS Batch scheduling-policy read plane. This is the only divergence cluster in the wire replay whose owned_by was EMPTY; every other batch cluster in the report is owned by an open issue that already carries its replay key (20261029T234420Z-batch-tag-mutations-answer-http-200-instead-of.md, 20261029T234420Z-batch-refusal-contracts-diverge-not-found-arn-wording.md) and is not re-filed here.

Cause 1 of 1: DescribeSchedulingPolicies' malformed-scheduling-policy-ARN refusal is rendered as a bare validator sentence instead of Batch's input-validation error envelope.

AWS: Smithy (services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json): DescribeSchedulingPolicies, POST /v1/describeschedulingpolicies, input arns -> StringList (required, clientOptional), output schedulingPolicies -> SchedulingPolicyDetailList, errors exactly ClientException (httpError 400) and ServerException (500). The model states no message text, so the wording is recorded behavior only. The recording refuses an entry of arns that is not a well-formed scheduling-policy ARN with HTTP 400 ClientException whose message is NOT asserted literally — it is asserted by SHAPE (the replay reports expected '<shaped-not-asserted>') precisely because the message ends in a per-call request id. The shape is this service's input-validation envelope, the same one its sibling validators use on this very operation: fixed text "Error executing request, Exception : ", then the validator's own sentence, then the fixed text ", RequestId: ", then this call's request id. (Evidence that this envelope is Batch's input-validation contract and not an invention: the sibling arns-count refusal on this same operation is recorded as "Error executing request, Exception : Number of scheduling policies should be between 1 and 100., RequestId: <id>" — see the open behavioral issue 20261029T173319Z-batch-8-net-new-root-causes-relationship-delete.md, cause 3.) The fixed parts are the two envelope fragments; the variable parts are the inner sentence's interpolated value (the rejected arns entry, taken from the request) and the request id (taken from the call).

Emulator: keeps the status (400) and the error class (ClientException) but answers the bare sentence 'Scheduling policy arn should match a valid Scheduling policy arn pattern: not-a-valid-arn' — no "Error executing request, Exception : " prefix, no ", RequestId: <id>" suffix at all. The divergence is therefore the ABSENCE of the envelope's fixed text, not a difference in the per-run request-id value: a message carrying the envelope with any request id would satisfy the recording's shape assertion, and this one carries no envelope.

Why: a client keyed on Batch's input-validation envelope (matching the "Error executing request, Exception : " prefix to classify a refusal as request validation, or parsing the trailing RequestId to correlate the refusal with a support case or a log line) sees neither, so the refusal falls through to a generic error path and the call becomes uncorrelatable. It also makes this operation's two validators mutually inconsistent in the emulator: a client cannot handle malformed-ARN and bad-arity refusals from the same operation with one matcher the way AWS's uniform envelope allows.

Fix: the input validation of the DescribeSchedulingPolicies handler in the batch crate — its arn-pattern check must raise the refusal through the same ClientException renderer that wraps the operation's other validation refusals (envelope plus the call's request id) rather than returning the validator's own sentence directly. The related open issue 20261029T173319Z-batch-8-net-new-root-causes-relationship-delete.md describes the same missing envelope for three OTHER validators (DescribeSchedulingPolicies arns count, ListQuotaShares maxResults, UpdateSchedulingPolicy computeReservation) but is a behavioral-corpus issue carrying no wire replay key, and it does not cover the arn-pattern branch, so this wire step is not gated by it.

Traces: batch-scheduling-policy-standalone-lifecycle#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true (/__admin/health 200, data listener accepting), 14 fixtures replayed, 98 steps, 86 passed, 12 mismatched, score 87.8%, bootstrap_signal false, 7 divergence clusters.

This cluster: divergence_key "service=batch|DescribeSchedulingPolicies|status=400->400|keys=$.message", fixture_id batch-scheduling-policy-standalone-lifecycle, step_key task#9, verdict mismatch, affected_count 1, stub_path empty, owned_by EMPTY (so no open issue carries this replay key).

expected_status 400, actual_status 400 — status and error class agree.
detail: "$.message: value_mismatch (expected '<shaped-not-asserted>', got 'Scheduling policy arn should match a valid Scheduling policy arn pattern: not-a-valid-arn')".

The expected side is a shape assertion rather than a literal because the recorded message terminates in a per-call request id; the actual message fails that assertion because it contains none of the envelope's fixed text ("Error executing request, Exception : " ... ", RequestId: "). The string 'not-a-valid-arn' in the actual message is the request's own arns entry filling the inner sentence's variable slot — it is not part of the requirement; the requirement is the enclosing envelope.

Smithy evidence: DescribeSchedulingPolicies declares exactly ClientException (httpError 400) / ServerException (500) and no message text, so the envelope is recorded behavior the emulator must reproduce without model support. Corroborating recording of the same envelope on the sibling validator of this operation: "Error executing request, Exception : Number of scheduling policies should be between 1 and 100., RequestId: <id>".

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator built from this checkout.
2. Fixture batch-scheduling-policy-standalone-lifecycle, step task#9: POST /v1/describeschedulingpolicies with arns containing one entry that is not a well-formed scheduling-policy ARN (the recorded probe uses the literal "not-a-valid-arn").
3. Expect HTTP 400 ClientException whose message is Batch's input-validation envelope: the fixed text "Error executing request, Exception : ", the arn-pattern sentence, the fixed text ", RequestId: " and this call's request id.
4. Observed HTTP 400 ClientException with message "Scheduling policy arn should match a valid Scheduling policy arn pattern: not-a-valid-arn" — the bare validator sentence, with neither envelope fragment and no request id.
5. Cross-check with the same operation's arity refusal (arns with 101 entries): it is recorded with the identical envelope, so both validators on this operation must render through it.
