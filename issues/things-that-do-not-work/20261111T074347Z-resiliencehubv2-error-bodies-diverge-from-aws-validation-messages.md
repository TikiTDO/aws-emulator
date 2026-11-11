Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehubv2-assertion-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehubv2-assertion-lifecycle resiliencehubv2-input-source-lifecycle resiliencehubv2-policy-validation-and-conflict-errors
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2|CreateAssertion|4xx->4xx|$.message
Issue-Key: resiliencehubv2|CreateInputSource|4xx->4xx|$.message
Issue-Key: resiliencehubv2|CreatePolicy|4xx->4xx|$.message
Issue-Key: resiliencehubv2|CreatePolicy|4xx->2xx|$.message
Issue-Key: resiliencehubv2|CreatePolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resiliencehubv2|CreatePolicy|4xx->2xx|status
Issue-Commit: 5f88a7dcadd1e69adbda000c9f17c3a3daf7e228

# resiliencehubv2 error bodies diverge from AWS: validation messages reworded and clientToken conflict never raised

> _Note: this issue was identified against the code base at commit `5f88a7dcadd1e69adbda000c9f17c3a3daf7e228`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssertion

## Description
Wire replay of the simparity-wire-aws-v1 corpus against the emulator built from this checkout found FIVE distinct net-new causes on the resiliencehubv2 REFUSAL paths. All five must be closed before this issue is submitted. Causes 3, 4 and 5 touch the same CreatePolicy surface as the open behavioral issue "resiliencehubv2: 4 net-new causes - availabilitySlo.target allowed-set validation missing, clientToken reuse not guarded, and both duplicate-name ConflictException messages reworded" (20261110T225437Z-resiliencehubv2-4-net-new-causes-availabilityslo-target-allowed.md), but that issue carries only behavioral check ids and none of the wire fixture/step keys below, so none of these steps is re-replayed by it. Cause 1 also shows that CreateAssertion is no longer a 501 scaffold (its stub issue 20261030T104302Z-resiliencehubv2-createassertion-stub.md now looks stale): the operation validates and refuses, just with the wrong message.

=== Cause 1 of 5: CreateAssertion's ValidationException message for a too-short `text` is reworded ===
AWS: Smithy CreateAssertion (POST /v2/create-assertion) declares input member `text` -> AssertionText, required, @length min 1 max 1000, and error ValidationException (com.amazonaws.resiliencehubv2#ValidationException, @error client, httpError 400, member `message` String required). The refusal TEXT is recorded behavior only: AWS answers HTTP 400 / ValidationException with a bracketed JSON-schema style message of the form `[string "<value>" is too short (length: <actualLength>, required minimum: <modeledMinimum>)]`. The fixed text is `[string "`, then `" is too short (length: `, then `, required minimum: `, then `)]`; the variable parts are the rejected string value taken from the request, its actual length, and the member's modeled @length minimum (1 for AssertionText). AWS does NOT name the member in this message and does NOT use the "N validation error detected" form here.
Emulator: HTTP 400 is right and the error code is right, but the body's `message` is the generic constraint-report form `1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must have length greater than or equal to <min>` - different fixed text, and it adds a member path AWS does not include.
Why: clients, IaC providers and test suites that key on AWS's refusal text (or parse the bracketed schema report) do not recognise the refusal and surface it as an unknown validation failure; the emulator's extra member path is not part of AWS's response for this error class.
Fix: the shared request-validation layer's ValidationException message builder for this service (the centralized deserialize/validate step that runs before CreateAssertion's handler) - emit this service's bracketed schema-report wording for @length violations instead of the generic constraint-report wording.
Traces: resiliencehubv2-assertion-lifecycle#task#3.

=== Cause 2 of 5: CreateInputSource's ValidationException message for a union with no member set is reworded ===
AWS: Smithy CreateInputSource (POST /v2/create-input-source) declares `resourceConfiguration` -> ResourceConfiguration (a union, documented "Resource configuration for an input source. Provide exactly one field.") as required, and error ValidationException (httpError 400). The refusal TEXT is recorded behavior only: AWS answers HTTP 400 / ValidationException with `[instance failed to match exactly one schema (matched <matchedCount> out of <alternativeCount>)]`. The fixed text is `[instance failed to match exactly one schema (matched `, then ` out of `, then `)]`; the variable parts are how many union alternatives the submitted object matched (0 when no member is set) and the number of alternatives in the service's JSON schema for that union. That alternative count is a property of the service's published schema, not of the model's member count (the model lists 5 members for ResourceConfiguration while the recording reports 7), so it is recorded behavior and must be taken from the service's schema rather than computed from the member list.
Emulator: HTTP 400 and the error code are right, but the message is the emulator's own internal wording `union '<UnionShapeName>' had no member set` - entirely different fixed text, and it leaks an internal shape name.
Why: same impact as Cause 1 - message-matching clients misclassify a malformed-union refusal, and the response discloses an internal shape name AWS does not return.
Fix: the shared request-validation layer's ValidationException message builder, union branch - emit the bracketed "failed to match exactly one schema" report instead of the internal union wording.
Traces: resiliencehubv2-input-source-lifecycle#task#3.

