Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-admin.create-application-assignment-duplicate-idempotent
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin.create-application-assignment-duplicate-idempotent|run#12|AssertionError|assert message == _CONFLICT_MESSAGE, (
Issue-Commit: 767d3b2b262f92d14fe8b40882b5ff45cb4c157e

# CreateApplicationAssignment refuses a repeat grant of an existing (application, principal) tuple, and its ConflictException carries a message AWS never words that way

> _Note: this issue was identified against the code base at commit `767d3b2b262f92d14fe8b40882b5ff45cb4c157e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplicationAssignment

## Description
One root cause, in the duplicate-grant branch of the CreateApplicationAssignment handler.

Cause 1 of 1: a second CreateApplicationAssignment for a tuple the application already holds is refused instead of succeeding, and when a ConflictException IS the right answer the emulator words its message itself. Failing check: sso-admin.create-application-assignment-duplicate-idempotent.

AWS: Smithy (services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json) models CreateApplicationAssignment with input ApplicationArn (required, @length 10..1224, pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso::\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}$), PrincipalId (required, @length 1..47, GUID pattern) and PrincipalType (required, GROUP|USER), an EMPTY output, and errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400. ConflictException is documented narrowly: "Occurs when a conflict with a previous successful write is detected. This generally occurs when the previous write did not have time to propagate to the host serving the current request. A retry (with appropriate backoff logic) is the recommended response to this exception." — i.e. a propagation/race conflict, not the steady-state fact that the grant already exists. Recorded behavior: granting the same (ApplicationArn, PrincipalType, PrincipalId) tuple again converges on the one existing assignment — each caller gets HTTP 200 with the modeled empty body, and only a caller that loses the concurrent write race may be refused with ConflictException (HTTP 409), whose message is the FIXED text "There is a conflicting operation in process." with NO variable parts: no ARN, no principal id, no principal type are interpolated into it.

Emulator: the handler looks up application_assignment_position(...) and, whenever the tuple is already stored, returns ConflictException unconditionally — so the steady-state repeat grant (and any retry of a grant that already landed) is refused rather than answered 200. The message is built by crate::state::application_assignment_already_granted_message(application_arn, principal_type, principal_id) and reads, for the replayed check, 'An assignment of application arn:aws:sso::000000000000:application/ssoins-7223b1d1ac5e1f0e/apl-3f6d687a03e257c5 to GROUP 3440b303-3184-40b9-ac94-2ac427e0544e already exists.' Its fixed text is "An assignment of application " / " to " / " already exists."; the ARN, the principal type token and the principal id are values taken from the request. Neither that template nor those interpolations appear in AWS's wording for this error class.

Why: a client that retries a grant — the normal response to a throttle, a timeout, or the very ConflictException the model tells it to retry — gets a hard 409 from the emulator where AWS answers 200, so idempotent provisioning loops (Terraform/CDK reconcile, assignment sync jobs) fail permanently against the emulator instead of converging. A client that parses or matches on the conflict message, or surfaces it to an operator, sees text AWS never emits, and sees request-derived identifiers leaked into an error whose AWS form has none.

Fix: the duplicate-assignment branch of the CreateApplicationAssignment handler (services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_application_assignment.rs lines 198-222): an already-present tuple must return the empty 200 output with the stored assignments unchanged rather than a refusal; and the ConflictException message helper in crate::state (application_assignment_already_granted_message) must emit AWS's fixed conflict text for the race case it is genuinely for, with no request values interpolated. The ResourceNotFoundException and quota branches of the same handler are not in scope.

Traces: sso-admin.create-application-assignment-duplicate-idempotent (5 concurrent CreateApplicationAssignment calls for one (application, GROUP, group-id) tuple; the losing racers were refused with the emulator's own message).

## Evidence
Failing check sso-admin.create-application-assignment-duplicate-idempotent:

  AssertionError: expected message 'There is a conflicting operation in process.', got 'An assignment of application arn:aws:sso::000000000000:application/ssoins-7223b1d1ac5e1f0e/apl-3f6d687a03e257c5 to GROUP 3440b303-3184-40b9-ac94-2ac427e0544e already exists.'

Expected (AWS): a repeat CreateApplicationAssignment for an (ApplicationArn, PrincipalType, PrincipalId) tuple already assigned returns HTTP 200 with the modeled empty CreateApplicationAssignmentResponse; a caller refused for a genuine write race gets HTTP 409 ConflictException with the fixed message "There is a conflicting operation in process." (no interpolated values). ListApplicationAssignments then shows exactly one assignment for the principal.

Actual (emulator): HTTP 409 ConflictException for every repeat of the tuple, with message template "An assignment of application <ApplicationArn> to <PrincipalType> <PrincipalId> already exists." where the three values come from the request. Source: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_application_assignment.rs:204-222 returns ConflictException whenever store.application_assignment_position(..).is_some(), with the message from crate::state::application_assignment_already_granted_message(..).

Model evidence: ConflictException (com.amazonaws.ssoadmin#ConflictException, errorKind client, httpError 409, single member Message) is documented as a not-yet-propagated-write conflict that the caller should retry — not as a permanent "already exists" refusal. CreateApplicationAssignment carries no @idempotent trait, but AWS still converges repeat grants on the single logical assignment; DeleteApplicationAssignment is modeled @idempotent.

## Steps to Reproduce
1. Boot the emulator and point an sso-admin client at it.
2. Resolve an ACTIVE Identity Center instance: list_instances, else create_instance, then poll describe_instance until Status is ACTIVE; take IdentityStoreId from describe_instance.
3. identitystore.create_group(IdentityStoreId=..., DisplayName=<unique>) -> GroupId.
4. sso-admin.create_application(InstanceArn=..., ApplicationProviderArn='arn:aws:sso::aws:applicationProvider/custom', Name=<unique>) -> ApplicationArn.
5. Call create_application_assignment(ApplicationArn=<arn>, PrincipalId=<GroupId>, PrincipalType='GROUP') twice (or 5x concurrently, as the check does).
6. Observe: the first call returns 200; every later call returns HTTP 409 ConflictException with message 'An assignment of application <arn> to GROUP <GroupId> already exists.'
   Expected: the later calls return HTTP 200 with an empty body; if any is refused for a write race, its ConflictException message is exactly 'There is a conflicting operation in process.'
7. list_application_assignments(ApplicationArn=<arn>) shows exactly one assignment for <GroupId> in both cases.
