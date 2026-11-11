Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatchomni.access-profile-name-is-unique-within-a-space cloudwatchomni.view-name-is-reusable-only-after-the-view-is-deleted cloudwatchomni.view-definition-must-be-exactly-one-select-statement cloudwatchomni.domain-with-a-space-cannot-be-deleted cloudwatchomni.domain-name-pattern-and-identity-center-rules-reject-updates
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni.access-profile-name-is-unique-within-a-space|run#8|AssertionError|assert (code, message) == ("ConflictException", "An access profile with that name already exists in this space"), (
Issue-Key: cloudwatchomni.view-name-is-reusable-only-after-the-view-is-deleted|run#7|AssertionError|assert message == f"View already exists: {name}", f"unexpected message {message!r}"
Issue-Key: cloudwatchomni.view-definition-must-be-exactly-one-select-statement|run#7|AssertionError|assert (code, message) == ("ValidationException", "View definition must be a SELECT query"), (
Issue-Key: cloudwatchomni.domain-with-a-space-cannot-be-deleted|run#6|AssertionError|assert message == (
Issue-Key: cloudwatchomni.domain-name-pattern-and-identity-center-rules-reject-updates|run#6|AssertionError|assert message == (
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# cloudwatchomni: conflict and validation error messages diverge from AWS wording (CreateAccessProfile, CreateView, DeleteDomain, UpdateDomain)

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessProfile

## Description
FIVE net-new root causes in the cloudwatchomni emulator, each an error whose HTTP status and error code are right but whose `message` is text AWS never emits. The error message is part of the contract, so each of these needs its own message template fixed; close all five before submitting.

Related but NOT covering these: open issue 20261109T172919Z-cloudwatchomni-validationexception-message-echoes-value-and-unordered-enum.md reports the same "Value '<input>' at" splice for CreateDomain.name (@length) and CreateAccessGrant.permission (enum); it does not carry the UpdateDomain pattern-constraint case of cause 5 below, which additionally renders the regex with anchors. Open issue 20261109T172919Z-cloudwatchomni-resourcenotfoundexception-messages-do-not-match-aws-wording.md concerns ResourceNotFoundException only, not the conflict/validation messages here. The other open cloudwatchomni issues are unimplemented-stub reports (CreateIntegration, ListDomains).

