Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.get-outbound-web-identity-federation-info
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.get-outbound-web-identity-federation-info|run#10|AssertionError|assert _no_policy_denial(unpriv_arn, "GetOutboundWebIdentityFederationInfo").fullmatch(message), (
Issue-Commit: a0165563333d7f6521b0bb32e0431915bfb4444e

# iam: the implicit-deny AccessDenied message for iam:GetOutboundWebIdentityFederationInfo omits AWS's GetRequestAuthorizationDetails / authorization-id trailer

> _Note: this issue was identified against the code base at commit `a0165563333d7f6521b0bb32e0431915bfb4444e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOutboundWebIdentityFederationInfo

## Description
Cause 1 of 1: the AccessDenied message IAM serves a zero-permission caller of GetOutboundWebIdentityFederationInfo stops after the reason clause and omits the trailing authorization-details pointer. (The open issue 20261109T163411Z-iam-3-net-new-causes-denials-omit-the.md describes the same root cause as its cause 1, but it carries only iam.get-account-authorization-details and iam.session-policy-intersects-the-role-policy; it does not carry this check, which reaches the identical divergence through this operation. Note also the two resolved issues 20261027T032337Z-iam-secretsmanager-iam-accessdenied-message-appends-a-console-url-getrequestauthorizationdetails.md and 20261028T163805Z-iam-secretsmanager-the-iam-listusers-implicit-deny-accessdenied.md, whose fixes removed that trailer; the recording for this operation requires it, so the trailer must be produced where AWS produces it rather than dropped everywhere.)
AWS: recorded behavior only — the message template is a service fact, not modelled (Smithy: iam#GetOutboundWebIdentityFederationInfo takes no input, returns IssuerIdentifier + JwtVendingEnabled and declares only FeatureDisabledException (FeatureDisabled, HTTP 404), so an unauthorized caller gets IAM's shared AccessDenied refusal). The recorded refusal is error code AccessDenied with message `User: <caller principal arn> is not authorized to perform: iam:GetOutboundWebIdentityFederationInfo on resource: * because no identity-based policy allows the iam:GetOutboundWebIdentityFederationInfo action. Go to https://<console region>.console.aws.amazon.com/iam/home?region=<console region>#/authorization-details/<authorization id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization id>`. Fixed text: every word of both clauses, including the sentence-ending period before 'Go to' and the literal 'for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id:'. Variable parts taken from the request and the server: the caller's principal ARN, the console region (the partition's global IAM console region), and one server-minted per-request authorization id which must appear with the same value in BOTH slots (the URL fragment and the trailing id).
Emulator: the message ends at the reason clause — `User: arn:aws:iam::<account>:user/<name> is not authorized to perform: iam:GetOutboundWebIdentityFederationInfo on resource: * because no identity-based policy allows the iam:GetOutboundWebIdentityFederationInfo action` — with no terminating period, no console URL and no authorization id. The error code (AccessDenied) and the refusal itself are correct.
Why: a client or operator following the documented diagnosis path extracts the authorization id from the denial and calls GetRequestAuthorizationDetails; with no id in the message that path does not exist, and any message comparison against AWS's denial text fails even though the code and status match.
Fix: the denial-message builder in the IAM authorization gate (the implicit-deny path shared by IAM's operations, reached here by GetOutboundWebIdentityFederationInfo): append the pointer clause and mint one per-request authorization id, rendered identically in the URL fragment and in the trailing id.
Traces: iam.get-outbound-web-identity-federation-info.

## Evidence
Expected (regex in the check): 'User: <user arn> is not authorized to perform: iam:GetOutboundWebIdentityFederationInfo on resource: * because no identity-based policy allows the iam:GetOutboundWebIdentityFederationInfo action. Go to https://<region>.console.aws.amazon.com/iam/home?region=<region>#/authorization-details/<id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <id>' (same <id> in both slots). Actual: AssertionError: expected AWS's no-policy denial of iam:GetOutboundWebIdentityFederationInfo; got 'User: arn:aws:iam::000000000000:user/nopriv-ea6957f16558 is not authorized to perform: iam:GetOutboundWebIdentityFederationInfo on resource: * because no identity-based policy allows the iam:GetOutboundWebIdentityFederationInfo action'. Error code AccessDenied matched.

## Steps to Reproduce
Enable the feature if GetOutboundWebIdentityFederationInfo answers FeatureDisabled (EnableOutboundWebIdentityFederation). Create an IAM user with no policies, mint an access key for it, wait until STS GetCallerIdentity recognises the key, then call GetOutboundWebIdentityFederationInfo with those credentials and read Error.Code and Error.Message.
