Issue-Source: trace-sweep-divergence
Issue-Trace: iot-authorization-testing-apis
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-authorization-testing-apis iot-vpce-traj-attach-target-mistakes iot-vpce-traj-effective-policy-lookup-mistakes iot-vpce-traj-fix-endpoint-via-policy-version
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|GetEffectivePolicies|4xx->4xx|$.message
Issue-Key: iot|GetEffectivePolicies|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|GetEffectivePolicies|4xx->4xx|status
Issue-Key: iot|GetEffectivePolicies|4xx->2xx|$.message
Issue-Key: iot|GetEffectivePolicies|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|GetEffectivePolicies|4xx->2xx|status
Issue-Key: iot|GetEffectivePolicies|2xx->2xx|$.effectivePolicies[*].policyDocument
Issue-Commit: 42b0d911cbfe674c0ad13877c2fae9531eb43724

# iot GetEffectivePolicies diverges on 5 causes: identity validation, refusal wording and stale default policy document

> _Note: this issue was identified against the code base at commit `42b0d911cbfe674c0ad13877c2fae9531eb43724`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEffectivePolicies

## Description
Five wire-replay divergence clusters on iot `GetEffectivePolicies` survive the replay report's ALREADY-FILED / OWNED markers (every one reports `stub_path: ""` and `owned_by: []`) and are bundled here, because they are all faults of the same handler: its identity-member validation, its refusal wording, and the policy document it projects. FIVE causes must be closed before submitting.

Related open issues that do NOT carry any of these replay keys, so they do not cover these causes: 20261110T010104Z-iot-6-net-new-causes-deletepolicy-geteffectivepolicies-updaterolealias.md (behavioral, records only the both-identities-supplied refusal, which the emulator now implements), 20261107T022027Z-iot-refusal-envelopes-8-causes-wrong-x-amzn.md (other operations' envelopes), 20261107T022027Z-iot-policy-document-plane-10-causes-document-validation.md (policy-version reads on GetPolicy/GetPolicyVersion, not the effective-policy projection), and 20261106T201842Z-iot-registry-plane-9-net-new-causes-creatething.md (registry wording on CreateThing/DeleteThing, not this operation's thing miss).

Cause 1 of 5: a request that names a `thingName` but NEITHER `principal` NOR `cognitoIdentityPoolId` is not refused; the thing lookup runs first and answers not-found.
AWS: Smithy — `GetEffectivePolicies` (POST /effective-policies) takes `principal`, `cognitoIdentityPoolId` and `thingName` (httpQuery `thingName`, @length 1..128, pattern `^[a-zA-Z0-9:_-]+$`) as three separate optional members, and models `InvalidRequestException` @httpError 400 and `ResourceNotFoundException` @httpError 404. Recorded behavior: AWS requires an IDENTITY — one of `principal` or `cognitoIdentityPoolId` — before it looks at anything else, and `thingName` alone does not satisfy that requirement. A request with `thingName` only is refused with HTTP 400, error code `InvalidRequestException` (`x-amzn-errortype: InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/`) and the message `Both principal and CognitoIdentityPoolId can not be null.` — the message is entirely FIXED text, with no variable part taken from the request or from state. This refusal takes precedence over resolving the thing: the recorded request named a thing this account does not hold and AWS still answered the 400, never the 404.
Emulator: the handler only refuses when all THREE members are absent, so a `thingName`-only request falls through to the thing lookup and answers HTTP 404 `ResourceNotFoundException` with the message `Thing <account>:<thingName> not found.`
Why: a client that forgot the identity member is told its thing does not exist. It then goes looking for a registry problem that does not exist (or creates the thing and retries, getting a different wrong answer — see Cause 2) instead of adding the `principal`/`cognitoIdentityPoolId` the API requires. The 400-vs-404 inversion also breaks error-class handling: a caller that treats `ResourceNotFoundException` as "skip this device" silently drops an authorization check it should have failed loudly on.
Fix: the GetEffectivePolicies handler's input validation — the identity requirement must be checked, and must be checked BEFORE the `thingName` resolution and before any principal resolution — and the fixed refusal text it emits.
Traces: iot-authorization-testing-apis#task#7.

