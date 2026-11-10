Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resiliencehubv2.availability-slo-target-rejects-values-outside-the-allowed-set resiliencehubv2.create-policy-client-token-replays-and-guards-the-original-request resiliencehubv2.create-policy-rejects-duplicate-name-until-deleted resiliencehubv2.create-service-rejects-duplicate-name-until-deleted
Issue-Shape-Id: com.amazonaws.resiliencehubv2#NGRHServiceCore
Issue-Key: resiliencehubv2.availability-slo-target-rejects-values-outside-the-allowed-set|_error#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: resiliencehubv2.create-policy-client-token-replays-and-guards-the-original-request|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: resiliencehubv2.create-policy-rejects-duplicate-name-until-deleted|run#6|AssertionError|assert (code, message) == ("ConflictException", f"Policy with name already exists: {name}"), (
Issue-Key: resiliencehubv2.create-service-rejects-duplicate-name-until-deleted|run#6|AssertionError|assert (code, message) == ("ConflictException", f"Service with name already exists: {name}"), (
Issue-Commit: 13cbee553af2e776db8b2ebed42254e169329488

# resiliencehubv2: 4 net-new causes — availabilitySlo.target allowed-set validation missing, clientToken reuse not guarded, and both duplicate-name ConflictException messages reworded

> _Note: this issue was identified against the code base at commit `13cbee553af2e776db8b2ebed42254e169329488`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicy

## Description
Behavioral replay of the resiliencehubv2 suite against the emulator built from this checkout found FOUR distinct net-new causes in the policy/service create paths. All four must be closed before this issue is submitted. None is covered by the open issues "Policy lifecycle responses diverge from AWS: empty tags dropped, multiAz not reflected, deleted policy still served 200" (20261104T144946Z-...), which owns the tags/multiAz/get-after-delete projection causes, nor by the CreateSystem / CreateAssertion / CreateTest / DeletePolicy / UpdatePolicy scaffold-stub issues, nor by the CreateService invoker-role issue (20261029T191749Z-iam-resiliencehubv2-...).

