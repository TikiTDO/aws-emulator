Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.delete-webhook-absent-project-not-found
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild|DeleteWebhook|ResourceNotFoundException
Issue-Commit: 07e872c7593558b582731e73c173338530c40226

# CodeBuild DeleteWebhook refuses an existing project that has no webhook instead of succeeding as an idempotent no-op

> _Note: this issue was identified against the code base at commit `07e872c7593558b582731e73c173338530c40226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWebhook

## Description
REOPENS 20261029T175929Z-codebuild-webhook-retry-and-batch-delete-preconditions-validated.md: its fix left the call refused.

That issue's Cause 3 reordered DeleteWebhook's preconditions so an ABSENT PROJECT is now reported as 'Project does not exist' — that half is fixed and the behavioral check confirms it. What the fix left behind is the second, distinct branch it introduced ahead of the delete: a project that EXISTS but carries no webhook is still refused.

Cause 1 of 1: DeleteWebhook on an existing project with no webhook configured is refused rather than treated as an idempotent no-op.

AWS: Smithy DeleteWebhook(projectName @required ProjectName @length 2-150 @pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$) -> DeleteWebhookOutput (output models NO members); modeled errors InvalidInputException, OAuthProviderException, ResourceNotFoundException, all errorKind client, httpError 400. The model says nothing about the no-webhook case, so: recorded behavior only — the recording shows DeleteWebhook against a project that exists and has never had a webhook returning HTTP 200 with an empty body and no error, and leaving the project otherwise untouched (BatchGetProjects afterwards still reports the project with projectsNotFound empty). ResourceNotFoundException on this operation is reserved for the resource the call addresses by name — the PROJECT — which is why the absent-project case carries the wholly fixed message 'Project does not exist'. The two 'absent' cases (absent project vs. absent webhook on a real project) are separately observable in AWS and must not collapse to the same outcome.

Emulator: HTTP 400 ResourceNotFoundException with message 'Webhook does not exist for this project'. Both the error code and the message here are wholly fixed emulator text — no part of it is interpolated from the request or from state — and AWS emits no error at all on this path, so there is no AWS wording for it to match. The refusal is unconditional on the webhook field being absent, so it fires for every project whose source type cannot carry a webhook at all (e.g. a NO_SOURCE project) as well as for one whose webhook was already deleted.

Why: a client cannot distinguish 'the project is gone' from 'the project is there and simply has no webhook'. Idempotent teardown and cleanup code — the normal shape, which calls DeleteWebhook unconditionally before DeleteProject and tolerates only the project-absent error — gets a hard ResourceNotFoundException on the ordinary success path, so teardown aborts and the project leaks. A repeat DeleteWebhook after a successful one is likewise refused instead of being a no-op, so retry-on-timeout logic turns one successful delete into a failure.

Fix: precondition handling in the DeleteWebhook handler (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/delete_webhook.rs) — the webhook-absent branch must not refuse: when the project resolves and holds no webhook, the handler should return the empty DeleteWebhookOutput (and leave the project's settings, including lastModified, as they are, since nothing changed). The absent-PROJECT branch ahead of it, and its 'Project does not exist' wording, stay exactly as they are. The webhook_not_found() helper and crate::state::WEBHOOK_NOT_FOUND_MESSAGE are then unused on this path.

Traces: behavioral check codebuild.delete-webhook-absent-project-not-found.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 65/182 passed; this check reported verdict [error].

Check codebuild.delete-webhook-absent-project-not-found (GROUP error-semantics, SERVICES [codebuild]):
- First phase PASSED: DeleteWebhook(projectName=<name never created>) was refused with code ResourceNotFoundException and message exactly 'Project does not exist', as the check asserts.
- Second phase FAILED: the check then creates a minimal NO_SOURCE/NO_ARTIFACTS project, confirms via BatchGetProjects that projectsNotFound == [] and that the returned project has no 'webhook' member, and calls DeleteWebhook(projectName=<that project>) expecting it to succeed. The emulator raised instead:

  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteWebhook operation: Webhook does not exist for this project

  Expected: HTTP 200, empty DeleteWebhookOutput, no error; project left intact (the check's follow-up BatchGetProjects assertions never ran).
  Actual:   HTTP 400, __type ResourceNotFoundException, message 'Webhook does not exist for this project' (fixed text, no request- or state-derived parts).

Source of the refusal in this checkout: services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/delete_webhook.rs lines 175-184 — `let Some(webhook) = stored.project.webhook.as_ref() else { ... return Err(refuse(..., webhook_not_found())) }` — and the message constant at lines 272-277 / crate::state::WEBHOOK_NOT_FOUND_MESSAGE.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — DeleteWebhook input {projectName @required ProjectName, length 2..150, pattern ^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$}, output DeleteWebhookOutput with no members, errors InvalidInputException / OAuthProviderException / ResourceNotFoundException (all client, httpError 400, defaulted).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Create an IAM role trusted by codebuild.amazonaws.com.
3. CreateProject with name=<any valid project name>, source={type: NO_SOURCE, buildspec: 'version: 0.2\nphases:\n  build:\n    commands:\n      - echo hi\n'}, artifacts={type: NO_ARTIFACTS}, environment={type: LINUX_CONTAINER, image: aws/codebuild/amazonlinux2-x86_64-standard:5.0, computeType: BUILD_GENERAL1_SMALL}, serviceRole=<that role ARN>. No webhook is ever created on it.
4. BatchGetProjects(names=[<that project>]) — confirm projectsNotFound == [] and that the returned project carries no 'webhook' member.
5. Call DeleteWebhook(projectName=<that project>). Expect HTTP 200 with an empty body and no error. Observe HTTP 400 ResourceNotFoundException with message 'Webhook does not exist for this project'.
6. Call DeleteWebhook(projectName=<a name that was never created>) — this still correctly answers 400 ResourceNotFoundException 'Project does not exist', showing the two absent cases must stay distinguishable.
7. Replay the behavioral check codebuild.delete-webhook-absent-project-not-found to confirm the fix.
