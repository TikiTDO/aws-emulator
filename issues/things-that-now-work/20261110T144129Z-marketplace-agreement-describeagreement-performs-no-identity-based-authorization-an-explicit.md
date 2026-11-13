Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-agreement.describe-agreement-missing-permission-denied
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement.describe-agreement-missing-permission-denied|_poll_until_code#23|Timeout|time.sleep(interval_s)
Issue-Commit: b675dd39ab70f935a04f17e1ddfdffc8fec4d0b8

# DescribeAgreement performs no identity-based authorization: an explicit IAM Deny on aws-marketplace:DescribeAgreement never produces AccessDeniedException, and the denied leg now never settles at all (check times out)

> _Note: this issue was identified against the code base at commit `b675dd39ab70f935a04f17e1ddfdffc8fec4d0b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAgreement

## Description
One root cause, grounded in one surviving failing behavioral check.

Related open issue: 20261107T200357Z-marketplace-agreement-describeagreement-ignores-identity-based-iam-policy-an-explicit.md describes this same area and even lists this check in its Issue-Trace-Ids, but it does NOT carry this check's current replay key: it recorded the failure as an explicit `AssertionError: expected ('AccessDeniedException', ...) within 60.0s, last observed ('ResourceNotFoundException', ...)` raised from `_poll_until_code`, whereas the check now fails earlier and differently, with the harness aborting the whole check (`Check timed out after 60.0s`) before any poll loop can raise. The suite therefore did not report this check as OWNED, so it is filed here as a surviving failure; a fixer should treat the two as the same underlying defect and close this one by making the denied leg settle on the modeled 403.

Cause 1 of 1: DescribeAgreement evaluates no identity-based policy for the calling principal, so an explicit Deny on the action has no effect on the call, and the denied leg of the check never reaches its expected outcome within the check's budget.

AWS: Smithy (services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model/com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301.json) models DescribeAgreement(agreementId: ResourceId @required, @length 1..64, pattern ^[A-Za-z0-9_/-]+$) -> DescribeAgreementOutput with errors AccessDeniedException (httpError 403), ResourceNotFoundException (httpError 404), ValidationException (400), ThrottlingException (429), InternalServerException (500); auth scheme sigv4. AWS evaluates the caller's identity-based policy BEFORE resolving the resource: with an explicit Deny on aws-marketplace:DescribeAgreement attached to the calling role, the identical call on an agreementId that does not exist is refused HTTP 403 AccessDeniedException, not HTTP 404 ResourceNotFoundException; detaching the Deny returns the identical call on the identical session to the 404 outcome. The AccessDeniedException message for this class is AWS's IAM-denial template. Its FIXED text is: "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " or ", " action, or explicitly denies the actions". Its VARIABLE parts are taken from the request and account state: the calling principal's assumed-role ARN (arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name>), the action name (aws-marketplace:DescribeAgreement), the resource the policy was evaluated against ("*"), and the alternative action AWS names for this API (aws-marketplace:ViewSubscriptions). The account id, role name and session name observed in any one run are values of that run, not part of the requirement. The not-found refusal is HTTP 404 ResourceNotFoundException with message "Provided agreement does not exist." — entirely fixed text, no variable part.

Emulator: with the Deny attached, the identical DescribeAgreement call on the same assumed-role session is still answered from the agreement store and keeps returning ('ResourceNotFoundException', 'Provided agreement does not exist.'); the modeled 403 AccessDeniedException is never produced. Because that leg polls for the expected (code, message) and it never arrives, the denied leg consumes the whole budget and the harness aborts the check with `Check timed out after 60.0s` — i.e. the divergence is now visible as a non-settling denied leg rather than a single wrong response. The pre-Deny leg, the unrestricted-caller control and the post-detach restore leg all expect the same not-found outcome the emulator already gives, so the defect is isolated to the denied leg: no identity-based policy is consulted for this operation, and the store lookup runs unconditionally.

Why: a client or test asserting least-privilege behaviour — that a principal explicitly denied (or never granted) aws-marketplace:DescribeAgreement cannot read agreements — sees the call behave exactly as if it were permitted. Authorization regressions are invisible against the emulator, AWS's 403-before-404 ordering is inverted, error handling keyed on AccessDeniedException never fires, and a caller cannot distinguish "not permitted" from "does not exist". Worse, a caller that waits for the denial to take effect (IAM changes are eventually consistent, so polling is the correct client pattern) hangs until its own timeout instead of observing a refusal.

Fix: authorization for the DescribeAgreement handler — evaluate the calling principal's identity-based policy for aws-marketplace:DescribeAgreement (honouring an explicit Deny, and the aws-marketplace:ViewSubscriptions alternative) before the agreement-store lookup, and return the modeled AccessDeniedException at HTTP 403 with AWS's IAM-denial message template rendered from the calling principal, the action and the evaluated resource.

Traces: marketplace-agreement.describe-agreement-missing-permission-denied.

## Evidence
Suite: 15/18 passed (83.3%); marketplace-agreement 15/18 (83%), 3 failed.

[mismatch] marketplace-agreement.describe-agreement-missing-permission-denied
  Reported error: `Check timed out after 60.0s` (the harness aborted the check; no poll loop got to raise its own AssertionError).

  Setup performed by the check: iam.create_role with a trust policy allowing the account root to sts:AssumeRole; iam.put_role_policy Allow on aws-marketplace:DescribeAgreement; sts.assume_role into the role (session name sim2real-mpa-deny-check); a marketplace-agreement client in us-east-1 built from those session credentials.

  Leg 1 (pre-Deny): as_role.describe_agreement(agreementId=<nonexistent id>) must settle on ('ResourceNotFoundException', 'Provided agreement does not exist.').
  Leg 2 (the divergence): after iam.put_role_policy with {'Effect':'Deny','Action':'aws-marketplace:DescribeAgreement','Resource':'*'}, the identical call on the identical session must settle on HTTP 403 ('AccessDeniedException', 'User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: aws-marketplace:DescribeAgreement on resource: * because no identity-based policy allows the aws-marketplace:DescribeAgreement or aws-marketplace:ViewSubscriptions action, or explicitly denies the actions').
    Expected: that 403 AccessDeniedException.
    Actual: the emulator keeps answering HTTP 404 ('ResourceNotFoundException', 'Provided agreement does not exist.') on every poll; the expected refusal never arrives, the poll loop spins for its full budget and the check is aborted by the 60s harness timeout.
  Leg 3 (unrestricted-caller control) and leg 4 (post-detach restore) both expect the not-found outcome.

  Model: DescribeAgreement errors include AccessDeniedException with httpError 403 (AccessDeniedException is also a service-common error of AWSMPCommerceService_v20200301), alongside ResourceNotFoundException at 404.

  Prior filing with a now-stale replay key: 20261107T200357Z-marketplace-agreement-describeagreement-ignores-identity-based-iam-policy-an-explicit.md (its Issue-Key pins `_poll_until_code#23|AssertionError`, which no longer matches the timeout the check now produces).