=== Cause 1 of 4: CreatePolicy accepts an availabilitySlo.target outside the allowed set instead of refusing it with ValidationException ===
AWS: Smithy models AvailabilitySlo$target as Double with @range min 0 max 100 only, so the allowed-value rule is recorded behavior only: AWS accepts exactly the discrete availability targets 99.9, 99.95 and 99.99 and refuses any other value on CreatePolicy (and on UpdatePolicy) with HTTP 400, error code ValidationException (com.amazonaws.resiliencehubv2#ValidationException, @error client, httpError 400, member message required) and the message "Invalid input for field availabilitySlo.target. Allowed values are [<allowed targets>]." — the fixed text is "Invalid input for field availabilitySlo.target. Allowed values are [", then the three allowed targets joined by ", " (AWS lists them in no fixed order), then "].". The member name in the message is the request's field path; the listed values are the service's allowed set, not a per-run value. A refused create must leave no policy behind (the name does not appear in ListPolicies).
Emulator: CreatePolicy with availabilitySlo={"target": 99.0} answers HTTP 200 and CREATES the policy, echoing availabilitySlo {"target": 99.0} in the response body. src/ops/create_policy.rs handle() stores input.availability_slo verbatim with no allowed-set check; the only bound applied is the model's @range 0..100, so any value in that range is stored.
Why: a client that relies on AWS to reject an unsupported SLO silently ends up with a policy whose availability target AWS would never have accepted. Resilience assessments and any downstream logic keyed on the discrete target set take an undefined branch, and validation-driven UIs/IaC that expect a 400 ValidationException proceed as if the input were valid.
Fix: the CreatePolicy handler's input validation (and the same check on the UpdatePolicy handler once it is implemented), rejecting a target not in the allowed set with the modeled ValidationException before anything is written.
Traces: resiliencehubv2.availability-slo-target-rejects-values-outside-the-allowed-set (the check fails on the first CreatePolicy; its UpdatePolicy half is not reached).

=== Cause 2 of 4: CreatePolicy replays a clientToken for a DIFFERENT request instead of refusing it with ConflictException ===
AWS: Smithy CreatePolicy$clientToken carries @idempotencyToken (ClientToken, @length 1..63, pattern ^[A-Za-z0-9_.-]{0,63}$) and CreatePolicy declares ConflictException (@error client, httpError 409). Recorded behavior: repeating CreatePolicy with the SAME token and the same parameters returns the originally created policy (same policyArn, same createdAt) and creates no second policy; reusing that token with DIFFERENT parameters is refused with HTTP 409, error code ConflictException and the message "Got two different request with the same token: <clientToken>, please use a different token" — the fixed text is "Got two different request with the same token: " and ", please use a different token", and the variable part is the clientToken value taken from the request. The refused call must not mutate the stored policy.
Emulator: the third call — same name and same clientToken but availabilitySlo target 99.99 instead of 99.9 — answers HTTP 200 with the ORIGINAL policy body (target 99.9) instead of raising ConflictException. src/ops/create_policy.rs handle() looks the token up in store.policy_idempotency and returns the stored policy unconditionally; it records no fingerprint of the request the token was first used for, so it cannot tell a true replay from a conflicting reuse.
Why: a client that accidentally reuses an idempotency token for a different policy gets a success for a policy it did not ask for. The caller believes the new parameters were applied (they were not), so the mismatch is discovered only on a later read, and the modeled 409/ConflictException that AWS uses to surface token misuse never fires.
Fix: the CreatePolicy handler's idempotency path plus the policy state store's token index — persist a fingerprint of the request each token was first accepted with, replay only on an identical request, and refuse a differing one with the modeled ConflictException.
Traces: resiliencehubv2.create-policy-client-token-replays-and-guards-the-original-request (the same-parameter replay assertions pass; the differing-parameter call is the divergence).

=== Cause 3 of 4: CreatePolicy's duplicate-name ConflictException message is reworded ===
AWS: Smithy CreatePolicy declares ConflictException (httpError 409, "Conflict — resource already exists"); CreatePolicy$name is EntityName (required, @length 2..60, pattern ^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$, "Resource name (used in ARN)"). Recorded behavior: a second CreatePolicy using a live policy's name is refused with HTTP 409, code ConflictException and the message "Policy with name already exists: <name>" — fixed text "Policy with name already exists: " followed by the name taken from the request, and nothing else. The status and code are correct in the emulator; only the message diverges. (After the first policy is deleted the name is free again — that half of the check is not reached.)
Emulator: the message is "A resilience policy named <name> already exists in this account: <policyArn>" — different fixed text and an extra variable (the existing policy's ARN). Built in src/ops/create_policy.rs handle(), the store.policy_named(&input.name) branch.
Why: message-matching clients, IaC providers and test suites that key on the AWS refusal text do not recognise the refusal, so "name already taken" is reported as an unknown conflict; the ARN leaked into the message is also not part of AWS's response for this error.
Fix: the CreatePolicy handler's duplicate-name refusal branch — word the ConflictException message exactly as AWS does.
Traces: resiliencehubv2.create-policy-rejects-duplicate-name-until-deleted.

=== Cause 4 of 4: CreateService's duplicate-name ConflictException message is reworded ===
AWS: Smithy CreateService declares ConflictException (httpError 409) and CreateService$name is EntityName (required, @length 2..60, same pattern). Recorded behavior: a second CreateService using a live service's name is refused with HTTP 409, code ConflictException and the message "Service with name already exists: <name>" — fixed text "Service with name already exists: " plus the name from the request. The refused create must leave the live service untouched.
Emulator: the message is "A service named <name> already exists in this account: <serviceArn>" — different fixed text plus the existing service's ARN. Built in src/ops/create_service.rs handle(), the store.service_names.get(&input.name) branch. Status 409 and code ConflictException are correct.
Why: same impact as Cause 3 on the service surface — clients keying on AWS's refusal wording misclassify a name collision, and the response discloses an ARN AWS does not return for this error.
Fix: the CreateService handler's duplicate-name refusal branch — word the ConflictException message exactly as AWS does. Causes 3 and 4 are the same wording defect in two handlers and must both be corrected.
Traces: resiliencehubv2.create-service-rejects-duplicate-name-until-deleted.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (bootstrap_signal=false; resiliencehubv2 2/35 checks passed).

1) Cause 1 — check resiliencehubv2.availability-slo-target-rejects-values-outside-the-allowed-set, verdict mismatch:
   rh.create_policy(name=<unique>, availabilitySlo={"target": 99.0}) was expected to raise ValidationException with message "Invalid input for field availabilitySlo.target. Allowed values are [99.9, 99.95, 99.99]." (the check grades the allowed-target set irrespective of order, then the whole text).
   Actual: AssertionError: expected the call to be refused, but it succeeded: HTTPStatusCode 200, body {'policy': {'policyArn': 'arn:aws:resiliencehub:<region>:<account>:policy/<uuid>', 'name': '<unique>', 'availabilitySlo': {'target': 99.0}, 'sharingEnabled': False, 'associatedServiceCount': 0, ...}} (ARN/uuid/name/timestamps are per-run values).

2) Cause 2 — check resiliencehubv2.create-policy-client-token-replays-and-guards-the-original-request, verdict mismatch:
   After create_policy(name=N, clientToken=T, availabilitySlo={'target': 99.9}) and an identical replay (which correctly returned the same policyArn/createdAt), create_policy(name=N, clientToken=T, availabilitySlo={'target': 99.99}) was expected to raise ConflictException with message "Got two different request with the same token: <T>, please use a different token".
   Actual: AssertionError: expected the call to be refused, but it succeeded: HTTPStatusCode 200, body {'policy': {... 'availabilitySlo': {'target': 99.9} ...}} — i.e. the original policy replayed for a differing request.

