Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-pool-schema-and-ui-customization
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-pool-schema-and-ui-customization
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|GetSigningCertificate|2xx->2xx|$.Certificate
Issue-Commit: b66c1efa9e88e06042b7a9bd3b7309ec99d3bbd2

# cognito-idp GetSigningCertificate serves an EC P-256 certificate where AWS serves an RSA x.509

> _Note: this issue was identified against the code base at commit `b66c1efa9e88e06042b7a9bd3b7309ec99d3bbd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSigningCertificate

## Description
One net-new cause (cognito-idp). GetSigningCertificate answers 200, but the Certificate it serves is not the kind of x.509 certificate AWS serves for a user pool, so the recorded shape assertion on $.Certificate fails. No open issue carries this replay key. The nearest neighbour, the open issue 'cognito-idp: 2 net-new causes — GetSigningCertificate returns a certificate whose public key is unrelated to the pool's published JWKS keys, and it is served (200) from a secondary replica region instead of OperationNotEnabledException' (20261112T021829Z-cognito-idp-2-net-new-causes-getsigningcertificate-returns.md), is grounded in the BEHAVIORAL corpus (checks cognito-idp.get-signing-certificate-matches-jwks-key and cognito-idp.operation-not-enabled-signing-cert) and carries neither this fixture nor this step; it independently corroborates the emulator's key material being EC P-256 rather than RSA.

Cause 1 of 1: the SAML 2.0 signing certificate is a self-signed ECDSA P-256 certificate instead of an RSA x.509 certificate over the pool's signing key.
AWS: GetSigningCertificate answers 200 with Certificate — Smithy: GetSigningCertificate input UserPoolId (UserPoolIdType, @length 1..55, @pattern ^[\w-]+_[0-9a-zA-Z]+$, @required), output Certificate (StringType, @length 0..131072), documented as "The x.509 certificate that signs SAML 2.0 authentication requests for your user pool". The model constrains only the string's length, so the key type is recorded behavior only: the recording's Certificate is the base64 DER of an RSA x.509 certificate (an RSA SubjectPublicKeyInfo, sha256WithRSAEncryption signature) over the pool's own signing key pair — the key the pool publishes at <endpoint>/<poolId>/.well-known/jwks.json — and is roughly 1.1-1.2 kB of base64 for a 2048-bit key. The fixture asserts that shape class, not a literal value (the recorded value is redacted to '<shaped-not-asserted>'), because the concrete DER bytes, serial, validity window and subject pool id are per-environment values and are not a requirement. What IS required is that the Certificate decodes to an x.509 certificate whose public key is an RSA key of the pool's signing key pair.
Emulator: answers 200 with a ~450-character base64 blob that decodes to a self-signed ECDSA P-256 certificate (id-ecPublicKey / prime256v1 SubjectPublicKeyInfo, ecdsa-with-SHA256 signature, subject and issuer CN=urn:amazon:cognito:sp:<poolId>, a 10-year validity), which fails the recorded shape. Hypothesis: the pool mints a fresh EC key pair for the certificate at pool-creation time instead of certifying the RSA signing key the pool's JWKS document publishes, so both the key type and the key identity are wrong.
Why: a client that wires a downstream SAML service provider or an IdP-side signature verifier from this certificate cannot use it — an RSA-expecting SAML verifier cannot even parse an EC SubjectPublicKeyInfo — and a client that cross-checks the certificate against the pool's JWKS document (which works on AWS, where both describe the same key pair) finds no match, so the federation path cannot be configured or validated at all against the emulator.
Fix: the pool's signing-key state and the certificate minted from it, not the response serializer: the certificate must be issued over one of the pool's RSA signing keys (the key pair the JWKS endpoint publishes) so that services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/get_signing_certificate.rs renders an RSA x.509 certificate.
Traces: cognito-idp-pool-schema-and-ui-customization#task#6.

## Evidence
Cause 1 — GetSigningCertificate, fixture cognito-idp-pool-schema-and-ui-customization step task#6 (200 -> 200):
  $.Certificate: value_mismatch (expected '<shaped-not-asserted>' — i.e. the recorded value is shape-asserted, an RSA x.509 certificate in base64 DER — got 'MIIBrTCCAVOgAwIBAgIUbecQXyXoSi+llP4CJnZuTRiT1A8wCgYIKoZIzj0EAwIwNDEyMDAGA1UEAwwpdXJuOmFtYXpvbjpjb2duaXRvOnNwOnVzLXdlc3QtMl9lMWMxZjczMWM...').
  Decoding the served value: the AlgorithmIdentifier is ecdsa-with-SHA256 (OID 1.2.840.10045.4.3.2, the 'MAoGCCqGSM49BAMC' bytes visible in the base64), the SubjectPublicKeyInfo is id-ecPublicKey/prime256v1 ('MBMGByqGSM49AgEGCCqGSM49AwEH'), and the subject/issuer is CN=urn:amazon:cognito:sp:<poolId>. Total length ~450 base64 characters against ~1.1-1.2 kB for the RSA certificate the recording carries.
  The pool id, serial, request id and the validity dates inside that body are per-run values and are NOT part of the requirement; the diverging property is the key type (and key identity) of the certificate.
  Independent corroboration from the behavioral corpus (different corpus, not this replay key): 20261112T021829Z-cognito-idp-2-net-new-causes-getsigningcertificate-returns.md records "signing certificate's RSA modulus was not among the 2 JWKS key modulus/moduli published for pool '<poolId>' -- cert and JWKS must describe the same key pair".

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture cognito-idp-pool-schema-and-ui-customization step task#6: CreateUserPool, then GetSigningCertificate(UserPoolId=<the created pool id>).
3. Base64-decode the returned $.Certificate and parse it as DER x.509 (e.g. `openssl x509 -inform DER -noout -text`).
4. AWS: the certificate parses with an RSA public key (sha256WithRSAEncryption signature), ~1.1-1.2 kB of base64. Emulator: it parses as a self-signed ECDSA P-256 certificate (id-ecPublicKey/prime256v1, ecdsa-with-SHA256), ~450 characters of base64 — the recorded shape assertion on $.Certificate fails.
5. Cross-check key identity: GET <cognito-idp endpoint>/<pool id>/.well-known/jwks.json and confirm the certificate's public key is one of the published keys, as it is on AWS.