Cause 1 of 5: CreateAccessProfile duplicate-name conflict message.
  AWS: Smithy CreateAccessProfile declares ConflictException (errorKind client, @httpError 409) and CreateAccessProfileInput.name is required, @length 1..256, @pattern ^[a-zA-Z0-9 _.:/=+@-]+$; the message text is recorded behavior only. A second profile under a name already taken in the space is refused with 409 ConflictException and the message "An access profile with that name already exists in this space" — entirely fixed text, with NO variable part: neither the name nor the spaceId appears.
  Emulator: "An access profile named '<name>' already exists in space '<spaceId>'." — wrong fixed text plus two interpolated request values AWS withholds (the recorded run's 'sim2real-behav-cwo-prof-b98f45b7d422' / '509bdd89-...' only fill that emulator template).
  Why: a client matching on the conflict message (contract tests, log scrapers, text surfaced to users) sees wording AWS never produces, and caller-supplied input is reflected back in an error AWS keeps opaque.
  Fix: CreateAccessProfile's handler — the duplicate-name ConflictException message construction.
  Traces: cloudwatchomni.access-profile-name-is-unique-within-a-space.

Cause 2 of 5: CreateView name-conflict message.
  AWS: Smithy CreateView declares ConflictException (client, @httpError 409); message text is recorded behavior only. A second CreateView under a live view's name is refused with 409 ConflictException and the message template "View already exists: <name>" — fixed text "View already exists: " followed by the request's `name` verbatim, with no quoting and no trailing period.
  Emulator: "A view named '<name>' already exists." — wrong fixed text, the name quoted, and a trailing period.
  Why: same class as cause 1; it also makes the conflict message inconsistent with the sibling not-found template "View not found: <name>" that AWS uses for GetView.
  Fix: CreateView's handler — the name-conflict ConflictException message construction.
  Traces: cloudwatchomni.view-name-is-reusable-only-after-the-view-is-deleted.

Cause 3 of 5: CreateView definition validation leaks the SQL parser's internal diagnostic.
  AWS: Smithy CreateViewRequest.definition is required, @length 1..10000, target ViewDefinition, and CreateView declares ValidationException (client, @httpError 400); the texts are recorded behavior only. A definition that is not a SELECT is refused with 400 ValidationException and the constant message "View definition must be a SELECT query"; a definition holding more than one statement is refused with 400 ValidationException and the constant message "View definition must contain exactly one SQL statement". Both are wholly fixed text — no echo of the submitted SQL, no parser position.
  Emulator: for "DROP TABLE default" it answers ValidationException with "Invalid view definition: sql parser error: Expected: SELECT, VALUES, or a subquery in the query body, found: DROP at Line: 1, Column: 1" — the underlying parser's error, with a token and line/column AWS never includes. (The multi-statement case was not reached because the check stops at the first mismatch; it needs its own constant message too.)
  Why: the client cannot classify the refusal — it gets an implementation-specific parser string instead of AWS's two stable sentences — and the message discloses parser internals that change with the parser version.
  Fix: CreateView's definition validation — classify the definition (not-a-SELECT vs more-than-one-statement) and emit AWS's two constant messages instead of propagating the parser diagnostic.
  Traces: cloudwatchomni.view-definition-must-be-exactly-one-select-statement.

Cause 4 of 5: DeleteDomain non-empty-domain conflict message.
  AWS: Smithy DeleteDomain is @idempotent and declares ConflictException (client, @httpError 409); message text is recorded behavior only. While the domain still holds spaces the delete is refused 409 ConflictException with the template "Cannot delete Domain <domainId> because it has <n> associated Spaces. Delete all Spaces first." — fixed text "Cannot delete Domain ", " because it has ", " associated Spaces. Delete all Spaces first.", with the request's domainId and the current count of spaces in that domain as the two variables (the count is taken from state; the recorded run's 1 is not part of the template). The domain stays readable with status ACTIVE.
  Emulator: "Domain '<domainId>' still contains spaces and cannot be deleted." — wrong fixed text, the domainId quoted, and the space count omitted entirely.
  Why: a client that parses the count out of the message to report how many spaces remain has nothing to parse, and any exact-match assertion on the refusal fails.
  Fix: DeleteDomain's handler — the non-empty-domain ConflictException message construction (must include the associated-space count from state).
  Traces: cloudwatchomni.domain-with-a-space-cannot-be-deleted.

