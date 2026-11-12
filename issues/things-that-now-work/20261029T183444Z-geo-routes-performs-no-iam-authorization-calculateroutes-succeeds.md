Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: geo-routes.calculate-routes-missing-permission-rejected geo-routes.iam-deny-calculate-routes-access-denied
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes.calculate-routes-missing-permission-rejected|_client_error#15|AssertionError|raise AssertionError(f"real AWS denies this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: geo-routes.iam-deny-calculate-routes-access-denied|_poll#20|Timeout|time.sleep(interval_s)
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# geo-routes performs no IAM authorization: CalculateRoutes succeeds for an unpermissioned principal and ignores an explicit Deny

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateRoutes

## Description
This issue carries 1 net-new root cause with two independent observations.

Cause 1 of 1: the geo-routes data plane never evaluates the caller's identity-based policy, so every signed request is served regardless of whether geo-routes:CalculateRoutes is allowed or explicitly denied.
AWS: Smithy: CalculateRoutes declares the error AccessDeniedException with errorKind client and httpError 403 (alongside ValidationException/ThrottlingException/InternalServerException), i.e. an authorization refusal is part of this operation's modelled contract. The message text is not in the model, so: recorded behavior - AWS refuses an unauthorized caller with HTTP 403, Error.Code 'AccessDeniedException' and the standard IAM authorization sentence. Its fixed text is 'User: <caller ARN> is not authorized to perform: <service>:<Action> on resource: <resource ARN>' followed by the reason clause, which is 'because no identity-based policy allows the <service>:<Action> action' for an implicit deny and 'with an explicit deny in an identity-based policy' when a Deny statement matched. The variable parts come from the request and account state: the caller's ARN (for an assumed role, arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>), the action geo-routes:CalculateRoutes, and the resource arn:aws:geo-routes:<region>::provider/default. Nothing in the message is a per-run value beyond those identifiers.
Emulator: serves HTTP 200 with a fully synthesized route in both situations. (a) A freshly created role with NO attached policies, assumed via sts:AssumeRole and used to sign CalculateRoutes, gets 200 (PricingBucket 'Core', one Car leg, Summary.Distance 1568) where a 403 AccessDeniedException is due. (b) An IAM user whose inline policy's single statement is flipped from Allow to Deny on geo-routes:CalculateRoutes keeps getting 200 for the identical call; the check polls for the refusal and never observes one, so it runs until the harness timeout (60s). Hypothesis: the geo-routes handlers authenticate the SigV4 signature but never call the policy evaluator, so neither an implicit nor an explicit deny is reachable.
Why: this is an authorization bypass. Any test or deployment rehearsal that uses the emulator to prove least-privilege scoping - that a role lacking geo-routes permissions cannot route, or that a guardrail Deny blocks the action - passes vacuously, and the resulting policy ships untested. Clients that branch on AccessDeniedException (re-auth, escalate, fall back to a cached route) never exercise that branch.
Fix: the geo-routes request pipeline's authorization component - evaluate the caller's identity-based policies for geo-routes:<Action> on arn:aws:geo-routes:<region>::provider/default before dispatching to the operation handler, and return the modelled AccessDeniedException (403) with the implicit-deny or explicit-deny sentence as appropriate. The IAM state the checks build (create_role/assume_role, put_user_policy with Allow then Deny) is already served, so only the geo-routes-side evaluation is missing.
Traces: geo-routes.calculate-routes-missing-permission-rejected (unpermissioned assumed role, expected 403 AccessDeniedException, got 200); geo-routes.iam-deny-calculate-routes-access-denied (explicit Deny never takes effect; the check's Allow positive control and its Deny poll are the same principal and policy name, so the divergence is the policy effect being ignored, not the credential being rejected).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator (28/62 passed; all failures in geo-routes).

geo-routes.calculate-routes-missing-permission-rejected [mismatch]:
  AssertionError: real AWS denies this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...'x-amz-geo-pricing-bucket': 'Core'...}, 'PricingBucket': 'Core', 'LegGeometryFormat': 'FlexiblePolyline', 'Notices': [], 'Routes': [{'Legs': [{... 'TravelMode': 'Car', 'Summary': {'Overview': {'Distance': 1568, 'Duration': 113 ...}}}]}]}
  Expected: Error.Code 'AccessDeniedException' with Message 'User: arn:aws:sts::<account>:assumed-role/<role-name>/sim2real-georoutes-noperm is not authorized to perform: geo-routes:CalculateRoutes on resource: arn:aws:geo-routes:<region>::provider/default because no identity-based policy allows the geo-routes:CalculateRoutes action' (account id, role name and region are values from the run's account/state; the rest is fixed text).
  The same check's positive control - the harness's own permissioned credentials making the identical call - is expected to succeed, so the divergence is the absence of a deny, not an unreachable endpoint.

geo-routes.iam-deny-calculate-routes-access-denied [mismatch]:
  Error: Check timed out after 60.0s, inside the poll described as 'the Deny statement blocks geo-routes:CalculateRoutes on the real signed call'. The check creates an IAM user, attaches an inline policy {Effect: Allow, Action: geo-routes:CalculateRoutes, Resource: '*'}, confirms the signed call succeeds, then rewrites the SAME policy name with Effect Deny and polls the identical call for a refusal.
  Expected once the Deny is attached: ('AccessDeniedException', 'User: <user ARN> is not authorized to perform: geo-routes:CalculateRoutes on resource: arn:aws:geo-routes:<region>::provider/default with an explicit deny in an identity-based policy'). Observed: the call keeps returning 200, so the poll never satisfies its predicate.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the behavioral suite simparity-behavioral-aws-v1 (service geo-routes).
2. Replay geo-routes.calculate-routes-missing-permission-rejected: create an IAM role whose trust policy allows the account root and attach NO permission policies, sts:AssumeRole into it, and sign POST /v2/routes (Origin/Destination/TravelMode=Car) with the returned session credentials. Expect HTTP 403 AccessDeniedException with the implicit-deny sentence; the emulator answers HTTP 200 with a synthesized route.
3. Replay geo-routes.iam-deny-calculate-routes-access-denied: create an IAM user, put an inline policy allowing geo-routes:CalculateRoutes on '*', create an access key, confirm the signed POST /v2/routes succeeds, then overwrite the SAME inline policy name with Effect=Deny and repeat the identical call. Expect HTTP 403 AccessDeniedException with the explicit-deny sentence; the emulator keeps answering HTTP 200 until the check's 60s budget expires.
4. Flip the policy back to Allow and confirm the call succeeds - this is the check's restore step and must keep passing after the fix.
