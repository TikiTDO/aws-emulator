Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.account-holds-at-most-one-domain cloudwatchomni.account-region-holds-at-most-one-space cloudwatchomni.dashboard-names-are-not-unique-within-a-space cloudwatchomni.deleting-an-access-profile-removes-the-grants-naming-it cloudwatchomni.create-view-retry-with-same-client-token-is-refused-as-a-conflict
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.account-holds-at-most-one-domain|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: cloudwatchomni.account-region-holds-at-most-one-space|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: cloudwatchomni|CreateOmniDashboard|ConflictException
Issue-Key: cloudwatchomni|DeleteAccessProfile|ConflictException
Issue-Key: cloudwatchomni.create-view-retry-with-same-client-token-is-refused-as-a-conflict|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# cloudwatchomni: account/region singleton rules unenforced and name-uniqueness/cascade rules inverted (CreateDomain, CreateSpace, CreateOmniDashboard, DeleteAccessProfile, CreateView clientToken)

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
FIVE net-new root causes in the cloudwatchomni emulator, all of them resource-rule defects: two AWS conflict rules are not enforced at all, two rules the emulator invents are enforced where AWS allows the call, and one idempotency-token short-circuit swallows a conflict. Each cause needs its own fix; close all five before submitting.

None of the five is covered by an open issue. The open cloudwatchomni issues are unimplemented-stub reports (CreateIntegration, ListDomains) and two message-wording issues (20261109T172919Z-cloudwatchomni-resourcenotfoundexception-messages-do-not-match-aws-wording.md, 20261109T172919Z-cloudwatchomni-validationexception-message-echoes-value-and-unordered-enum.md); none of them concerns these state rules.

Cause 1 of 5: CreateDomain does not enforce the one-domain-per-account rule.
  AWS: Smithy CreateDomain declares ConflictException (errorKind client, @httpError 409); the model does not state the cardinality rule, so the rule itself is recorded behavior only. The recording shows that while an account already holds a CloudWatch Omni domain, a second CreateDomain is refused with 409 ConflictException and the message "A domain already exists for this account" — entirely fixed text, with no variable part (it does not name the existing or requested domain). After DeleteDomain the name/account is free and CreateDomain succeeds again with a fresh domainId.
  Emulator: the second CreateDomain returns HTTP 200 with a complete second `domain` member (a second domainId, domainArn and endpoint URLs), so the account ends up holding two domains.
  Why: a client that treats the domain as an account singleton (the normal pattern: create-or-reuse driven by ConflictException, or a CloudFormation-style provisioner that relies on the refusal to converge) creates duplicate estates against the emulator and then diverges from AWS on every ListDomains/GetDomain that follows; code paths handling ConflictException are never exercised.
  Fix: CreateDomain's handler/state layer — the account-scoped domain uniqueness check before the domain is inserted.
  Traces: cloudwatchomni.account-holds-at-most-one-domain.

