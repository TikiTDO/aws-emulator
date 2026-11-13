Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.e2e-federation-token-policy-scopes-down-then-expires sts.federation-token-session-policy-scope-and-expiration-at-requested-duration
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.e2e-federation-token-policy-scopes-down-then-expires|run#20|AssertionError|assert message == (
Issue-Key: sts.federation-token-session-policy-scope-and-expiration-at-requested-duration|run#21|AssertionError|assert message == (
Issue-Commit: cc1e381aa14d9fe18794857891e95e0b753bc640

# sts: a GetFederationToken session-policy refusal is attributed to "no identity-based policy" instead of "no session policy", so the AccessDenied message for a federated-user session is wrong

> _Note: this issue was identified against the code base at commit `cc1e381aa14d9fe18794857891e95e0b753bc640`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFederationToken

## Description
Cause 1 of 1: when credentials minted by GetFederationToken are used for an action the inline session policy does NOT allow, the emulator now correctly refuses with HTTP 403 / AccessDenied, but the refusal message attributes the implicit deny to the wrong policy type — it says "because no identity-based policy allows the <action> action" where AWS says "because no session policy allows the <action> action". Failing checks: sts.e2e-federation-token-policy-scopes-down-then-expires, sts.federation-token-session-policy-scope-and-expiration-at-requested-duration.

AWS: Smithy (com.amazonaws.sts#GetFederationToken) models Policy as sessionPolicyDocumentType (@length 1..2048, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$) and documents it as an inline *session policy*: the session's permissions are the intersection of the IAM user's policies and the session policy, and "you cannot use session policies to grant more permissions than those that are defined in the permissions policy of the IAM user". Because the deny comes from the SESSION policy boundary and not from the principal's identity policies, AWS words the called service's implicit-deny message with the session-policy clause. Recorded behavior only for the exact text: the message is
  User: <FederatedUser.Arn> is not authorized to perform: <service>:<Action> on resource: "<resource ARN>" because no session policy allows the <service>:<Action> action
Its FIXED text is `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no session policy allows the `, ` action`, and the quoting of the resource ARN. Its VARIABLE parts are taken from the request and the emulator's state: the principal ARN is exactly the FederatedUser.Arn that GetFederationToken returned (arn:aws:sts::<account>:federated-user/<Name>), the action is the action the caller attempted (here s3:CreateBucket), and the quoted resource ARN is the resource the request names (arn:aws:s3:::<bucket> for CreateBucket). No account id, federated-user name or bucket name from any single run is part of the requirement. HTTP status 403 and error code AccessDenied are unchanged and already correct.

Emulator: the status (403) and the code (AccessDenied) match, and the principal ARN, action and quoted resource ARN in the message are all correct, but the trailing clause is `because no identity-based policy allows the s3:CreateBucket action`. Replay output for both checks is the assertion on the message text, e.g.
  AssertionError: User: arn:aws:sts::<account>:federated-user/<name> is not authorized to perform: s3:CreateBucket on resource: "arn:aws:s3:::<bucket>" because no identity-based policy allows the s3:CreateBucket action
The legs before it pass: the mint is faithful (Expiration = issuance + requested DurationSeconds within tolerance, FederatedUser.Arn ends in federated-user/<Name>), the session-policy-allowed s3:ListAllMyBuckets succeeds, and the denied CreateBucket has no effect (the bucket is absent from a subsequent admin ListBuckets). So only the attribution clause of the message diverges.

Why: the clause is the only thing in an AWS implicit-deny message that tells a caller WHICH policy withheld the action. A client or operator debugging a least-privilege federation flow reads "no identity-based policy allows ..." and concludes the IAM user's own policy is missing the permission, and will widen the user's identity policy — which changes nothing, because the actual boundary is the inline session policy passed to GetFederationToken. Test suites and log scrapers that match on the session-policy clause (and the AWS-documented distinction between identity-based, session, and resource-based implicit denies) never see it.

Fix: the authorization decision must record WHY the action was implicitly denied — that the matching boundary was the session policy attached to the credential, not the principal's identity policies — and the implicit-deny message serializer in the called service's request-authorization path must select the session-policy clause when the deciding boundary is a session policy. The clause selection belongs with the AccessDenied message construction used by the service that accepts the federation credential (S3 request authorization is the enforcement site these checks exercise), fed by the session scope that the GetFederationToken handler stores on the minted session in the STS crate's session state.

Related but NOT covering this: open issue 20261111T093908Z-sts-a-getfederationtoken-session-policy-does-not-scope.md described the earlier state in which s3:CreateBucket under that session was SERVED 200; the refusal now happens, so what remains is only the message's attribution clause, and that issue does not carry either check's current replay key.

Traces: sts.e2e-federation-token-policy-scopes-down-then-expires (denied CreateBucket under a GetFederationToken session policy of Allow s3:ListAllMyBuckets only), sts.federation-token-session-policy-scope-and-expiration-at-requested-duration (same refusal reached from a fresh IAM user whose own identity policy DOES allow s3:CreateBucket, which is precisely why the session-policy clause is required).

## Evidence
Checks sts.e2e-federation-token-policy-scopes-down-then-expires [mismatch] and sts.federation-token-session-policy-scope-and-expiration-at-requested-duration [mismatch].

Expected (AWS): CreateBucket with credentials from GetFederationToken whose inline Policy is {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]} is refused HTTP 403 / Code=AccessDenied with Message:
  User: <FederatedUser.Arn> is not authorized to perform: s3:CreateBucket on resource: "arn:aws:s3:::<bucket>" because no session policy allows the s3:CreateBucket action

