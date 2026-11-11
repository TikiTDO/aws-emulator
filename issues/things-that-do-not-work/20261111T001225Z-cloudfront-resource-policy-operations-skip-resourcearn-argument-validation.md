Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-resource-policy-not-found-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-resource-policy-not-found-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|GetResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetResourcePolicy|4xx->4xx|status
Issue-Key: cloudfront|PutResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|PutResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|PutResourcePolicy|4xx->4xx|status
Issue-Commit: b26d594b442f382e51363fa18dcb67bd5cc86e1e

# CloudFront resource-policy operations skip ResourceArn argument validation and answer 404 EntityNotFound where AWS refuses with 400 InvalidArgument

> _Note: this issue was identified against the code base at commit `b26d594b442f382e51363fa18dcb67bd5cc86e1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
Two net-new wire divergence clusters (2 causes) share one fault: GetResourcePolicy and PutResourcePolicy resolve the ResourceArn against stored state BEFORE validating the argument itself, so every bad ResourceArn — a missing one, and one naming an unsupported resource type — collapses into a single 404 EntityNotFound instead of the operation's modeled 400 InvalidArgument refusal with its own wording. No open issue carries a cloudfront resource-policy replay key (the existing resource-policy issues on file are for other services: dynamodb, kinesis, redshift, lambda, ssm, signin, etc.).

Cause 1 of 2: GetResourcePolicy with no usable ResourceArn is answered 404 EntityNotFound instead of 400 InvalidArgument.
AWS: Smithy: GetResourcePolicy input member ResourceArn is @required (string); http {POST /2020-05-31/get-resource-policy, code 200}; modeled errors AccessDenied (403), EntityNotFound (404), InvalidArgument (400), UnsupportedOperation (400). A request that does not supply a usable ResourceArn fails the argument check, so AWS refuses with HTTP 400 and error Code 'InvalidArgument'. Message: "The ResourceArn parameter is required. Specify a valid ARN and try again." — this is entirely fixed text; the member name ResourceArn is the input member from the model, and the message has no values taken from the request or from state.
Emulator: answers HTTP 404 with $.ErrorResponse.Error.Code 'EntityNotFound' and Message "The specified resource doesn't exist. Update your request and try again."
Why: the two error classes mean opposite things to a client. 400 InvalidArgument is a non-retryable caller bug in the request itself; 404 EntityNotFound says the request was well-formed and the named resource is simply absent, which is the signal a client uses to branch into "create it" or to treat the policy as not yet set. Being told a malformed request is a missing resource sends a caller down the create path for an ARN it never actually named, and SDK error-type dispatch (catch InvalidArgument vs EntityNotFound) takes the wrong branch, so the caller never learns that the required member was missing.
Fix: GetResourcePolicy's input validation must run before resource lookup in its handler — validate/parse ResourceArn and raise InvalidArgument (400) with AWS's wording, and reach the EntityNotFound (404) path only for a well-formed ARN that state does not hold.
Traces: cloudfront-resource-policy-not-found-errors#task#1.

Cause 2 of 2: PutResourcePolicy with a ResourceArn naming an unsupported resource type is answered 404 EntityNotFound instead of 400 InvalidArgument.
AWS: Smithy: PutResourcePolicy input members ResourceArn (@required) and PolicyDocument (@required); http {POST /2020-05-31/put-resource-policy, code 200}; modeled errors AccessDenied (403), EntityNotFound (404), IllegalUpdate (400), InvalidArgument (400), PreconditionFailed (412), UnsupportedOperation (400). CloudFront resource policies apply only to a VPC origin, so an ARN of any other resource type is rejected on its type, not looked up: HTTP 400, error Code 'InvalidArgument', Message "The specified resource type isn't valid. Specify a VPC origin and try again." That message is fixed text in full — it names the supported resource class (VPC origin), not the caller's ARN — so it does not carry any value from the request or from state.
Emulator: answers HTTP 404 with $.ErrorResponse.Error.Code 'EntityNotFound' and Message "The specified resource doesn't exist. Update your request and try again." — the same generic refusal as Cause 1, so the emulator gives the caller no way to tell an unsupported resource type from an absent resource.
Why: a client attaching a policy to the wrong resource type is told the resource does not exist, so it retries or provisions instead of correcting the ARN; and because 404 EntityNotFound is the documented "not yet created" signal, a put-if-absent control loop treats a permanently invalid request as a transient gap and never terminates.
Fix: PutResourcePolicy's input validation — classify the ResourceArn by resource type before resolving it, and refuse an unsupported type with InvalidArgument (400) and AWS's wording; keep EntityNotFound (404) for a VPC-origin ARN that state does not hold. Both operations should share one ResourceArn validation step so the two refusal classes stay distinct.
Traces: cloudfront-resource-policy-not-found-errors#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false, 1303 steps, 1016 passed, 282 mismatched, 5 blocked, score 78.0%, 84 divergence clusters). Both clusters below have an empty owned_by and an empty stub_path.

Cause 1 — cloudfront-resource-policy-not-found-errors#task#1, GetResourcePolicy, expected status 400 / actual 404:
  status: expected 400, got 404
  $.ErrorResponse.Error.Code: value_mismatch (expected 'InvalidArgument', got 'EntityNotFound')
  $.ErrorResponse.Error.Message: value_mismatch (expected 'The ResourceArn parameter is required. Specify a valid ARN and try again.', got "The specified resource doesn't exist. Update your request and try again.")
  (divergence_key service=cloudfront|GetResourcePolicy|status=400->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status; affected_count 1)

Cause 2 — cloudfront-resource-policy-not-found-errors#task#2, PutResourcePolicy, expected status 400 / actual 404:
  status: expected 400, got 404
  $.ErrorResponse.Error.Code: value_mismatch (expected 'InvalidArgument', got 'EntityNotFound')
  $.ErrorResponse.Error.Message: value_mismatch (expected "The specified resource type isn't valid. Specify a VPC origin and try again.", got "The specified resource doesn't exist. Update your request and try again.")
  (divergence_key service=cloudfront|PutResourcePolicy|status=400->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status; affected_count 1)

Both expected messages are fixed text end to end — no request id, clock value, generated id or account-specific value is being asserted. Model grounding: smithy_query operation=GetResourcePolicy,PutResourcePolicy on services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json — ResourceArn @required on both; InvalidArgument httpError 400 and EntityNotFound httpError 404 both modeled on both operations.

## Steps to Reproduce
1. git_clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture cloudfront-resource-policy-not-found-errors and inspect step task#1 — POST /2020-05-31/get-resource-policy without a usable ResourceArn. The emulator answers 404 with Error.Code EntityNotFound and the generic "resource doesn't exist" message; the recording is 400 with Error.Code InvalidArgument and "The ResourceArn parameter is required. Specify a valid ARN and try again."
4. Cause 2: same fixture, step task#2 — POST /2020-05-31/put-resource-policy with a ResourceArn whose resource type is not a VPC origin (and a PolicyDocument). The emulator answers the identical 404 EntityNotFound; the recording is 400 InvalidArgument with "The specified resource type isn't valid. Specify a VPC origin and try again."
5. Confirm the two refusal classes are distinguishable afterwards: a well-formed VPC-origin ARN that state does not hold must still answer 404 EntityNotFound.
6. Both causes must match the recordings before closing.
