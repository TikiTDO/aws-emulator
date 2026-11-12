Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.get-web-identity-token-rejected-by-assume-role-with-web-identity
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.get-web-identity-token-rejected-by-assume-role-with-web-identity|_refusal#8|AssertionError|raise AssertionError(
Issue-Commit: 0814fbf24ca8581d1be982874f66970e2423c484

# IAM CreateOpenIDConnectProvider accepts the account's own in-partition STS web-identity issuer URL instead of refusing it with InvalidInput

> _Note: this issue was identified against the code base at commit `0814fbf24ca8581d1be982874f66970e2423c484`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOpenIDConnectProvider

## Description
One net-new root cause, surfaced by an STS outbound-web-identity-federation check.

Cause 1 of 1: IAM CreateOpenIDConnectProvider registers an inbound OIDC provider whose Url is an STS issuer identifier from the same partition, where AWS refuses the call.

AWS: outbound web-identity federation (IAM EnableOutboundWebIdentityFederation / GetOutboundWebIdentityFederationInfo, which return the account's `IssuerIdentifier`, and STS GetWebIdentityToken, which mints JWTs signed by that issuer) is outbound-only. An issuer identifier belonging to the same partition cannot be turned back into an inbound trust anchor: CreateOpenIDConnectProvider called with that issuer URL is refused. Smithy (com.amazonaws.iam#CreateOpenIDConnectProvider): the operation models InvalidInputException -> awsQueryError code `InvalidInput`, HTTP 400. Recorded behavior: the refusal carries the message "Creating an OIDC provider with an STS issuer URL from the same partition is not supported. Within a partition, use AssumeRole instead of web identity federation." That whole sentence is fixed text for this refusal class — it interpolates nothing from the request, so the Url the caller passed does not appear in it.

Emulator: the call SUCCEEDS with HTTP 200 and returns an OpenIDConnectProviderArn minted from that issuer URL (of the form arn:aws:iam::<account>:oidc-provider/<issuer host and path>, e.g. the replay observed `arn:aws:iam::000000000000:oidc-provider/<sts-host>:<port>/<issuer-id>`; the host, port and issuer id are per-environment values, only the acceptance is the defect). No InvalidInput is raised and the provider is stored and readable.

Why: a client that relies on AWS refusing this (and on the sentence AWS words it with) never sees the refusal, so it believes it has created a working inbound trust anchor for the partition's own STS issuer. In the emulated account that provider then exists and can be named as a `Federated` principal in a role trust policy, i.e. a same-partition token-exchange path that AWS deliberately blocks becomes reachable — a trust topology no real account can be put into. Negative-path tests that assert the InvalidInput refusal fail, and the guidance AWS gives in the message ("use AssumeRole instead") is never delivered.

Fix: the Url validation in the create_open_id_connect_provider handler — it must resolve the account's outbound web-identity-federation issuer identifier (the same value IAM's EnableOutboundWebIdentityFederation / GetOutboundWebIdentityFederationInfo report, i.e. the emulator's own STS issuer) and, when the submitted Url names that in-partition STS issuer, refuse with InvalidInput (HTTP 400) and AWS's fixed sentence instead of minting an ARN and writing a provider record.

Traces: sts.get-web-identity-token-rejected-by-assume-role-with-web-identity (the check's Refusal 1; handler file services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_open_id_connect_provider.rs).

Not covered by the open issue 20261029T192356Z-iam-10-net-new-validation-and-accept-reject.md: its ten causes on this operation are the missing thumbprint derivation and the non-https Url message, neither of which is the same-partition STS-issuer refusal described here.

## Evidence
Failing check sts.get-web-identity-token-rejected-by-assume-role-with-web-identity, at its first assertion:

  File "check_sts_get_web_identity_token_rejected_by_assume_role_with_web_identity.py", line 41, in run
    provider_code, provider_message = _refusal(_create_provider)
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned 'arn:aws:iam::000000000000:oidc-provider/127.0.0.1:41983/F7B11509F4D675C3C44F0DD37CA830BB'

Expected (AWS): CreateOpenIDConnectProvider(Url=<the account's outbound web-identity IssuerIdentifier>, ClientIDList=[<audience>]) -> HTTP 400, Error.Code `InvalidInput`, Error.Message "Creating an OIDC provider with an STS issuer URL from the same partition is not supported. Within a partition, use AssumeRole instead of web identity federation." (entirely fixed text for this class).
Actual (emulator): HTTP 200 with an OpenIDConnectProviderArn derived from the issuer URL; the provider is created. The account id, host, port and issuer-id inside the returned ARN are per-environment values and are not part of the requirement — the 200 is.

## Steps to Reproduce
1. Call IAM EnableOutboundWebIdentityFederation (or GetOutboundWebIdentityFederationInfo if already enabled) and take the returned `IssuerIdentifier`; that URL is the account's in-partition STS web-identity issuer.
2. Call IAM CreateOpenIDConnectProvider with Url set to that exact IssuerIdentifier and ClientIDList set to any audience, e.g. ["https://example.com/webid-rejection-check"].
3. AWS refuses with HTTP 400 / InvalidInput and the message "Creating an OIDC provider with an STS issuer URL from the same partition is not supported. Within a partition, use AssumeRole instead of web identity federation."; the emulator answers HTTP 200 and returns an OpenIDConnectProviderArn, and GetOpenIDConnectProvider on that ARN reads the provider back.
