Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied|_until_allowed#24|Timeout|time.sleep(3)
Issue-Commit: 6c2a412138b2fb0170715c3d0a682813a233dd77

# iam: the iam:PolicyARN condition key is never put in the request context, so a delegated Attach/DetachUserPolicy scoped by ArnEquals iam:PolicyARN is refused AccessDenied even for the policy it allows

> _Note: this issue was identified against the code base at commit `6c2a412138b2fb0170715c3d0a682813a233dd77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachUserPolicy

## Description
One cause.

Cause 1 of 1: IAM's own authorization gate never populates the service-specific condition key `iam:PolicyARN`, so any identity-based statement whose grant is conditioned on it can never be satisfied and the grant is withheld. Failing check: iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied.

AWS: `iam:PolicyARN` is a service-specific condition key AWS places in the request context of the managed-policy attachment calls — AttachUserPolicy, DetachUserPolicy and their role/group peers — carrying the value of that request's `PolicyArn` parameter. A delegated administrator whose identity policy allows `iam:AttachUserPolicy`/`iam:DetachUserPolicy` on a target user under `Condition: {"ArnEquals": {"iam:PolicyARN": "<allowed-policy-arn>"}}` is therefore ALLOWED when the call names that policy ARN, and refused when it names any other. Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508 model, AttachUserPolicy / DetachUserPolicy): input `PolicyArn` (arnType, @length 20..2048) is required, `UserName` (userNameType, @length 1..64, pattern ^[\w+=,.@-]+$) is required; the modeled errors are InvalidInputException (400), LimitExceededException (409), NoSuchEntityException (404), PolicyNotAttachableException (400), ServiceFailureException (500). The model does not enumerate condition keys, so the condition-key semantics above are AWS-documented behavior, not model text. The refusal AWS returns for the NON-matching policy ARN is HTTP 403 with error code `AccessDenied` and the message whose fixed text is "User: " … " is not authorized to perform: " … " on resource: user " … " because no identity-based policy allows the " … " action"; its variable parts are the caller's ARN (the delegate user's ARN from the signing key), the action (`iam:AttachUserPolicy` or `iam:DetachUserPolicy`, as the request asked), and the target user's friendly NAME as the request carried it. No value in that message is a literal from any one recording.

Emulator: the allowed call is refused as well, permanently. services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/attach_user_policy.rs calls `crate::state::authorization::authorize_named(&store, ctx, ATTACH_USER_POLICY, &resource, &format!("user {}", input.user_name))` and hands it ONLY the action, the target ARN and the display name — the request's `PolicyArn` is never passed. The gate assembles the condition context in services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state/authorization.rs `request_context`, which fills only the signing principal's name/unique id/`aws:PrincipalTag/*`, the target's `aws:ResourceTag/*` and `iam:ResourceTag/*`, the written-tag keys for tagging calls, and the clock (`aws:CurrentTime`/`aws:EpochTime`); `iam:PolicyARN` is in neither that context nor `PUBLISHED_ABSENT_KEYS`. By that module's own documented fail-closed rule — "A clause over any OTHER key is left undecided, which withholds a grant and keeps a refusal" — the delegate's `OnlyThisPolicy` statement never grants, so BOTH the attach of the allowed policy and the detach of it are answered 403 AccessDenied. DetachUserPolicy has the same shape. Because the check retries only the two propagation codes (InvalidClientTokenId, AccessDenied) while waiting for the call AWS allows, it never gets a verdict and the run is killed at the harness cap: "Error: Check timed out after 60.0s".

Why: a client relying on AWS's behavior uses `iam:PolicyARN` exactly to build least-privilege delegated administration — "this delegate may attach/detach only this one managed policy". Against the emulator that delegation grants nothing at all: every attach and every detach by the delegate is AccessDenied, so the delegated-admin path appears broken rather than scoped, and any test that asserts the allowed ARN works and the other ARN is refused cannot distinguish a correctly-scoped emulator from one that denies everything. The divergence is over-restrictive on the allowed ARN while the scoping it is supposed to express is unenforceable, so it hides both outcomes.

Fix: the IAM authorization gate's condition-context assembly and the managed-policy attachment handlers — publish and populate `iam:PolicyARN` from the request's `PolicyArn` parameter at the AttachUserPolicy / DetachUserPolicy (and AttachRolePolicy / DetachRolePolicy / AttachGroupPolicy / DetachGroupPolicy) call sites so the shared evaluation can decide an `ArnEquals`/`ArnLike` clause over it, and declare it to the evaluation as a key the gate can answer for so IAM's absent-key rule applies instead of leaving the clause undecided. Related but NOT covering this: open issue 20261029T192356Z-iam-9-net-new-authorization-and-policy-evaluation.md collects other IAM authorization/policy-evaluation causes; it does not carry this check or the iam:PolicyARN context key.

Traces: iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied (code citations: src/ops/attach_user_policy.rs lines 204-221; src/state/authorization.rs lines 44-75, 252, 325-362, 392-438).

## Evidence
Failing check iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied, verdict [mismatch], detail: "Error: Check timed out after 60.0s" (no traceback — the check never reached an assertion).

Expected (AWS): with the delegate's inline policy allowing iam:AttachUserPolicy/iam:DetachUserPolicy on the target user's ARN under Condition {"ArnEquals": {"iam:PolicyARN": <p_ok>}}:
  - delegate AttachUserPolicy(UserName=<target>, PolicyArn=<p_ok>) -> HTTP 200, and ListAttachedUserPolicies(<target>) reports exactly [<p_ok>]
  - delegate AttachUserPolicy(UserName=<target>, PolicyArn=<p_other>) -> HTTP 403 AccessDenied, message "User: <delegate-arn> is not authorized to perform: iam:AttachUserPolicy on resource: user <target-name> because no identity-based policy allows the iam:AttachUserPolicy action", and the target's attachments unchanged
  - delegate DetachUserPolicy(UserName=<target>, PolicyArn=<p_ok>) -> HTTP 200
  - delegate DetachUserPolicy(UserName=<target>, PolicyArn=<p_other>) -> HTTP 403 AccessDenied with the same message template and the action iam:DetachUserPolicy, the policy staying attached

Actual (emulator): the ALLOWED attach is refused too. The check's _until_allowed helper retries only InvalidClientTokenId/AccessDenied while waiting for that call to be permitted, and the check is killed at the 60 s cap, so no allow/deny verdict is ever produced.

Source evidence for the cause:
- services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/attach_user_policy.rs:215 — authorize_named(&store, ctx, ATTACH_USER_POLICY, &resource, &format!("user {}", input.user_name)) — input.policy_arn is never handed to the gate.
- services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state/authorization.rs:325 request_context() — builds the context from principal name/id/tags, resource tags, written tags and the clock only; no iam:PolicyARN entry.
- same file:252 PUBLISHED_ABSENT_KEYS — lists aws:MultiFactorAuthPresent, aws:MultiFactorAuthAge, aws:TokenIssueTime, aws:SourceIdentity and peers; iam:PolicyARN is absent from it as well.
- same file module docs lines 66-75: "A clause over any OTHER key is left undecided, which withholds a grant and keeps a refusal." -> the delegate's conditioned Allow can never grant.

## Steps to Reproduce
1. Create an IAM user TARGET and two customer-managed policies P_OK and P_OTHER (any valid document).
2. Create an IAM user DELEGATE, create a long-term access key for it, and put an inline policy on it with two statements: Allow iam:GetUser on DELEGATE's own ARN, and Allow [iam:AttachUserPolicy, iam:DetachUserPolicy] on TARGET's ARN with Condition {"ArnEquals": {"iam:PolicyARN": "<P_OK arn>"}}.
3. Using DELEGATE's access key, call AttachUserPolicy(UserName=TARGET, PolicyArn=<P_OK arn>).
   Expected: HTTP 200, and ListAttachedUserPolicies(TARGET) == [P_OK].
   Observed: HTTP 403 AccessDenied, repeatedly, because the ArnEquals clause over iam:PolicyARN is never decidable; the behavioral check retries this call until the 60 s cap and times out.
4. Using DELEGATE's access key, call AttachUserPolicy(UserName=TARGET, PolicyArn=<P_OTHER arn>).
   Expected: HTTP 403 AccessDenied with message "User: <DELEGATE arn> is not authorized to perform: iam:AttachUserPolicy on resource: user TARGET because no identity-based policy allows the iam:AttachUserPolicy action", TARGET's attachments unchanged.
5. Repeat 3 and 4 with DetachUserPolicy (after the admin attaches P_OTHER itself): the P_OK detach must be allowed and the P_OTHER detach refused with the same message template naming iam:DetachUserPolicy.
6. Replay: iam.policy-arn-condition-scopes-delegated-attach-other-policy-denied