Actual (emulator): HTTP 403 / Code=AccessDenied (both correct) with Message:
  User: <FederatedUser.Arn> is not authorized to perform: s3:CreateBucket on resource: "arn:aws:s3:::<bucket>" because no identity-based policy allows the s3:CreateBucket action

Replay output, check sts.e2e-federation-token-policy-scopes-down-then-expires:
  File "check_sts_e2e_federation_token_policy_scopes_down_then_expires.py", line 66, in run
    assert message == (
  AssertionError: User: arn:aws:sts::000000000000:federated-user/sim2real-fed-6daf7c17be13 is not authorized to perform: s3:CreateBucket on resource: "arn:aws:s3:::sim2real-behav-fed-407690cdbf6f" because no identity-based policy allows the s3:CreateBucket action

Replay output, check sts.federation-token-session-policy-scope-and-expiration-at-requested-duration:
  File "check_sts_federation_token_session_policy_scope_and_expiration_at_requested_duration.py", line 168, in run
    assert message == (
  AssertionError: User: arn:aws:sts::000000000000:federated-user/sim2real-fed-3c2c9df6fd26 is not authorized to perform: s3:CreateBucket on resource: "arn:aws:s3:::sim2real-behav-fed-85c90801faa8" because no identity-based policy allows the s3:CreateBucket action

(The account id, federated-user name and bucket name above are per-run values that fill the template; only the clause ` because no session policy allows the ` / ` action` is the requirement that diverges.)

Passing legs that precede the divergence, so neither the mint nor the enforcement is the defect: the allowed s3:ListAllMyBuckets succeeds under the session credentials; Expiration equals the response Date + the requested DurationSeconds (900 s) within tolerance; FederatedUser.Arn ends in federated-user/<Name>; the denied CreateBucket created nothing. In the second check the IAM user's OWN identity policy explicitly allows s3:CreateBucket, so "no identity-based policy allows" is not merely differently worded — it is factually false for that request.

## Steps to Reproduce
1. Boot the emulator and replay sts.e2e-federation-token-policy-scopes-down-then-expires and sts.federation-token-session-policy-scope-and-expiration-at-requested-duration.
2. Or by hand: iam.create_user(UserName=<u>) + iam.put_user_policy(<u>, PolicyDocument=Allow on ["sts:GetFederationToken","s3:ListAllMyBuckets","s3:CreateBucket"] Resource "*") + iam.create_access_key(<u>).
3. With that user's long-term key: resp = sts.get_federation_token(Name=<<=32 chars>, DurationSeconds=900, Policy=json.dumps({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]})). Note resp["FederatedUser"]["Arn"].
4. Build an S3 client from resp["Credentials"] (AccessKeyId / SecretAccessKey / SessionToken). s3.list_buckets() succeeds — correct.
5. Call s3.create_bucket(Bucket=<new name>[, CreateBucketConfiguration={"LocationConstraint": <region>} outside us-east-1]). It is refused 403 / AccessDenied — correct — but read the Error.Message: it ends `because no identity-based policy allows the s3:CreateBucket action`. AWS ends it `because no session policy allows the s3:CreateBucket action`, with the same principal ARN (the FederatedUser ARN), action and quoted resource ARN.
