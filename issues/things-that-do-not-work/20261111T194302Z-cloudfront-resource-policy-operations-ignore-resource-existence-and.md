Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-resource-policy-missing-resource-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-resource-policy-missing-resource-error
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|PutResourcePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|PutResourcePolicy|4xx->2xx|status
Issue-Key: cloudfront|GetResourcePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|GetResourcePolicy|4xx->2xx|status
Issue-Key: cloudfront|DeleteResourcePolicy|4xx->2xx|$
Issue-Key: cloudfront|DeleteResourcePolicy|4xx->2xx|status
Issue-Commit: 14c2f807276b097453c7eebbf32725a3995b3836

# CloudFront resource-policy operations ignore resource existence and answer success instead of EntityNotFound

> _Note: this issue was identified against the code base at commit `14c2f807276b097453c7eebbf32725a3995b3836`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
Three net-new wire divergence clusters (3 causes) share one fault: the CloudFront resource-policy operations never check that the resource named by ResourceArn exists. Put/Get/DeleteResourcePolicy all answer their 200 success shape for a resource that is absent from state, so the modeled EntityNotFound (404) refusal is unreachable on all three operations. Each of the three clusters has an empty owned_by and an empty stub_path.

This is NOT the fault carried by the open issue 20261111T001225Z-cloudfront-resource-policy-operations-skip-resourcearn-argument-validation.md: that issue is about a *malformed or unsupported* ResourceArn being answered 404 EntityNotFound where AWS refuses 400 InvalidArgument (argument validation ordering). The fault here is the opposite end of the same path — a well-formed ResourceArn that state does not hold is answered 200 success where AWS refuses 404 EntityNotFound — so fixing that issue's validation step does not make the existence check appear.

Cause 1 of 3: PutResourcePolicy on a resource that does not exist returns 200 instead of 404 EntityNotFound.
AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json): PutResourcePolicy input members ResourceArn (@required) and PolicyDocument (@required); http {POST /2020-05-31/put-resource-policy, code 200}; modeled errors AccessDenied (403), EntityNotFound (404), IllegalUpdate (400), InvalidArgument (400), PreconditionFailed (412), UnsupportedOperation (400). Shape EntityNotFound is an @error client shape with httpError 404 and doc "The entity was not found." Attaching a policy to an ARN whose resource state does not hold is refused: HTTP 404 with an XML ErrorResponse whose Error.Type is 'Sender', Error.Code is 'EntityNotFound' and Error.Message is "The specified resource doesn't exist. Update your request and try again." That message is fixed text end to end — it carries no value from the request or from state; the envelope's RequestId is a per-run value and is not a requirement.
Emulator: answers HTTP 200 with the PutResourcePolicy success body and no ErrorResponse element at all ($.ErrorResponse missing).
Why: a client that attaches a policy to a resource it believes exists is told the write succeeded when nothing was stored and no resource was named. The 404 EntityNotFound signal is exactly what a caller uses to detect a stale or wrong ARN and to branch into provisioning the resource; being handed a 200 instead, the caller records the policy as applied, skips the create/repair branch, and later reads back no policy — a silent write loss rather than a reported error.
Fix: PutResourcePolicy's handler must resolve ResourceArn against state after argument validation and raise EntityNotFound (404) with AWS's wording when state holds no such resource; only an existing resource may reach the state-write path.
Traces: cloudfront-resource-policy-missing-resource-error#task#1.

Cause 2 of 3: GetResourcePolicy on a resource that does not exist returns 200 instead of 404 EntityNotFound.
AWS: Smithy: GetResourcePolicy input member ResourceArn (@required); output members ResourceArn, PolicyDocument; http {POST /2020-05-31/get-resource-policy, code 200}; modeled errors AccessDenied (403), EntityNotFound (404), InvalidArgument (400), UnsupportedOperation (400). A read whose ResourceArn names no resource is refused HTTP 404 with Error.Type 'Sender', Error.Code 'EntityNotFound' and the same fixed Message "The specified resource doesn't exist. Update your request and try again."
Emulator: answers HTTP 200 and omits the ErrorResponse body entirely ($.ErrorResponse missing).
Why: the read becomes indistinguishable from a resource that exists with no policy, so a caller cannot tell "resource gone" from "policy unset". A get-then-put control loop that keys on EntityNotFound to create the resource never fires, and SDK error-type dispatch (catch EntityNotFound) is dead code against the emulator.
Fix: GetResourcePolicy's handler — look the ResourceArn up in state and raise EntityNotFound (404) with AWS's wording when it is absent, instead of returning the success shape unconditionally.
Traces: cloudfront-resource-policy-missing-resource-error#task#2.

