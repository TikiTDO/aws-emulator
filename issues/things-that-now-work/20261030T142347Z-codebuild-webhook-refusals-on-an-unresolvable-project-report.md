Issue-Source: trace-sweep-divergence
Issue-Trace: codebuild-project-registry-and-absent-project-errors
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codebuild-project-registry-and-absent-project-errors
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|UpdateWebhook|4xx->4xx|$.message
Issue-Key: codebuild|DeleteWebhook|4xx->4xx|$.message
Issue-Commit: 6d14a9a831b2c017b564e34b5138bca4659f1a33

# CodeBuild webhook refusals on an unresolvable project report the webhook-absent message instead of ResourceNotFoundException 'Project does not exist' (2 causes)

> _Note: this issue was identified against the code base at commit `6d14a9a831b2c017b564e34b5138bca4659f1a33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWebhook

## Description
On the webhook mutators (UpdateWebhook, DeleteWebhook) the emulator refuses a request whose projectName does not resolve to a project with its own fixed webhook-absent text instead of the project-not-found refusal AWS gives. The HTTP status (400) and the modeled error code (ResourceNotFoundException) are right; the message — which is part of the contract for this error class — is wrong, and it misattributes the fault to a webhook rather than to the project the call addresses by name. TWO causes are bundled here (one per operation); both must be closed before this issue is resolved.

Cause 1 of 2 — UpdateWebhook against a project that does not exist (fixture codebuild-project-registry-and-absent-project-errors, step task#11).
AWS: UpdateWebhook(projectName @required ProjectName, @length 2..150, @pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$) resolves the named project FIRST and, when it names no project, fails with HTTP 400 and __type ResourceNotFoundException. Smithy (services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json): UpdateWebhook -> UpdateWebhookOutput{webhook}; errors InvalidInputException, OAuthProviderException, ResourceNotFoundException, all errorKind client, httpError 400 (defaulted). The message text is recorded behavior only: the recording shows the message 'Project does not exist' — wholly fixed text for this error class, with NO part interpolated from the request or from state (the project name/ARN the caller sent does not appear in it). Sibling operations in the same recording use the same fixed wording for the same cause (UpdateProject, CreateWebhook, DeleteWebhook on the same absent project).
Emulator: HTTP 400 with __type ResourceNotFoundException (correct) but message 'Webhook does not exist for this project' — again wholly fixed emulator text, nothing interpolated. It is produced by the webhook-absent branch, i.e. a webhook precondition is evaluated and reported for a project the handler should have refused to resolve, so the refusal names the wrong resource. (In this fixture the project is addressable in emulator state only because CreateProject at step task#6 wrongly accepted an unassumable service role; that acceptance is a separate, already-filed defect — see Related — but the wording emitted here is this handler's own, and the handler must report the project it could not legitimately resolve.)
Why: clients branch on the refusal's message as well as its code when both absent cases share one code. A caller told 'Webhook does not exist for this project' concludes the project is fine and only the webhook is missing, so create-project-then-update-webhook and reconciliation flows retry the webhook update forever instead of (re)creating the project, and negative-path tests asserting the AWS wording pass here and fail against AWS.
Fix: precondition ordering/message selection in the UpdateWebhook handler (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/update_webhook.rs) — resolve the project first and raise ResourceNotFoundException with the project-not-found wording before any webhook-presence check; reserve the webhook-absent branch for a project that genuinely resolves.
Traces: codebuild-project-registry-and-absent-project-errors#task#11.

Cause 2 of 2 — DeleteWebhook against the same unresolvable project (same fixture, step task#12).
AWS: DeleteWebhook(projectName @required ProjectName, same length/pattern constraints) -> DeleteWebhookOutput (no members); errors InvalidInputException, OAuthProviderException, ResourceNotFoundException, all client, httpError 400 (defaulted). Recorded behavior only: HTTP 400, __type ResourceNotFoundException, message 'Project does not exist' — fixed text, no request- or state-derived parts.
Emulator: HTTP 400, __type ResourceNotFoundException, message 'Webhook does not exist for this project' (fixed text), i.e. the same webhook-absent branch fires ahead of / in place of the project refusal.
Why: idempotent teardown that calls DeleteWebhook before DeleteProject and tolerates only the project-absent refusal sees an unrecognised webhook error and aborts, leaving the project behind; and a caller cannot tell 'the project is gone' from 'the project is there without a webhook', which AWS keeps separately observable.
Fix: precondition ordering/message selection in the DeleteWebhook handler (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/delete_webhook.rs) — the project-not-found refusal must win over the webhook-presence branch for any project that does not resolve.
Traces: codebuild-project-registry-and-absent-project-errors#task#12.

Related (does NOT cover these two causes — neither open issue carries a replay key for these fixture steps):
- 20261023T134241Z-codebuild-absent-codebuild-resources-accepted-instead-of-modeled-error.md bundles the upstream CreateProject acceptance (task#6) and the CreateWebhook/UpdateProject/UpdateProjectVisibility refusals on the same absent project (task#6/#7/#9/#10), but not UpdateWebhook (task#11) or DeleteWebhook (task#12).
- 20261030T140529Z-codebuild-deletewebhook-refuses-an-existing-project-that-has.md is a behavioral issue about DeleteWebhook on a project that DOES exist and has no webhook (it must be an idempotent no-op). It is the complementary branch and carries no wire replay key; the requirement here is the wording of the UNRESOLVABLE-project refusal.
- 20261029T175929Z-codebuild-absent-resource-and-invalid-input-error-messages.md collects other absent-resource wording divergences but lists neither of these steps.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 106 steps, 74 passed, score 69.8%, 20 divergence clusters). These two clusters report owned_by = [] — no open issue carries their replay key.

Cause 1 — UpdateWebhook @ codebuild-project-registry-and-absent-project-errors#task#11 (divergence_key service=codebuild|UpdateWebhook|status=400->400|keys=$.message):
  expected: HTTP 400, $.__type 'ResourceNotFoundException', $.message 'Project does not exist'
  actual:   HTTP 400, $.__type 'ResourceNotFoundException', $.message 'Webhook does not exist for this project'
  detail:   "$.message: value_mismatch (expected 'Project does not exist', got 'Webhook does not exist for this project')"
  Both messages are entirely fixed text for their error class; no segment of either is taken from the request (the project name sim2real-probe-codebuild-codebuild-absent-proj-74c4d814 appears in neither) or from emulator state.

Cause 2 — DeleteWebhook @ codebuild-project-registry-and-absent-project-errors#task#12 (divergence_key service=codebuild|DeleteWebhook|status=400->400|keys=$.message):
  expected: HTTP 400, $.__type 'ResourceNotFoundException', $.message 'Project does not exist'
  actual:   HTTP 400, $.__type 'ResourceNotFoundException', $.message 'Webhook does not exist for this project'
  detail:   "$.message: value_mismatch (expected 'Project does not exist', got 'Webhook does not exist for this project')"

Context from the same fixture (already-filed, cited only to explain why the emulator can resolve the project at all): step task#6 CreateProject answered 200 instead of 400 InvalidInputException, so the project the later steps address exists in emulator state; steps task#7/#9/#10 and task#14 likewise diverge and are owned by 20261023T134241Z-... / 20261029T175929Z-....

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — UpdateWebhook input {projectName @required ProjectName length 2..150 pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$, branchFilter, rotateSecret, filterGroups, buildType, pullRequestBuildPolicy}, output {webhook}; DeleteWebhook input {projectName @required ProjectName, same constraints}, output DeleteWebhookOutput with no members; both carry modeled errors InvalidInputException / OAuthProviderException / ResourceNotFoundException, all errorKind client with httpError 400 defaulted. The model does not fix the message text, so the wording requirement is taken from the recording.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1 (representative): fixture codebuild-project-registry-and-absent-project-errors, step task#11 — call UpdateWebhook with projectName naming a project that does not legitimately exist (in the fixture it is the project whose CreateProject at task#6 should have been refused). Expect HTTP 400, __type ResourceNotFoundException, message 'Project does not exist'. Observe HTTP 400, __type ResourceNotFoundException, message 'Webhook does not exist for this project'.
4. Cause 2: same fixture, step task#12 — call DeleteWebhook with the same projectName. Expect HTTP 400 / ResourceNotFoundException / 'Project does not exist'; observe HTTP 400 / ResourceNotFoundException / 'Webhook does not exist for this project'.
5. Re-replay both fixture steps (codebuild-project-registry-and-absent-project-errors#task#11 and #task#12) to confirm the fix; the wording of the absent-project refusal must match the sibling operations in the same fixture (UpdateProject, CreateWebhook).
