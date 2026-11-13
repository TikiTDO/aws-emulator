Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms.create-analysis-template-duplicate-name-conflict
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms.create-analysis-template-duplicate-name-conflict|run#13|AssertionError|assert message.startswith("The ") and message.endswith(
Issue-Commit: e9e518e3fed0eb72155b08c250942d32d0463cfb

# CreateAnalysisTemplate duplicate-name ConflictException message is emulator-worded instead of AWS's "The ...: with membership id: <membershipId> already exists."

> _Note: this issue was identified against the code base at commit `e9e518e3fed0eb72155b08c250942d32d0463cfb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAnalysisTemplate

## Description
One cause.

Cause 1 of 1: CreateAnalysisTemplate's duplicate-name refusal carries an emulator-invented message instead of AWS's wording for the "resource already exists" conflict class. Failing check id: cleanrooms.create-analysis-template-duplicate-name-conflict.

AWS: Smithy (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/smithy-model/com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda.json) models CreateAnalysisTemplate with error ConflictException (errorKind client, httpError 409), documented "Updating or deleting a resource can cause an inconsistent state"; `name` targets TableAlias (@length max 128) and `membershipIdentifier` targets MembershipIdentifier (36-char UUID). The model does not state the message text, so the message is recorded behavior only: the recording shows AWS rejects the second create under a name already taken in the same membership with ConflictException whose message begins with the fixed text `The ` and ends with the fixed text `: with membership id: ` + the membership id from the request + the fixed text ` already exists.` — i.e. a template of the form `The <resource phrase naming the conflicting analysis template, which contains a server-chosen id>: with membership id: <membershipIdentifier> already exists.` Fixed text in that template: the leading `The `, the literal `: with membership id: `, and the trailing ` already exists.`. Variable parts: the conflicting template's id (taken from emulator state) and the membership id (taken from the request's membershipIdentifier path label). The ids/names in any one recording (e.g. template c5850309-e351-42b4-b930-a566e499404f, membership 8376fc54-dc6c-4980-a4d3-346d54f08b52, name sim2real_cr_tpl_edd2cd3a6b7a) are per-run values filling that template, not part of it.

Emulator: returns the right error code (ConflictException) and status (409) but the message is `analysis template <existing id> already uses name <name> in membership <membership id>` — observed as `analysis template c5850309-e351-42b4-b930-a566e499404f already uses name sim2real_cr_tpl_edd2cd3a6b7a in membership 8376fc54-dc6c-4980-a4d3-346d54f08b52`. It neither begins with `The ` nor ends with `: with membership id: <membershipIdentifier> already exists.`, so the check's prefix/suffix assertion fails.

Why: a client that parses or matches on the conflict message — log scrapers, retry/idempotency logic that distinguishes "already exists" from other conflicts, and tests asserting AWS's documented refusal text — sees a message shaped nothing like AWS's. The membership id is not in the position AWS puts it, so extraction of the membership from the refusal fails, and operators comparing emulator logs against AWS logs see a different refusal sentence for the same condition.

Why this is net-new: the open issue 20261102T062702Z-cleanrooms-deleteanalysistemplate-not-found-refusal-is-emulator-worded-instead.md covers DeleteAnalysisTemplate's ResourceNotFoundException wording, and 20261029T213946Z-cleanrooms-refusal-fidelity-duplicate-analysis-rule-classified-conflictexception.md covers analysis-rule/membership/privacy-budget-template duplicate refusals; neither carries CreateAnalysisTemplate's duplicate-name ConflictException message. The suite marked this check neither ALREADY FILED nor OWNED.

Fix: belongs in the CreateAnalysisTemplate handler's refusal construction — the duplicate-name branch of `claim()` in services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/create_analysis_template.rs, which builds the ConflictException message inline. The message template should be shared with the service's other "already exists" conflicts (crate::state message helpers, alongside st::CONFLICT_REASON_ALREADY_EXISTS) so the fixed text is written once and only the ids/names vary.

## Evidence
Check: cleanrooms.create-analysis-template-duplicate-name-conflict (verdict: mismatch).

Expected (recorded AWS): ConflictException, HTTP 409, message matching `The <phrase naming the existing analysis template incl. its id>: with membership id: <membershipIdentifier> already exists.` — assertion: message.startswith("The ") and message.endswith(f": with membership id: {membership_id} already exists.").

Actual (emulator): ConflictException, HTTP 409, message `analysis template c5850309-e351-42b4-b930-a566e499404f already uses name sim2real_cr_tpl_edd2cd3a6b7a in membership 8376fc54-dc6c-4980-a4d3-346d54f08b52`.

Replay output:
  File "check_cleanrooms_create_analysis_template_duplicate_name_conflict.py", line 56, in run
    assert message.startswith("The ") and message.endswith(
  AssertionError: expected the duplicate-template conflict naming membership 8376fc54-dc6c-4980-a4d3-346d54f08b52, got 'analysis template c5850309-e351-42b4-b930-a566e499404f already uses name sim2real_cr_tpl_edd2cd3a6b7a in membership 8376fc54-dc6c-4980-a4d3-346d54f08b52'

Source of the emulator message: services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/create_analysis_template.rs lines 203-219 (`claim()`, duplicate-name branch):
  message: Some(format!("analysis template {} already uses name {} in membership {}", existing.id, existing.name, membership.id))

The error code, HTTP status and the rest of the check (only the original template is listed; a differently-named template still succeeds) are correct — only the message text diverges.

## Steps to Reproduce
1. CreateCollaboration (members=[], creatorMemberAbilities=[CAN_QUERY, CAN_RECEIVE_RESULTS], creatorDisplayName="creator", queryLogStatus=DISABLED) and note the collaboration id.
2. CreateMembership(collaborationIdentifier=<collab id>, queryLogStatus=DISABLED) and note the membership id.
3. CreateAnalysisTemplate(membershipIdentifier=<membership id>, name=<N>, format=SQL, source={text: "SELECT 1 AS one"}) — succeeds.
4. CreateAnalysisTemplate again with the SAME membershipIdentifier and the SAME name=<N> (any source text).
5. Observe the refusal: code is ConflictException at HTTP 409 (correct), but the message is `analysis template <id> already uses name <N> in membership <membership id>` rather than AWS's `The ...<id>...: with membership id: <membership id> already exists.`

Replay the whole check with: cleanrooms.create-analysis-template-duplicate-name-conflict