Cause 5 of 5: UpdateDomain @pattern validation message splices the value and renders the regex with anchors.
  AWS: Smithy UpdateDomainInput.name targets String with @length 3..63 and @pattern ^[a-z0-9]+(-[a-z0-9]+)*$; UpdateDomain declares ValidationException (client, @httpError 400). The message template is recorded behavior only: "<n> validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" where the variables are the error count, the member name and the model's pattern rendered WITHOUT the ^ and $ anchors — for this member, "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]+(-[a-z0-9]+)*". The rejected value is NOT part of the template.
  Emulator: "1 validation error detected: Value 'Sim2Real-Behav-Upper' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]+(-[a-z0-9]+)*$" — two divergences: it inserts "Value '<submitted value>' at" where AWS has "Value at" (the recorded 'Sim2Real-Behav-Upper' is the run's input; the defect is that any value is interpolated), and it prints the anchored model pattern instead of stripping ^/$. Status 400 and code ValidationException are correct.
  Why: exact-match comparisons on validation messages fail on both counts, and a client that compiles the quoted pattern to pre-validate input gets a different regex than AWS advertises. Note the same operation's second refusal — switching identityProviders to ["IDC"] with no Identity Center instance, which AWS answers 400 ValidationException "identityCenterInstanceArn is required when authorizationProviders includes IDC" — is not yet reachable because the check stops at this first mismatch.
  Fix: the shared constraint-violation message formatter reached from UpdateDomain's input validation (drop the value clause; strip the regex anchors). See the related open issue named above, which reports the value-clause half for other members/constraints.
  Traces: cloudwatchomni.domain-name-pattern-and-identity-center-rules-reject-updates.

## Evidence
Cause 1 — cloudwatchomni.access-profile-name-is-unique-within-a-space: expected ("ConflictException", "An access profile with that name already exists in this space"); got ConflictException: "An access profile named 'sim2real-behav-cwo-prof-b98f45b7d422' already exists in space '509bdd89-4776-41c7-aa54-4c1bf9f0b286'." Code/status match; only $.message diverges. Fixed text is the whole AWS sentence — the emulator's name and space id are request/state values that do not belong in it.
Cause 2 — cloudwatchomni.view-name-is-reusable-only-after-the-view-is-deleted: expected code ConflictException and message "View already exists: <name>"; got "A view named 'view.sim2realbehavcwo089634325174' already exists." The name is the request variable; the surrounding words, the quoting and the trailing period are the divergence.
Cause 3 — cloudwatchomni.view-definition-must-be-exactly-one-select-statement: CreateView(definition="DROP TABLE default") expected ("ValidationException", "View definition must be a SELECT query"); got ValidationException: 'Invalid view definition: sql parser error: Expected: SELECT, VALUES, or a subquery in the query body, found: DROP at Line: 1, Column: 1'. The second assertion (definition="SELECT 1; SELECT 2" -> "View definition must contain exactly one SQL statement") is still unverified because the check aborted at the first.
Cause 4 — cloudwatchomni.domain-with-a-space-cannot-be-deleted: expected code ConflictException and message "Cannot delete Domain <domainId> because it has 1 associated Spaces. Delete all Spaces first."; got "Domain 'd-1naa6qgdhi2sun9oyzl7cpkfk' still contains spaces and cannot be deleted." The domainId is the request variable and the count comes from state; everything else is fixed text the emulator does not reproduce.
Cause 5 — cloudwatchomni.domain-name-pattern-and-identity-center-rules-reject-updates: UpdateDomain(name="Sim2Real-Behav-Upper") expected ValidationException with "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]+(-[a-z0-9]+)*"; got "1 validation error detected: Value 'Sim2Real-Behav-Upper' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9]+(-[a-z0-9]+)*$". Smithy: UpdateDomainInput.name @length 3..63 @pattern ^[a-z0-9]+(-[a-z0-9]+)*$, ValidationException @httpError 400.
Smithy cited throughout: ConflictException errorKind client @httpError 409 and ValidationException errorKind client @httpError 400 are declared on CreateAccessProfile, CreateView, DeleteDomain and UpdateDomain; neither shape fixes its message text, so the AWS wording above is recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1) against it.
3. Cause 1: run check cloudwatchomni.access-profile-name-is-unique-within-a-space — create a domain, a space, an access profile, then CreateAccessProfile again with the same name in that space; the refusal must be 409 ConflictException with message "An access profile with that name already exists in this space".
4. Cause 2: run check cloudwatchomni.view-name-is-reusable-only-after-the-view-is-deleted — CreateView, then CreateView again under the same name; the refusal must be 409 ConflictException with message "View already exists: <name>".
5. Cause 3: run check cloudwatchomni.view-definition-must-be-exactly-one-select-statement — CreateView with definition "DROP TABLE default" must be 400 ValidationException "View definition must be a SELECT query", and with definition "SELECT 1; SELECT 2" must be 400 ValidationException "View definition must contain exactly one SQL statement"; neither may leak a parser diagnostic.
6. Cause 4: run check cloudwatchomni.domain-with-a-space-cannot-be-deleted — create a domain and a space in it, then DeleteDomain; the refusal must be 409 ConflictException with message "Cannot delete Domain <domainId> because it has <n> associated Spaces. Delete all Spaces first." and GetDomain must still report status ACTIVE.
7. Cause 5: run check cloudwatchomni.domain-name-pattern-and-identity-center-rules-reject-updates — UpdateDomain with an uppercase name must be 400 ValidationException "1 validation error detected: Value at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9]+(-[a-z0-9]+)*" (no value clause, no regex anchors), and UpdateDomain with identityProviders=["IDC"] and no Identity Center instance must be 400 ValidationException "identityCenterInstanceArn is required when authorizationProviders includes IDC".