Cause 2 of 5: the same missing-identity request against an EXISTING thing answers 200 with a policy list instead of the 400 refusal.
AWS: Smithy as in Cause 1. Recorded behavior: with `thingName` naming a thing this account does hold and no `principal`/`cognitoIdentityPoolId`, AWS still refuses with HTTP 400 `InvalidRequestException` and the same fixed text `Both principal and CognitoIdentityPoolId can not be null.`; it never answers 200. There is no "thing-only" success form of this operation.
Emulator: answers HTTP 200 with an `effectivePolicies` list and no error body at all — the replay reports the expected `$.message` and `x-amzn-errortype` as missing because the response is a success.
Why: the worst form of this fault. A caller asking "which policies govern this device?" without an identity gets a plausible-looking authorization answer that AWS would never have produced, so a VPC-endpoint / policy-attachment audit concludes a device is governed (or ungoverned) on the strength of a response the real API refuses. Nothing in the answer signals that the request was malformed.
Fix: same component as Cause 1 — the GetEffectivePolicies handler's identity-member validation, which must refuse before computing any scope rather than returning an empty-or-partial success.
Traces: iot-vpce-traj-attach-target-mistakes#task#15.

Cause 3 of 5: a `principal` that is not an ARN is refused with emulator-authored policy-target prose instead of AWS's Cognito-identity refusal.
AWS: Smithy — `principal` (shape `Principal`) documents its valid forms as `CertificateArn (arn:aws:iot:region:accountId:cert/certificateId)`, `thingGroupArn (arn:aws:iot:region:accountId:thinggroup/groupName)` and `CognitoId (region:id)`; the operation models `InvalidRequestException` @httpError 400. Recorded behavior: a `principal` that is not one of the ARN forms is classified as a CognitoId, and AWS then requires the pool it belongs to, refusing with HTTP 400 `InvalidRequestException` and the message `CognitoIdentityPoolId must be provided if your principal is CognitoIdentityId.` — entirely FIXED text, no variable part: the offending value is NOT echoed. The recorded request passed a bare 64-hex certificate id (the id without its ARN wrapper), i.e. exactly the common mistake this refusal exists to name.
Emulator: the status is right (400 `InvalidRequestException`) but the message is its own `resolve_policy_target` prose: `Invalid policy target: <value>. A policy target is a certificate ARN (arn:aws:iot:region:accountId:cert/certificateId), a thing group ARN (arn:aws:iot:region:accountId:thinggroup/groupName) or a Cognito identity (region:id)` — fixed text the API does not use, plus the request's value echoed where AWS echoes nothing.
Why: the message is the only part of this refusal that tells a caller what to do, and the two messages prescribe different fixes. AWS says "you gave me a Cognito identity, now give me its pool id"; the emulator says "your target is malformed". A client (or a human reading a log) following the emulator's wording reformats the principal instead of supplying `cognitoIdentityPoolId`, and any assertion or log scraper keyed to AWS's fixed sentence never matches.
Fix: the GetEffectivePolicies handler's principal validation — a non-ARN principal must take this operation's own Cognito-identity refusal text rather than delegating to the shared policy-target parser's message.
Traces: iot-vpce-traj-effective-policy-lookup-mistakes#task#6 (also #task#7).

Cause 4 of 5: the thing-not-found refusal is worded as a generic registry miss instead of this operation's text.
AWS: Smithy — `ResourceNotFoundException` @httpError 404 is modeled for this operation. Recorded behavior: when `thingName` names a thing the account does not hold, AWS answers HTTP 404 `ResourceNotFoundException` with the message `<thingName> thing doesn't exists` — the FIXED text is ` thing doesn't exists` and the single variable part is the request's `thingName`; the account id is NOT part of the message and the name is not prefixed by anything.
Emulator: answers the right status and error class but the shared registry wording `Thing <account>:<thingName> not found.` — different fixed text, an account id AWS does not include, a prefixed name, and a trailing period AWS does not send.
Why: the message is a requirement of the error, and this one differs in every fixed token. Clients and tests that match AWS's sentence (or parse the thing name out of it) fail against the emulator, and the leaked `<account>:` prefix makes the response look like it is about a differently-named resource than the one requested.
Fix: the GetEffectivePolicies handler's `thingName` miss path — it currently reuses `crate::state::missing_thing_message` (services/service-com-amazonaws-iot-awsiotservice/src/ops/get_effective_policies.rs:229) and must emit this operation's own fixed text with the request's `thingName` as the only variable part. Note this refusal is also ordered AFTER the identity check per Cause 1.
Traces: iot-vpce-traj-effective-policy-lookup-mistakes#task#9.