Cause 3 of 3: DeleteResourcePolicy on a resource that does not exist returns 200 with an empty body instead of 404 EntityNotFound.
AWS: Smithy: DeleteResourcePolicy input member ResourceArn (@required), output Unit; http {POST /2020-05-31/delete-resource-policy, code 200}; modeled errors AccessDenied (403), EntityNotFound (404), IllegalDelete (400), InvalidArgument (400), PreconditionFailed (412), UnsupportedOperation (400). Deleting a policy from a resource that does not exist is refused HTTP 404 with an ErrorResponse carrying Error.Type 'Sender', Error.Code 'EntityNotFound' and Message "The specified resource doesn't exist. Update your request and try again." — i.e. the delete is NOT idempotent over a missing resource.
Emulator: answers HTTP 200 with a completely empty body (replay reports $: empty_body, expected the ErrorResponse envelope, got {}).
Why: the delete silently succeeds, so a teardown step cannot detect that it operated on the wrong ARN, and a caller using EntityNotFound as the "already gone" terminator sees a success it must not see — any audit that asserts a policy existed before deletion passes vacuously.
Fix: DeleteResourcePolicy's handler — resolve ResourceArn against state before mutating and raise EntityNotFound (404) with AWS's wording when absent. All three operations should share one existence check so the three refusals stay consistent.
Traces: cloudfront-resource-policy-missing-resource-error#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 1303 steps total, 1011 passed, 287 mismatched, 5 blocked, score 77.6%, 90 divergence clusters. All three clusters below report owned_by [] and stub_path "".

Cause 1 — cloudfront-resource-policy-missing-resource-error#task#1, PutResourcePolicy, expected status 404 / actual 200:
  status: expected 404, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'EntityNotFound', 'Message': "The specified resource doesn't exist. Update your request and try again."}, 'RequestId': '<per-run value, not asserted as a requirement>'}, got None)
  divergence_key: service=cloudfront|PutResourcePolicy|status=404->200|keys=$.ErrorResponse,status (affected_count 1)

Cause 2 — cloudfront-resource-policy-missing-resource-error#task#2, GetResourcePolicy, expected status 404 / actual 200:
  status: expected 404, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'EntityNotFound', 'Message': "The specified resource doesn't exist. Update your request and try again."}, 'RequestId': '<per-run value>'}, got None)
  divergence_key: service=cloudfront|GetResourcePolicy|status=404->200|keys=$.ErrorResponse,status (affected_count 1)

Cause 3 — cloudfront-resource-policy-missing-resource-error#task#3, DeleteResourcePolicy, expected status 404 / actual 200:
  status: expected 404, got 200
  $: empty_body (expected {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'EntityNotFound', 'Message': "The specified resource doesn't exist. Update your request and try again."}, 'RequestId': '<per-run value>'}}, got {})
  divergence_key: service=cloudfront|DeleteResourcePolicy|status=404->200|keys=$,status (affected_count 1)

Fixed vs variable parts of the asserted message: "The specified resource doesn't exist. Update your request and try again." is fixed text in full for all three operations — no name, id, limit or member name from the request or from state appears in it. The only per-run values in the recorded envelope are the RequestId values, which are NOT part of the requirement. Model grounding: smithy_query query=operation operation=PutResourcePolicy,GetResourcePolicy,DeleteResourcePolicy and query=shape shape=EntityNotFound on the cloudfront 2020-05-31 model — EntityNotFound (httpError 404) is modeled on all three operations.

## Steps to Reproduce
1. git_clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture cloudfront-resource-policy-missing-resource-error, step task#1 — POST /2020-05-31/put-resource-policy with a well-formed ResourceArn for a VPC origin that was never created, plus a PolicyDocument. The emulator answers 200 with the success shape; the recording is 404 with ErrorResponse Error.Type Sender / Error.Code EntityNotFound / Message "The specified resource doesn't exist. Update your request and try again."
4. Cause 2: same fixture, step task#2 — POST /2020-05-31/get-resource-policy for the same absent resource. The emulator answers 200; the recording is the same 404 EntityNotFound envelope.
5. Cause 3: same fixture, step task#3 — POST /2020-05-31/delete-resource-policy for the same absent resource. The emulator answers 200 with an empty body; the recording is the same 404 EntityNotFound envelope.
6. Regression guard: create a VPC origin, put a policy on it, and confirm Put/Get/DeleteResourcePolicy still answer 200 for the resource that does exist.
7. All three causes must match the recordings before closing.
