Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail-data.put-audit-events-requires-the-cloudtrail-data-action-not-cloudtrail-wildcard
Issue-Shape-Id: com.amazonaws.cloudtraildata#CloudTrailDataService
Issue-Key: cloudtrail-data.put-audit-events-requires-the-cloudtrail-data-action-not-cloudtrail-wildcard|_wait_for_code#32|Timeout|time.sleep(3.0)
Issue-Commit: 1266dfca2a9321742ae6c3c2504880e1e0dae7dd

# cloudtrail-data PutAuditEvents is never authorized against the caller's IAM policy, so an unentitled principal is answered ChannelNotFound/400 instead of AccessDeniedException/403 (check never converges)

> _Note: this issue was identified against the code base at commit `1266dfca2a9321742ae6c3c2504880e1e0dae7dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAuditEvents

## Description
One root cause: the PutAuditEvents request path contains no IAM authorization step at all, so the identity of the caller has no effect on the answer. The only surviving (not-already-filed) cloudtrail-data failure in this replay is a check that holds the channel ARN, the region, the account and the event payload byte-identical across five arms and varies ONLY the inline policy on the role it assumes; the emulator never produces a different verdict for any of them and the check never converges within its budget.

=== Cause 1 of 1: no authorization decision for cloudtrail-data:PutAuditEvents ===
AWS: PutAuditEvents is authorized under the `cloudtrail-data:` IAM action prefix, which is disjoint from `cloudtrail:`. A principal that does not hold `cloudtrail-data:PutAuditEvents` is refused by the IAM authorization layer ahead of the service with error code `AccessDeniedException` and HTTP `403`, and that refusal happens BEFORE channel resolution, so the response says nothing about whether the channel exists and carries neither `successful` nor `failed`. A principal that DOES hold it gets past authorization and is refused by resolution instead: `ChannelNotFound`, HTTP `400`. Smithy: com.amazonaws.cloudtraildata#PutAuditEvents declares errors ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException — and NO AccessDeniedException, confirming the denial is produced by the authorization layer in front of the service rather than by the operation. The 403/AccessDeniedException pairing itself is recorded behavior only: the sibling check cloudtrail-data.put-audit-events-rejects-a-cross-account-channel-as-unauthorized records AccessDeniedException/403 for an unauthorized PutAuditEvents. The message example recorded there names the action `cloudtrail-data:PutAuditEvents` and the refused principal; its rule is request-derived (action + caller ARN), so the requirement is 'names the action under the cloudtrail-data prefix and the principal', not the recorded sentence.
Emulator: the verdict does not depend on the caller at all. The check assumes a freshly created role whose inline policy grants only `cloudtrail:*` on `*` (arm 2) and then only `cloudtrail:PutAuditEvents` on `*` (arm 4), and polls PutAuditEvents for `AccessDeniedException`; the emulator keeps answering the resolution-layer verdict (ChannelNotFound/400, as every other cloudtrail-data check in this replay shows for a same-account absent channel) and the expected 403 never arrives, so the check runs out of wall clock — reported verdict: 'Check timed out after 60.0s'. Hypothesis: the non-converging arm is arm 2 (the first arm that requires an authorization verdict); arm 0 and arm 1, which only require ChannelNotFound/400, are satisfied by the handler as it stands, and the alternative stall point — sts:AssumeRole on the freshly created role never succeeding inside the check's 90 s bounded retry — would equally be an emulator-side gap. Either way, no 403 is ever produced for a principal that lacks the action.
Why: a client cannot distinguish 'you may not call this' from 'the channel is not there'. It gets a 400 request-shape class where AWS gives a 403 authorization class, so permission handling, credential-vs-policy triage and retry classification are all wrong; least-privilege policies cannot be validated against the emulator (a role holding only `cloudtrail:*`, or the right action name under the wrong `cloudtrail:` prefix, is silently treated as entitled); and the emulator remains an existence oracle for callers that AWS would have refused before resolution.
Fix: an authorization gate in the PutAuditEvents handler (services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/src/ops/put_audit_events.rs) that evaluates the signing principal's effective policy for the action `cloudtrail-data:PutAuditEvents` on the parsed channel ARN and returns AccessDeniedException/403 with no `successful`/`failed` member, running BEFORE ARN resolution and before the existing post-resolution owner-account check. The action namespace must be matched on the full `cloudtrail-data:` prefix, not on the operation name alone, so `cloudtrail:*` and `cloudtrail:PutAuditEvents` do not satisfy it.
Note on scope: the open issue 20261022T215433Z-cloudtrail-data-putauditevents-rejection-fidelity-channel-arn-syntax.md (Cause 3) covers the ACCOUNT SEGMENT of the ARN being answered ChannelNotFound instead of AccessDeniedException — a resource-ownership gate that can be satisfied without any authorizer. It does not cover this cause, which holds the ARN fixed in the caller's own account and varies the principal's policy. The open cloudtrail (control-plane) issue 20261023T135552Z-cloudtrail-has-no-updatetrail-concurrency-control-cloudformation-never.md records 'authorization is never evaluated' for a different service.
Traces: cloudtrail-data.put-audit-events-requires-the-cloudtrail-data-action-not-cloudtrail-wildcard.

## Evidence
Behavioral replay against the emulator, service cloudtrail-data, operation PutAuditEvents (POST /PutAuditEvents?channelArn=...).

Failing check: cloudtrail-data.put-audit-events-requires-the-cloudtrail-data-action-not-cloudtrail-wildcard — verdict [mismatch], 'Error: Check timed out after 60.0s'. The check creates a role, assumes it, and issues the IDENTICAL PutAuditEvents call (same in-account channel ARN arn:aws:cloudtrail:<region>:<caller-account>:channel/<uuid>, same single-event batch) under three successive inline policies, polling for the expected error code after each policy change:
  arm 1  policy allows cloudtrail-data:PutAuditEvents  -> expected ChannelNotFound / 400
  arm 2  policy allows only cloudtrail:* on *          -> expected AccessDeniedException / 403, message naming cloudtrail-data:PutAuditEvents and the assumed-role session ARN, response carrying no `successful` and no `failed`
  arm 4  policy allows only cloudtrail:PutAuditEvents  -> expected AccessDeniedException / 403
  arm 5  policy restored to cloudtrail-data:PutAuditEvents -> expected ChannelNotFound / 400 again
Expected: the 400-vs-403 split tracks the policy. Actual: the expected AccessDeniedException never arrives and the check is killed at 60 s without ever observing a verdict change.

Corroborating, from the same replay: the emulator answers a same-account absent channel ChannelNotFound/400 regardless of caller (cloudtrail-data.channel-cross-account-access-denied-vs-same-account-not-found observed code 'ChannelNotFound', message 'The channel could not be found: arn:aws:cloudtrail:us-west-2:111111111111:channel/<uuid>' where AccessDeniedException/403 was expected) — i.e. no authorization verdict is reachable on this path at all.

Smithy (services/service-com-amazonaws-cloudtraildata-cloudtraildataservice/smithy-model/com-amazonaws-cloudtraildata-cloudtraildataservice.json): PutAuditEvents errors = ChannelInsufficientPermission, ChannelNotFound, ChannelUnsupportedSchema, DuplicatedAuditEventId, InvalidChannelARN, UnsupportedOperationException (all httpError 400, defaulted); no AccessDeniedException is modelled, so the 403 belongs to the authorization layer in front of the operation. Input channelArn is @required with @pattern ^arn:.*$ bound to httpQuery; output members `successful` and `failed` are @required on SUCCESS only.

Caveat recorded for the fixer: this check is marked in-repo as an unblessed draft with no .gt.jsonl oracle, so the exact denial MESSAGE text is an example, not a requirement; the requirements are the error code AccessDeniedException, HTTP 403, the absence of `successful`/`failed`, and that the verdict changes with the principal's policy.

## Steps to Reproduce
Boot the emulator. In the emulated account/region (us-west-2 in the replay):
1. iam.create_role with a trust policy allowing arn:aws:iam::<account>:root to sts:AssumeRole.
2. iam.put_role_policy granting {Allow sts:GetCallerIdentity *} plus {Allow cloudtrail-data:PutAuditEvents *}.
3. sts.assume_role on that role; build a cloudtrail-data client from the returned session credentials.
4. Pick channel_arn = arn:aws:cloudtrail:us-west-2:<account>:channel/<random-uuid> (no channel is created) and batch = [{id:<uuid>, eventData:'{"version":"1.0","userIdentity":{...},"eventSource":"example","eventName":"Probe","eventTime":"2026-01-01T00:00:00Z"}'}].
5. Call put_audit_events(channelArn=channel_arn, auditEvents=batch) with the session client — expect ChannelNotFound / HTTP 400 (positive control; this part already holds).
6. iam.put_role_policy replacing the inline policy with {Allow sts:GetCallerIdentity *} plus {Allow cloudtrail:* *} (note: NO cloudtrail-data action). Re-issue the IDENTICAL call in step 5.
   Expect AccessDeniedException / HTTP 403, a message naming cloudtrail-data:PutAuditEvents and the assumed-role session ARN arn:aws:sts::<account>:assumed-role/<role>/<session>, and a response with no `successful`/`failed` member.
   Observe instead the unchanged ChannelNotFound / HTTP 400.
7. Repeat step 6 with {Allow cloudtrail:PutAuditEvents *} — the right action name under the wrong service prefix — and expect the same AccessDeniedException / 403.
8. Restore {Allow cloudtrail-data:PutAuditEvents *} and confirm the identical call goes back to ChannelNotFound / 400.
9. Also confirm sts.get_caller_identity from the limited session still returns the session ARN, so the 403 in steps 6-7 is attributable to the action and not to a dead credential.
