Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins
Issue-Key: cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins|_poll_read#34|Timeout|time.sleep(3.0)
Issue-Commit: 3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e

# cognito-identity__s3: a bucket policy is never evaluated for a Cognito-vended role session — an explicit Deny does not refuse GetObject, so the check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `3b8e580b43dad03fd4ed613a27e30f57a2ae4b6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Cause 1 of 1: S3 does not evaluate the bucket's resource-based policy (nor the role's identity policy) for requests signed with credentials GetCredentialsForIdentity vended, so neither the policy's explicit Deny nor its load-bearing Allow changes the outcome of a read.
  AWS: a request made with a Cognito-vended session is authorized as the mapped role's principal against the union of the role's identity-based policy and the bucket policy, with an explicit Deny overriding every Allow. Concretely: (a) `granted/<key>` is readable when only the BUCKET policy allows the role (the role's own policy grants restricted/* only); (b) `restricted/<key>` is refused even though both the role policy and the bucket's Allow cover it, because the bucket policy carries an explicit Deny on that prefix — HTTP 403, error code `AccessDenied`, message `User: <principal-arn> is not authorized to perform: s3:GetObject on resource: "<object-arn>" with an explicit deny in a resource-based policy` (fixed text: `is not authorized to perform:`, `with an explicit deny in a resource-based policy`; variable parts taken from the request and state: the vended principal ARN `arn:<partition>:sts::<account-id>:assumed-role/<mapped-role>/CognitoIdentityCredentials`, the action, the object ARN); (c) removing the Deny makes `restricted/` readable again; (d) removing the bucket Allow and keeping only the Deny makes `granted/` AccessDenied too. Smithy: S3 models `GetObject` with no authorization errors beyond NoSuchKey/NoSuchBucket — the IAM/bucket-policy decision is recorded behavior only; the recording shows the 403 AccessDenied above.
  Emulator: the check is killed at the 60 s harness cap. Its waits are policy-propagation polls (120 s budget, fresh client per attempt) that poll a read until it is REFUSED — the check's own comment notes "A target that never denies fails here." The emulator serves the object on every attempt, so the poll for the explicit-Deny refusal never terminates.
  Why: a client that uses a bucket policy as the authorization boundary for Cognito guest/authenticated identities gets no boundary at all. An explicit Deny on a sensitive prefix is inert, and a bucket policy that is the ONLY grant is equally inert, so both over-permission and under-permission regressions pass against the emulator. Downstream, any scenario that waits for an S3 denial cannot terminate instead of failing cleanly.
  Fix: the S3 request path's authorization decision for requests signed with an STS role session — evaluate the bucket's resource-based policy and the assumed role's identity-based policy (explicit Deny wins) and answer the modeled 403 AccessDenied above, rather than serving every authenticated request. The cognito-identity side already mints a real role session (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs:369-425), so the gap is on the consuming side.
  Traces: cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins.

Related open issue that does not cover this (the check is not marked OWNED by it, and it describes the vended session's principal identity rather than policy evaluation): 20261025T005354Z-cognito-identity-s3-the-cognito-vended-session-is.md.

## Evidence
cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins
  expected: GetObject on restricted/<key> under a bucket policy with an explicit Deny on that prefix is refused — HTTP 403, Code "AccessDenied", message "User: arn:<partition>:sts::<account-id>:assumed-role/<mapped-role>/CognitoIdentityCredentials is not authorized to perform: s3:GetObject on resource: \"<object-arn>\" with an explicit deny in a resource-based policy"
  actual:   Check timed out after 60.0s. The only unbounded waits are `_poll_read(..., predicate=lambda r: r[0] is None, timeout_s=120)` loops that poll for exactly that refusal; the read keeps returning the object body, so no poll ever satisfies its predicate. The same inertness applies to the later legs: after the bucket Allow is removed, granted/<key> is still served instead of AccessDenied.

## Steps to Reproduce
1. Build and boot the emulator and replay cognito-identity__s3.xsvc-cognito-identity-3-bucket-policy-grants-the-federated-role-and-its-deny-prefix-wins.
2. Manually: create a bucket with objects granted/<k1> and restricted/<k2>. Create an IAM role whose trust policy admits cognito-identity.amazonaws.com for an identity pool (amr unauthenticated) and whose identity policy grants s3:GetObject on restricted/* ONLY.
3. Create the identity pool with AllowUnauthenticatedIdentities=true and SetIdentityPoolRoles Roles={unauthenticated: <roleArn>}.
4. PutBucketPolicy: Allow <roleArn> s3:GetObject on arn:aws:s3:::<bucket>/*, plus an explicit Deny on arn:aws:s3:::<bucket>/restricted/*.
5. GetId + GetCredentialsForIdentity, build an S3 client from the vended credentials, and GetObject restricted/<k2>. The object is returned; AWS refuses it with 403 AccessDenied. Then PutBucketPolicy with only the Deny (no Allow) and GetObject granted/<k1>: it is still served instead of AccessDenied.