Cause 5 of 5: the effective policy document is a stale version — the policy's CURRENT default version is not what gets projected.
AWS: Smithy — `EffectivePolicy` carries `policyName`, `policyArn` and `policyDocument` ("The IAM policy document"). Recorded behavior: `policyDocument` is the document of the policy version that is in effect NOW, i.e. the policy's current default version. After a new version is created and made default (CreatePolicyVersion with setAsDefault, or SetDefaultPolicyVersion), the very next GetEffectivePolicies for a principal that policy is attached to reports the NEW default version's document; the superseded version's document is never reported again.
Emulator: reports the document of the earlier version — the recorded step's two documents are byte-identical except for the one condition value the new version changed (an `aws:SourceVpce` value in `Condition.StringEquals`), and the emulator serves the pre-update value while AWS serves the updated one. The concrete vpce ids in the recording are per-run values and are not the requirement; the requirement is that the projected document equals the policy's current default version's document.
Why: this is an authorization answer that is wrong in the direction that matters. A policy whose condition was just corrected (or tightened) still reads as the old condition, so a client verifying "did my policy fix take effect?" through GetEffectivePolicies concludes it did not — or, worse, a client that reads the operation as the source of truth for a device's permissions makes a grant/deny decision from a document the service has already replaced.
Fix: the GetEffectivePolicies handler's document projection (services/service-com-amazonaws-iot-awsiotservice/src/ops/get_effective_policies.rs:252 calls `policy.default_document()`) together with the policy state that records which version is default — the default-version pointer written by CreatePolicyVersion(setAsDefault)/SetDefaultPolicyVersion in services/service-com-amazonaws-iot-awsiotservice/src/state.rs must be what this read follows.

## Evidence
All five are wire-replay divergence clusters from trace set simparity-wire-aws-v1 (service `iot`, operation `GetEffectivePolicies`), each with `verdict: mismatch`, `stub_path: ""` and `owned_by: []`.

Cause 1 — fixture iot-authorization-testing-apis, step task#7
  Expected: HTTP 400; body `{"message": "Both principal and CognitoIdentityPoolId can not be null."}`; header `x-amzn-errortype: InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/`.
  Actual:   HTTP 404; `$.message` = `Thing 000000000000:sim2real-probe-iot-iot-thing-18b8f741-missing not found.`; header `x-amzn-errortype: ResourceNotFoundException`.
  (divergence_key `service=iot|GetEffectivePolicies|status=400->404|keys=$.message,$headers.x-amzn-errortype,status`)

Cause 2 — fixture iot-vpce-traj-attach-target-mistakes, step task#15
  Expected: HTTP 400; `$.message` = `Both principal and CognitoIdentityPoolId can not be null.`; an `x-amzn-errortype` error header present.
  Actual:   HTTP 200; no `message`, no `x-amzn-errortype` — a success response where AWS refuses.
  (divergence_key `service=iot|GetEffectivePolicies|status=400->200|keys=$.message,$headers.x-amzn-errortype,status`)

Cause 3 — fixture iot-vpce-traj-effective-policy-lookup-mistakes, steps task#6 and task#7 (2 steps in the cluster)
  Expected: HTTP 400; `$.message` = `CognitoIdentityPoolId must be provided if your principal is CognitoIdentityId.`
  Actual:   HTTP 400; `$.message` = `Invalid policy target: 24da2face5104318fee6710830eed81853b5066b87aed7d840b3928db6f73280. A policy target is a certificate ARN (arn:aws:iot:region:accountId:cert/certificateId), a thing group ARN (arn:aws:iot:region:accountId:thinggroup/groupName) or a Cognito identity (region:id)`
  (divergence_key `service=iot|GetEffectivePolicies|status=400->400|keys=$.message`)