Cause 2 of 5: CreateSpace does not enforce the one-space-per-account-per-Region rule.
  AWS: Smithy CreateSpace declares ConflictException (client, @httpError 409); the cardinality rule is recorded behavior only. The recording shows a second CreateSpace in the same account and Region is refused with 409 ConflictException and the message template "A space already exists for account <accountId> in region <region>" — fixed text "A space already exists for account " and " in region ", with the two variables taken from the caller's account id and the request's Region (the recorded run's 000000000000/us-west-2 fill the template, they are not part of it). ListSpaces for the domain still returns only the first space.
  Emulator: the second CreateSpace returns HTTP 200 with a second `space` (new spaceId and spaceArn) under the same domain, and ListSpaces then returns two spaces.
  Why: the same class of breakage as cause 1 one level down — provisioners that depend on the refusal to detect an existing space create a duplicate, and subsequent space-scoped calls (dashboards, access profiles, grants) land in a space AWS would never have created.
  Fix: CreateSpace's handler/state layer — the account+Region space uniqueness check before insert.
  Traces: cloudwatchomni.account-region-holds-at-most-one-space.

Cause 3 of 5: CreateOmniDashboard invents a name-uniqueness rule AWS does not have.
  AWS: recorded behavior only — dashboard names are NOT unique within a space. Two CreateOmniDashboard calls with the same `name` and the same `spaceId` both succeed (HTTP 200) and mint two distinct dashboardIds; ListOmniDashboards returns both under that one name, and deleting one leaves the other readable through GetOmniDashboard. Smithy: CreateOmniDashboardInput.name is required, @length 1..256, @pattern ^[a-zA-Z0-9_.@~()-]+$ — no uniqueness constraint is modeled, and the output's dashboardId is the identity.
  Emulator: the second create is refused with 409 ConflictException, message "A dashboard named '<name>' already exists in space '<spaceId>'." (the recorded run's name and space id merely fill that emulator-invented template).
  Why: a client that legitimately keeps same-named dashboards (per-team copies, a create-then-rename flow, re-create after a soft delete) is rejected by the emulator on a call AWS accepts, so the dashboard is never created and everything downstream of its dashboardId fails.
  Fix: CreateOmniDashboard's handler — remove the per-space name conflict check; identity is the generated dashboardId.
  Traces: cloudwatchomni.dashboard-names-are-not-unique-within-a-space.

Cause 4 of 5: DeleteAccessProfile refuses a profile referenced by access grants instead of cascading.
  AWS: Smithy DeleteAccessProfile is @idempotent, declares ConflictException (client, 409) but NOT ResourceNotFoundException; the cascade is recorded behavior only. The recording shows DeleteAccessProfile succeeds while an access grant names that profile as its principal (principalType ACCESS_PROFILE): the profile is then not found (404 ResourceNotFoundException, message "Access profile not found") and the grant that named it is gone too (GetAccessGrant -> 404 ResourceNotFoundException, message "AccessGrant not found"), while a grant naming a different profile is untouched.
  Emulator: the delete is refused with 409 ConflictException, message "Access profile '<profileId>' cannot be deleted while access grants reference it: <grantId>." — so the profile and its grants are undeletable through the documented path.
  Why: teardown of a space is wedged: a client following AWS's order (delete profile, grants vanish) gets a hard conflict and leaks the profile and its grants; the grant-cascade state AWS produces is never reachable.
  Fix: DeleteAccessProfile's handler/state layer — delete the grants whose principal names the profile instead of raising ConflictException.
  Traces: cloudwatchomni.deleting-an-access-profile-removes-the-grants-naming-it.

Cause 5 of 5: CreateView replayed with the same clientToken returns the stored view instead of the name conflict.
  AWS: Smithy CreateViewRequest.clientToken carries @idempotencyToken, and CreateView declares ConflictException (client, @httpError 409); the precedence is recorded behavior only. The recording shows that repeating CreateView with the SAME clientToken for a name that already exists is refused with 409 ConflictException and the message template "View already exists: <name>" (fixed text "View already exists: " plus the request's name) — the name conflict wins over idempotent replay — and the stored view stays the first request's (same arn, definition unchanged).
  Emulator: the retry returns HTTP 200 with the stored view body (name/type/description/definition/arn of the first create), i.e. it treats the repeated clientToken as an idempotent replay and never evaluates the conflict.
  Why: a client retrying a create whose first attempt actually landed gets a success where AWS reports a conflict, so it cannot distinguish "my retry was absorbed" from "the name is taken by someone else" and silently skips the conflict-handling branch.
  Fix: CreateView's handler — evaluate the view-name conflict before the clientToken idempotency short-circuit.
  Traces: cloudwatchomni.create-view-retry-with-same-client-token-is-refused-as-a-conflict.

## Evidence
Cause 1 — cloudwatchomni.account-holds-at-most-one-domain: expected (ConflictException, "A domain already exists for this account") from the second CreateDomain; got HTTP 200 and a second domain, e.g. {'domain': {'domainId': 'd-a40pv0m31tek7mp8th7892jzd', 'domainArn': 'arn:aws:cloudwatch:...:domain/d-a40pv0m31tek7mp8th7892jzd', ...}} with ResponseMetadata HTTPStatusCode 200 (ids/times are per-run values). Harness: AssertionError: expected the call to be rejected, but it succeeded.
Cause 2 — cloudwatchomni.account-region-holds-at-most-one-space: expected code ConflictException and message "A space already exists for account <accountId> in region <region>"; got HTTP 200 and a second space, e.g. {'space': {'spaceId': 'bf30d410-d0ff-4e1e-a55f-11404a574c5d', 'spaceArn': 'arn:aws:cloudwatch:...:space/...', ...}}. Harness: AssertionError: expected the call to be rejected, but it succeeded.
Cause 3 — cloudwatchomni.dashboard-names-are-not-unique-within-a-space: the check expects two 200s with distinct dashboardIds; the emulator raised botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the CreateOmniDashboard operation: A dashboard named 'sim2real-behav-cwo-dash-d59b860db7de' already exists in space '99347fc6-27fe-4e45-8f69-7bad623dbf32'. AWS returns 200 here; the 409 and that whole message are emulator-only.
Cause 4 — cloudwatchomni.deleting-an-access-profile-removes-the-grants-naming-it: the check expects DeleteAccessProfile to succeed and then GetAccessProfile -> (ResourceNotFoundException, "Access profile not found") and GetAccessGrant -> (ResourceNotFoundException, "AccessGrant not found"); the emulator raised botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the DeleteAccessProfile operation: Access profile '25597cb5-db52-4d7a-8347-e4b84f8d1d0e' cannot be deleted while access grants reference it: d787e4fe-010b-4e12-8bf9-c836c34583c7.
Cause 5 — cloudwatchomni.create-view-retry-with-same-client-token-is-refused-as-a-conflict: expected code ConflictException with message "View already exists: <name>"; got HTTP 200 returning the stored view: {'name': 'view.sim2realbehavcwo83cd04ee8949', 'type': 'USER', 'description': 'first', 'definition': 'SELECT 1', 'arn': 'arn:aws:cloudwatch:...:view/view.sim2realbehavcwo83cd04ee8949', ...} (ResponseMetadata HTTPStatusCode 200). Harness: AssertionError: expected the call to be rejected, but it succeeded.
Smithy cited: CreateDomain errors [AccessDenied 403, Conflict 409, InternalServer 500, ServiceQuotaExceeded 402, Throttling 429, Validation 400]; CreateSpace same plus ResourceNotFound 404; CreateOmniDashboardInput.name @length 1..256 @pattern ^[a-zA-Z0-9_.@~()-]+$; DeleteAccessProfile @idempotent with ConflictException 409; CreateViewRequest.clientToken @idempotencyToken, CreateView errors include ConflictException 409.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1) against it.
3. Cause 1: run check cloudwatchomni.account-holds-at-most-one-domain — CreateDomain, then CreateDomain again with a different name in the same account; the second call must be refused 409 ConflictException "A domain already exists for this account" and ListDomains must still show one domain.
4. Cause 2: run check cloudwatchomni.account-region-holds-at-most-one-space — CreateDomain, CreateSpace, then CreateSpace again in the same account/Region; the second must be refused 409 ConflictException "A space already exists for account <accountId> in region <region>" and ListSpaces must show one space.
5. Cause 3: run check cloudwatchomni.dashboard-names-are-not-unique-within-a-space — call CreateOmniDashboard twice with the same spaceId and name; both must return 200 with distinct dashboardIds and both must be listed.
6. Cause 4: run check cloudwatchomni.deleting-an-access-profile-removes-the-grants-naming-it — create two profiles, one grant per profile with principalType ACCESS_PROFILE, then DeleteAccessProfile on the first; it must succeed, the profile and its grant must then be 404 ResourceNotFoundException, and the other profile's grant must survive.
7. Cause 5: run check cloudwatchomni.create-view-retry-with-same-client-token-is-refused-as-a-conflict — CreateView with a clientToken, then repeat the identical CreateView with that same clientToken; the repeat must be refused 409 ConflictException "View already exists: <name>" and GetView must still show the first view.
