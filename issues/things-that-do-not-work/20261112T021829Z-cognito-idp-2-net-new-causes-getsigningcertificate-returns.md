Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.get-signing-certificate-matches-jwks-key cognito-idp.operation-not-enabled-signing-cert
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.get-signing-certificate-matches-jwks-key|run#9|AssertionError|assert cert_modulus in jwks_moduli, (
Issue-Key: cognito-idp.operation-not-enabled-signing-cert|_error#16|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: cee9bc81d35f2c60da800505adfbbb9699b50fe6

# cognito-idp: 2 net-new causes — GetSigningCertificate returns a certificate whose public key is unrelated to the pool's published JWKS keys, and it is served (200) from a secondary replica region instead of OperationNotEnabledException

> _Note: this issue was identified against the code base at commit `cee9bc81d35f2c60da800505adfbbb9699b50fe6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSigningCertificate

## Description
Two net-new root causes in cognito-idp's GetSigningCertificate, both grounded in failing behavioral checks. Neither is covered by an open issue; the nearest neighbour, 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md, is about the algorithm of ISSUED TOKENS, not about the SAML signing certificate or its relationship to the pool's JWKS document, and the suite did not mark either check as owned by it.

Cause 1 of 2: the SAML 2.0 signing certificate GetSigningCertificate returns is an independently generated key pair, and not an RSA key pair at all, so it describes none of the keys the pool publishes at its own JWKS endpoint.
AWS: Smithy GetSigningCertificate — input UserPoolId (UserPoolIdType, @length 1..55, @pattern ^[\w-]+_[0-9a-zA-Z]+$, required); output Certificate (StringType, @length 0..131072), documented as "The x.509 certificate that signs SAML 2.0 authentication requests for your user pool" and "Given a user pool ID, returns the signing certificate for SAML 2.0 federation." The model does not state the key's relationship to the JWKS document, so for that part: recorded behavior only — the recording shows the certificate's RSA public key (its modulus) is one of the RSA keys the pool publishes at GET <endpoint>/<poolId>/.well-known/jwks.json (the JWKS keys carry an RSA modulus member `n`); certificate and JWKS describe the same signing key pair, not independently generated values.
Emulator: the certificate rendered by the handler is a self-signed ECDSA P-256 certificate (the response body decodes to a TBS with id-ecPublicKey/prime256v1 and an ecdsa-with-SHA256 signature) minted per pool at pool-creation time with no link to the key material the pool's JWKS endpoint publishes. The replay fails with "signing certificate's RSA modulus was not among the 2 JWKS key modulus/moduli published for pool '<poolId>' -- cert and JWKS must describe the same key pair": the pool publishes 2 RSA JWKS keys and the certificate matches neither, and it carries no RSA public key at all.
Why: a client that configures a downstream SAML service provider, or an IdP-side signature verifier, from the pool's JWKS document (or that cross-checks the certificate against the JWKS before trusting it) rejects every request the pool signs — the two documents advertise unrelated key material, and an RSA-expecting verifier cannot even parse an EC certificate. Cross-checking cert vs JWKS, which works on AWS, is impossible here.
Fix: the pool's signing-key state and the certificate it issues — `crate::state::issue_signing_certificate` and the pool's `saml_signing_certificate()` must mint the certificate over one of the pool's RSA signing keys (the key pair the JWKS endpoint publishes), rather than generating a fresh EC key pair; the handler itself only renders what state holds.
Traces: cognito-idp.get-signing-certificate-matches-jwks-key.

Cause 2 of 2: GetSigningCertificate addressed to a secondary replica region of a PLUS-tier user pool succeeds with HTTP 200 and a certificate, instead of being refused.
AWS: Smithy GetSigningCertificate declares error OperationNotEnabledException (client, HTTP 400), documented as "This exception is thrown when an operation is not available in the current region or for the current user pool configuration. This can occur when attempting to perform operations that are not supported in secondary replica regions." The recorded behavior for a pool whose replica was created with CreateUserPoolReplica is: the primary region serves the certificate, and the same UserPoolId sent to the replica region answers HTTP 400 OperationNotEnabledException with message "This operation is not enabled in this region for this User Pool" (the whole message is fixed text for this error class; it carries no request or state values). Before the replica region knows the pool, it transiently answers ResourceNotFoundException "User pool <UserPoolId> does not exist." — fixed text "User pool " + the UserPoolId from the request + " does not exist." — and then settles on the OperationNotEnabledException refusal.
Emulator: the replica region serves the request: HTTP 200 with a {"Certificate": "..."} body (replay recorded a 200 with the EC certificate above). The handler in services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/get_signing_certificate.rs only looks the pool up in state and renders its certificate; it has no region/replica gate and can never return the OperationNotEnabledException variant its own error enum declares.
Why: a client validating that replica-region endpoints reject operations that are primary-only gets a success here, so code that is supposed to fail over to the primary region silently reads a certificate from the wrong region; conversely, a test asserting the documented refusal cannot pass against the emulator. The operation is overly permissive: a refusal AWS always issues is absent.
Fix: the operation's handler/validation in services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/get_signing_certificate.rs, together with the replica state that records which region is the pool's primary (as written by CreateUserPoolReplica/ListUserPoolReplicas): when the request's region is a secondary replica region for the named pool, return OperationNotEnabledException (HTTP 400) with the fixed message above instead of rendering the certificate.
Traces: cognito-idp.operation-not-enabled-signing-cert.

## Evidence
Cause 1 — check cognito-idp.get-signing-certificate-matches-jwks-key (mismatch):
  Expected (AWS/recording): the RSA modulus extracted from GetSigningCertificate's Certificate is one of the moduli published at <endpoint>/<poolId>/.well-known/jwks.json.
  Actual (emulator): AssertionError: signing certificate's RSA modulus was not among the 2 JWKS key modulus/moduli published for pool 'us-west-2_2eb34e379' -- cert and JWKS must describe the same key pair. The certificate itself is EC P-256/ecdsa-with-SHA256, so it contains no RSA public key; the only DER SEQUENCE(INTEGER,INTEGER) in it is the ECDSA signature.

Cause 2 — check cognito-idp.operation-not-enabled-signing-cert (mismatch):
  Expected (AWS/recording): ('OperationNotEnabledException', 'This operation is not enabled in this region for this User Pool'), HTTP 400, when GetSigningCertificate(UserPoolId=<primary pool id>) is sent to the replica region.
  Actual (emulator): HTTP 200 with {'Certificate': 'MIIBrDCCAVOgAwIBAgIUJrMEFMB/GtZ8tA5nX6VlDtpnBrgw...'} (an EC P-256 self-signed cert for subject CN=urn:amazon:cognito:sp:us-west-2_8a4451417). Replay failure: AssertionError: expected the call to be refused, but it succeeded. Note the generated pool id, request id and timestamps in that body are per-run values and are not part of the requirement.

## Steps to Reproduce
Cause 1:
1. CreateUserPool(PoolName=<any>, DeletionProtection=INACTIVE).
2. GetSigningCertificate(UserPoolId=<pool id>) and parse the RSA modulus out of the returned x.509 certificate.
3. GET <cognito-idp endpoint>/<pool id>/.well-known/jwks.json and collect the `n` (modulus) of every published key.
4. AWS: the certificate's modulus is one of the JWKS moduli. Emulator: the certificate is an EC P-256 certificate and matches none of the published RSA keys.

Cause 2:
1. Create a multi-Region KMS CMK, grant cognito-idp use of it, and replicate it to a second region.
2. CreateUserPool(UserPoolTier=PLUS, KeyConfiguration={KeyType: CUSTOMER_MANAGED_KEY, KmsKeyArn: <CMK arn>}).
3. GetSigningCertificate(UserPoolId=<pool id>) in the primary region — succeeds (both on AWS and on the emulator).
4. CreateUserPoolReplica(UserPoolId=<pool id>, RegionName=<second region>) and poll ListUserPoolReplicas until the replica appears.
5. GetSigningCertificate(UserPoolId=<pool id>) against the second (replica) region.
6. AWS: HTTP 400 OperationNotEnabledException 'This operation is not enabled in this region for this User Pool' (after a transient ResourceNotFoundException 'User pool <pool id> does not exist.'). Emulator: HTTP 200 with a certificate.