3) Cause 3 — check resiliencehubv2.create-policy-rejects-duplicate-name-until-deleted, verdict mismatch:
   expected (code, message) == ('ConflictException', 'Policy with name already exists: <name>')
   actual: AssertionError: duplicate CreatePolicy: ConflictException: 'A resilience policy named <name> already exists in this account: arn:aws:resiliencehub:<region>:<account>:policy/<uuid>'

4) Cause 4 — check resiliencehubv2.create-service-rejects-duplicate-name-until-deleted, verdict mismatch:
   expected (code, message) == ('ConflictException', 'Service with name already exists: <name>')
   actual: AssertionError: duplicate CreateService: ConflictException: 'A service named <name> already exists in this account: arn:aws:resiliencehub:<region>:<account>:service/<uuid>'

Model evidence (services/service-com-amazonaws-resiliencehubv2-ngrhservicecore/smithy-model/com-amazonaws-resiliencehubv2-ngrhservicecore.json):
- CreatePolicy: POST /v2/create-policy, input name (EntityName, required, len 2..60), availabilitySlo (AvailabilitySlo), clientToken (ClientToken @idempotencyToken, len 1..63); errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400.
- AvailabilitySlo: structure { target: Double @range min 0 max 100 } — no enumerated allowed set in the model, so the allowed-target rule is recorded behavior.
- ConflictException: @error client, httpError 409, message (String, required). ValidationException: @error client, httpError 400, message (required), reason, fieldList.
- CreateService: POST /v2/create-service, input name (EntityName, required), regions, permissionModel, clientToken (@idempotencyToken); same error set.

Emulator code evidence:
- src/ops/create_policy.rs handle(): stores input.availability_slo verbatim (no allowed-set check); the clientToken branch returns store.policy_view(...) for any request bearing a known token; the duplicate-name branch formats "A resilience policy named {} already exists in this account: {}".
- src/ops/create_service.rs handle(): duplicate-name branch formats "A service named {} already exists in this account: {existing}"; the clientToken branch replays store.idempotency unconditionally.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and look at the resiliencehubv2 checks listed under trace_ids.
3. Cause 1: POST /v2/create-policy with a valid unique name and availabilitySlo={"target": 99.0}. The emulator answers 200 and the policy appears in ListPolicies; AWS answers 400 ValidationException "Invalid input for field availabilitySlo.target. Allowed values are [99.9, 99.95, 99.99]." and creates nothing.
4. Cause 2: POST /v2/create-policy with name=N, clientToken=T, availabilitySlo={"target": 99.9}; repeat the identical call (correctly replays); then repeat it with availabilitySlo={"target": 99.99} and the same name and token. The emulator answers 200 with the original policy; AWS answers 409 ConflictException "Got two different request with the same token: T, please use a different token".
5. Cause 3: POST /v2/create-policy with name=N, then POST /v2/create-policy again with the same name. The 409 body's message is "A resilience policy named N already exists in this account: <arn>" instead of "Policy with name already exists: N".
6. Cause 4: create an IAM role resiliencehub.amazonaws.com may assume, POST /v2/create-service with name=S, regions=[<region>], permissionModel={"invokerRoleName": <role>}, then POST /v2/create-service again with the same name. The 409 body's message is "A service named S already exists in this account: <arn>" instead of "Service with name already exists: S".