Cause 4 — fixture iot-vpce-traj-effective-policy-lookup-mistakes, step task#9
  Expected: HTTP 404; `$.message` = `sim2real-probe-iotvpce-thing-165280d7-absent thing doesn't exists`
  Actual:   HTTP 404; `$.message` = `Thing 000000000000:sim2real-probe-iotvpce-thing-165280d7-absent not found.`
  (divergence_key `service=iot|GetEffectivePolicies|status=404->404|keys=$.message`)

Cause 5 — fixture iot-vpce-traj-fix-endpoint-via-policy-version, step task#10
  Expected: HTTP 200; `$.effectivePolicies[0].policyDocument` = the policy's current default version, i.e. the document whose `Statement[0].Condition.StringEquals."aws:SourceVpce"` is the value written by the version just promoted to default (recorded run: `vpce-0a1b2c3d4e5f60718`).
  Actual:   HTTP 200; `$.effectivePolicies[0].policyDocument` = the superseded version's document, same JSON with the pre-update condition value (recorded run: `vpce-0f1e2d3c4b5a69788`).
  (divergence_key `service=iot|GetEffectivePolicies|status=200->200|keys=$.effectivePolicies[0].policyDocument`)

Emulator code backing the diagnosis (services/service-com-amazonaws-iot-awsiotservice/src/ops/get_effective_policies.rs): lines 174-179 refuse only when `principal`, `cognitoIdentityPoolId` AND `thing_name` are all absent, with the emulator-authored text `Principal, CognitoIdentityPoolId or thingName must be provided.` (Causes 1-2); lines 188-202 delegate principal classification to `store.resolve_policy_target`, whose `Invalid` message is returned verbatim (Cause 3); line 229 returns `crate::state::missing_thing_message(thing_name)` (Cause 4); line 252 projects `policy.default_document()` (Cause 5).

## Steps to Reproduce
1. git_clone the repo, build and boot the emulator, then replay trace set simparity-wire-aws-v1 and re-run these fixture steps:
   - iot-authorization-testing-apis#task#7 (Cause 1)
   - iot-vpce-traj-attach-target-mistakes#task#15 (Cause 2)
   - iot-vpce-traj-effective-policy-lookup-mistakes#task#6 and #task#7 (Cause 3)
   - iot-vpce-traj-effective-policy-lookup-mistakes#task#9 (Cause 4)
   - iot-vpce-traj-fix-endpoint-via-policy-version#task#10 (Cause 5)

2. Minimal repro with a botocore `iot` client against the running emulator (any region, any account):
   - Cause 1: `get_effective_policies(thingName='no-such-thing-<rand>')` (no principal, no cognitoIdentityPoolId). Expect HTTP 400 `InvalidRequestException` / `Both principal and CognitoIdentityPoolId can not be null.`; observe HTTP 404 `ResourceNotFoundException` / `Thing <account>:<thingName> not found.`
   - Cause 2: `create_thing(thingName=T)` then `get_effective_policies(thingName=T)` with no identity member. Expect the same HTTP 400 refusal; observe HTTP 200 with an `effectivePolicies` list.
   - Cause 3: `get_effective_policies(principal='<64-hex certificate id, no ARN wrapper>')`. Expect HTTP 400 `InvalidRequestException` / `CognitoIdentityPoolId must be provided if your principal is CognitoIdentityId.`; observe HTTP 400 with the `Invalid policy target: ...` prose instead.
   - Cause 4: `get_effective_policies(principal='<valid cert ARN>', thingName='no-such-thing-<rand>')`. Expect HTTP 404 `ResourceNotFoundException` / `<thingName> thing doesn't exists`; observe `Thing <account>:<thingName> not found.`
   - Cause 5: `create_policy(policyName=P, policyDocument=D1)`; `create_keys_and_certificate(setAsActive=True)`; `attach_policy(policyName=P, target=<certArn>)`; `get_effective_policies(principal=<certArn>)` and note the document; then `create_policy_version(policyName=P, policyDocument=D2, setAsDefault=True)` where D2 differs from D1 in one condition value; `get_policy(policyName=P)` reports D2 as the default version's document. Re-run `get_effective_policies(principal=<certArn>)`: expect `effectivePolicies[0].policyDocument == D2`; observe D1.