## Steps to Reproduce
All marketplace-agreement calls go to the emulator's us-east-1 endpoint (the service answers only there).

1. iam.create_role(RoleName=<role>, AssumeRolePolicyDocument={'Version':'2012-10-17','Statement':[{'Effect':'Allow','Principal':{'AWS':'arn:aws:iam::<account-id>:root'},'Action':'sts:AssumeRole'}]}).
2. iam.put_role_policy(RoleName=<role>, PolicyName='allow-describe-agreement', PolicyDocument={'Version':'2012-10-17','Statement':[{'Effect':'Allow','Action':'aws-marketplace:DescribeAgreement','Resource':'*'}]}).
3. creds = sts.assume_role(RoleArn=<role arn>, RoleSessionName='sim2real-mpa-deny-check')['Credentials']; mpa_role = boto3.client('marketplace-agreement', region_name='us-east-1', aws_access_key_id=..., aws_secret_access_key=..., aws_session_token=...).
4. mpa_role.describe_agreement(agreementId='<nonexistent-id>') -> HTTP 404 ResourceNotFoundException 'Provided agreement does not exist.' (matches AWS).
5. iam.put_role_policy(RoleName=<role>, PolicyName='deny-describe-agreement', PolicyDocument={'Version':'2012-10-17','Statement':[{'Effect':'Deny','Action':'aws-marketplace:DescribeAgreement','Resource':'*'}]}).
6. Repeat step 4 on the SAME session, polling for IAM eventual consistency. Expected: HTTP 403 AccessDeniedException with the IAM-denial message template above. Observed: HTTP 404 ResourceNotFoundException 'Provided agreement does not exist.' on every poll, forever — the poll never settles and the caller (or the harness) times out.
7. iam.delete_role_policy(RoleName=<role>, PolicyName='deny-describe-agreement'); repeat step 4: the not-found outcome is expected and observed.