=== Cause 3 of 5: CreatePolicy's ValidationException message for an out-of-range availabilitySlo.target is reworded ===
AWS: Smithy AvailabilitySlo is `structure { target: Double, @range min 0 max 100 }` and CreatePolicy (POST /v2/create-policy) declares ValidationException (httpError 400). The refusal TEXT is recorded behavior only: AWS answers HTTP 400 / ValidationException with `[format attribute "double" not supported, numeric instance is greater than the required maximum (maximum: <modeledMaximum>, found: <value>)]`. The fixed text is `[format attribute "double" not supported, numeric instance is greater than the required maximum (maximum: `, then `, found: `, then `)]`; the variable parts are the member's modeled @range maximum (100 for AvailabilitySlo$target) and the value taken from the request.
Emulator: HTTP 400 and the code are right, but the message is again the generic form `1 validation error detected: Value '<value>' at 'availabilitySlo.target' failed to satisfy constraint: Member must have value less than or equal to <max>`.
Why: same impact as Cause 1, on the policy-creation surface a client hits first.
Fix: the shared request-validation layer's ValidationException message builder, numeric-@range branch.
Traces: resiliencehubv2-policy-validation-and-conflict-errors#task#1 (the cluster's other member, task#2, is the same refusal on the same operation).

=== Cause 4 of 5: CreatePolicy's duplicate-name ConflictException message is reworded and leaks the existing policy ARN ===
AWS: Smithy CreatePolicy declares ConflictException (com.amazonaws.resiliencehubv2#ConflictException, @error client, httpError 409, member `message` String required, doc "Conflict - resource already exists") and CreatePolicy$name is EntityName (required, @length 2..60, pattern ^[A-Za-z0-9][A-Za-z0-9_\\-]{1,59}$). Recorded behavior: a second CreatePolicy using a live policy's name is refused with HTTP 409 / ConflictException and the message `Policy with name already exists: <name>` - fixed text `Policy with name already exists: ` followed by the name taken from the request, and nothing else.
Emulator: status 409 and code ConflictException are right, but the message is `A resilience policy named <name> already exists in this account: <existingPolicyArn>` - different fixed text plus an extra variable (the existing policy's ARN) that AWS does not return for this error.
Why: clients keying on AWS's refusal wording misclassify a name collision as an unknown conflict, and the response discloses an ARN that is not part of AWS's error body.
Fix: the CreatePolicy handler's duplicate-name refusal branch - word the ConflictException message exactly as AWS does and drop the ARN.
Traces: resiliencehubv2-policy-validation-and-conflict-errors#task#4.

=== Cause 5 of 5: CreatePolicy replays a clientToken used for a DIFFERENT request with HTTP 200 instead of refusing it with ConflictException ===
AWS: Smithy CreatePolicy$clientToken is ClientToken with @idempotencyToken (@length 1..63, pattern ^[A-Za-z0-9_.-]{0,63}$) and CreatePolicy declares ConflictException (httpError 409). Recorded behavior: reusing a token with parameters that differ from the request it was first accepted for is refused with HTTP 409 / ConflictException and the message `Got two different request with the same token: <clientToken>, please use a different token` - fixed text `Got two different request with the same token: ` and `, please use a different token`, with the clientToken value taken from the request. The response also carries the x-amzn-errortype header for the modeled error. The refused call must not mutate the stored policy. (An identical repeat with the same token must still replay the original policy.)
Emulator: answers HTTP 200 with the ORIGINAL policy body and no error at all - no `message`, no x-amzn-errortype header. The idempotency path replays whatever policy a known token maps to without recording a fingerprint of the request the token was first used for, so it cannot tell a true replay from a conflicting reuse.
Why: a client that accidentally reuses an idempotency token for different parameters gets a success for a policy it never asked for; it believes the new parameters were applied (they were not) and only discovers the mismatch on a later read, while the modeled 409/ConflictException AWS uses to surface token misuse never fires.
Fix: the CreatePolicy handler's idempotency branch plus the policy state store's token index - persist a fingerprint of the request each token was accepted with, replay only on an identical request, and refuse a differing one with the modeled ConflictException before anything is written.
Traces: resiliencehubv2-policy-validation-and-conflict-errors#task#5.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 13 fixtures replayed, 124 steps, 29 passed, score 23.4%). All five clusters below reported stub_path="" and owned_by=[] - none is blocked by an unimplemented operation and none is owned by an existing issue's replay key.

1) Cause 1 - fixture resiliencehubv2-assertion-lifecycle, step task#3, operation CreateAssertion, verdict mismatch, expected status 400 / actual status 400:
   $.message: value_mismatch
     expected: '[string "" is too short (length: 0, required minimum: 1)]'
     actual:   "1 validation error detected: Value '' at 'text' failed to satisfy constraint: Member must have length greater than or equal to 1"
   (the empty string and length 0 are this recording's request values filling the template; the fixed text and the `required minimum: 1` taken from AssertionText's @length are the requirement)

2) Cause 2 - fixture resiliencehubv2-input-source-lifecycle, step task#3, operation CreateInputSource, verdict mismatch, expected 400 / actual 400:
   $.message: value_mismatch
     expected: '[instance failed to match exactly one schema (matched 0 out of 7)]'
     actual:   "union 'ResourceConfiguration' had no member set"

