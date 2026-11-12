Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: controltower.access-denied-without-permission-rejected
Issue-Shape-Id: com.amazonaws.controltower#AWSControlTowerApis
Issue-Key: controltower.access-denied-without-permission-rejected|_poll#16|Timeout|time.sleep(interval_s)
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# controltower requests are never authorized: an explicit IAM Deny on controltower:ListLandingZones does not refuse the call, so AccessDeniedException is unreachable

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListLandingZones

## Description
One cause.

Cause 1 of 1 — identity-policy evaluation is absent from the controltower request path, so an explicit Deny is ignored and every signed caller is treated as authorized.
AWS: Smithy: ListLandingZones declares AccessDeniedException as a modelled error with httpError 403 (alongside ValidationException, ThrottlingException, InternalServerException). AWS evaluates the caller's identity policies before the operation runs, and an explicit Deny overrides an Allow for the same action. Recorded behavior: with an inline role policy that Allows controltower:ListLandingZones and a second inline policy that Denies the same action, the call made under that assumed role is refused with error code AccessDeniedException and the message 'User: arn:aws:sts::<account>:assumed-role/<roleName>/<sessionName> is not authorized to perform: controltower:ListLandingZones on resource: arn:aws:controltower:<region>:<account>:landingzone/* with an explicit deny in an identity-based policy'. In that message the fixed text is 'User: ', ' is not authorized to perform: ', ' on resource: ' and ' with an explicit deny in an identity-based policy'; the variable parts come from the request's signing identity (account id, role name, session name), the action being the operation's IAM action name, and the resource being the operation's landingzone ARN for the caller's Region and account. Removing the Deny while leaving the Allow in place must make the identical call succeed again.
Emulator: the call keeps succeeding after the Deny is attached. The check polls the same signed call every 2s for 60s waiting for AccessDeniedException and times out, i.e. the emulator never produced a 403 for this principal; AccessDeniedException is not reachable through policy at all. The positive controls confirm the call itself works (it succeeded before the Deny), so this is authorization not being evaluated, not a broken operation.
Why: a client relying on AWS's behavior gets a security-relevant divergence. Guardrail and least-privilege tests pass against the emulator that would be refused by AWS: code paths exercised under a Deny never see the 403 they must handle, and a caller validating that a role CANNOT read landing zones is told it cannot, while the emulator proves nothing. Every negative authorization assertion in the suite is silently unverifiable.
Fix: authorization for the controltower front door — evaluate the signing principal's identity policies (Deny over Allow) before dispatching the operation, and emit the modelled AccessDeniedException 403 with AWS's wording built from the signing identity, the operation's IAM action and the operation's resource ARN. The same gate is needed for every operation in this service, not just ListLandingZones.
Traces: controltower.access-denied-without-permission-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (controltower), check controltower.access-denied-without-permission-rejected [mismatch]:

Error: Check timed out after 60.0s.

The check creates a role with an inline Allow for controltower:ListLandingZones, confirms (positive control, by polling) that ListLandingZones under that assumed role SUCCEEDS, then attaches a second inline policy with Effect Deny for the same action and polls the identical signed call every 2s for up to 60s for error code AccessDeniedException. The emulator never returns it — the probe keeps succeeding until the 60s budget is exhausted, so the poll never satisfies its predicate. Expected on the Deny: code AccessDeniedException with message 'User: arn:aws:sts::<account>:assumed-role/<roleName>/sim2real-ct-denied-probe is not authorized to perform: controltower:ListLandingZones on resource: arn:aws:controltower:<region>:<account>:landingzone/* with an explicit deny in an identity-based policy'. The timeout is the emulator's own answer to the probe (a 200 where a 403 is required), not a missing runner tool or a transport failure.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay suite simparity-behavioral-aws-v1, check controltower.access-denied-without-permission-rejected.
2. iam.CreateRole with a trust policy allowing sts:AssumeRole from arn:aws:iam::<account>:root; iam.PutRolePolicy attaching {Effect: Allow, Action: controltower:ListLandingZones, Resource: '*'}.
3. Assume that role and call controltower.ListLandingZones — it succeeds (positive control).
4. iam.PutRolePolicy attaching a second inline policy {Effect: Deny, Action: controltower:ListLandingZones, Resource: '*'}.
5. Re-issue the identical signed ListLandingZones call under the assumed role; observe it still returns HTTP 200 with a landingZones list indefinitely, instead of 403 AccessDeniedException with the explicit-deny message.
6. iam.DeleteRolePolicy for the Deny policy and re-issue the call; against AWS this restores success, proving the Deny (not an absent grant) caused the refusal — the emulator cannot show that transition because it never refused.