3) Cause 3 - fixture resiliencehubv2-policy-validation-and-conflict-errors, step task#1, operation CreatePolicy, verdict mismatch, expected 400 / actual 400 (cluster also covers task#2):
   $.message: value_mismatch
     expected: '[format attribute "double" not supported, numeric instance is greater than the required maximum (maximum: 100, found: 100.5)]'
     actual:   "1 validation error detected: Value '100.5' at 'availabilitySlo.target' failed to satisfy constraint: Member must have value less than or equal to 100"

4) Cause 4 - fixture resiliencehubv2-policy-validation-and-conflict-errors, step task#4, operation CreatePolicy, verdict mismatch, expected 409 / actual 409:
   $.message: value_mismatch
     expected: 'Policy with name already exists: <name>'
     actual:   'A resilience policy named <name> already exists in this account: arn:aws:resiliencehub:<region>:<account>:policy/<uuid>'
   (the recording's name sim2real-probe-rhv2-a6c38727 and ARN .../policy/466a510c-b8ed-87c1-a927-0f0c0f7524ba are per-run values; the fixed text is the requirement, and the ARN must not be in the message at all)

5) Cause 5 - fixture resiliencehubv2-policy-validation-and-conflict-errors, step task#5, operation CreatePolicy, verdict mismatch:
   status: expected 409, got 200
   $.message: missing (expected 'Got two different request with the same token: <clientToken>, please use a different token', got None)
   $headers.x-amzn-errortype: missing (expected the modeled error type header, got None)
   (the recording's token sim2real-probe-rhv2-tok-a57393e1 is a per-run value filling <clientToken>)

Model evidence (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json, service NGRHServiceCore, sdkId resiliencehubv2, protocol restJson1):
- CreateAssertion: POST /v2/create-assertion; text -> AssertionText required @length 1..1000; errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400.
- CreateInputSource: POST /v2/create-input-source; resourceConfiguration -> ResourceConfiguration (union, 5 modeled members: resourceTags, cfnStackArn, tfStateFileUrl, eks, designFileS3Url) required; same error set.
- CreatePolicy: POST /v2/create-policy; name -> EntityName required @length 2..60; availabilitySlo -> AvailabilitySlo { target: Double @range 0..100 }; clientToken -> ClientToken @idempotencyToken @length 1..63; same error set.
- ValidationException: @error client, httpError 400, members message (required), reason, fieldList. ConflictException: @error client, httpError 409, member message (required).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1 and inspect the five fixture#step keys named under Traces.
3. Cause 1: POST /v2/create-assertion with a valid serviceArn and text="". Observe HTTP 400 with message "1 validation error detected: Value '' at 'text' failed to satisfy constraint: Member must have length greater than or equal to 1" instead of AWS's '[string "" is too short (length: 0, required minimum: 1)]'.
4. Cause 2: POST /v2/create-input-source with a valid serviceArn and resourceConfiguration={} (no member set). Observe HTTP 400 with message "union 'ResourceConfiguration' had no member set" instead of '[instance failed to match exactly one schema (matched 0 out of 7)]'.
5. Cause 3: POST /v2/create-policy with a valid unique name and availabilitySlo={"target": 100.5}. Observe HTTP 400 with the "1 validation error detected: ..." message instead of '[format attribute "double" not supported, numeric instance is greater than the required maximum (maximum: 100, found: 100.5)]'.
6. Cause 4: POST /v2/create-policy with name=N, then POST /v2/create-policy again with the same name. The 409 body's message is "A resilience policy named N already exists in this account: <arn>" instead of "Policy with name already exists: N".
7. Cause 5: POST /v2/create-policy with name=N, clientToken=T and one set of targets; then POST /v2/create-policy again with the same name and token but DIFFERENT targets. The emulator answers HTTP 200 with the original policy and no x-amzn-errortype header; AWS answers HTTP 409 ConflictException "Got two different request with the same token: T, please use a different token".
