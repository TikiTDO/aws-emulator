Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.add-trust-store-revocation-invalid-content-rejected elbv2.add-trust-store-revocations-invalid-content-fails elbv2.add-trust-store-revocations-malformed-crl-rejected elbv2.add-trust-store-revocations-too-many-entries-fails elbv2.trust-store-not-found-family
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.add-trust-store-revocation-invalid-content-rejected|run#18|AssertionError|assert code == "InvalidRevocationContent", f"expected InvalidRevocationContent, got {code}"
Issue-Key: elbv2.add-trust-store-revocations-invalid-content-fails|run#8|AssertionError|assert code == "InvalidRevocationContent", f"expected InvalidRevocationContent, got {code}"
Issue-Key: elbv2.add-trust-store-revocations-malformed-crl-rejected|run#14|AssertionError|assert code == "InvalidRevocationContent", f"expected InvalidRevocationContent, got {code}"
Issue-Key: elbv2.add-trust-store-revocations-too-many-entries-fails|run#12|AssertionError|assert code == "TooManyTrustStoreRevocationEntries", (
Issue-Key: elbv2.trust-store-not-found-family|run#10|AssertionError|assert code == "TrustStoreNotFound", f"expected TrustStoreNotFound reading the CA bundle of a fabricated trust store, got {code!r}"
Issue-Commit: 4123aa4e43568a1d6639765fc2c5592729132419

# elbv2: 2 net-new causes — AddTrustStoreRevocations and GetTrustStoreCaCertificatesBundle answer 501 NotImplemented instead of the modeled trust-store refusals (InvalidRevocationContent / TooManyTrustStoreRevocationEntries / TrustStoreNotFound)

> _Note: this issue was identified against the code base at commit `4123aa4e43568a1d6639765fc2c5592729132419`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTrustStoreRevocations

## Description
Two trust-store operations of the ELBv2 emulator are still scaffold stubs that answer HTTP 501 with error code `NotImplemented`, so five behavioral checks that pin their refusal codes and messages reach the wrong verdict. Two open issues already ask for these operations to be implemented — `20261111T160347Z-elastic-load-balancing-v2-addtruststorerevocations-stub.md` and `20261111T160347Z-elastic-load-balancing-v2-gettruststorecacertificatesbundle-stub.md` — but neither carries the replay keys of the five checks below, and neither states the error classes, status codes and message templates those checks require. This issue records the behavioral requirement and the checks a fixer must re-replay; it is grounded only in checks the suite did NOT mark ALREADY FILED or OWNED.

Cause 1 of 2: AddTrustStoreRevocations answers 501 NotImplemented for every request, so none of its modeled refusals is ever reached.
Failing check ids: elbv2.add-trust-store-revocation-invalid-content-rejected, elbv2.add-trust-store-revocations-invalid-content-fails, elbv2.add-trust-store-revocations-malformed-crl-rejected, elbv2.add-trust-store-revocations-too-many-entries-fails.
AWS: Smithy (com.amazonaws.elasticloadbalancingv2#AddTrustStoreRevocations) models the errors InvalidRevocationContentException (awsQueryError code `InvalidRevocationContent`, HTTP 400 — "The provided revocation file is an invalid format, or uses an incorrect algorithm"), RevocationContentNotFoundException (`RevocationContentNotFound`, HTTP 400), TooManyTrustStoreRevocationEntriesException (`TooManyTrustStoreRevocationEntries`, HTTP 400 — "The specified trust store has too many revocation entries") and TrustStoreNotFoundException (`TrustStoreNotFound`, HTTP 400); on success it answers 200 with `TrustStoreRevocations`. Recorded behavior for the message text: an S3 object that is not an X.509 CRL is refused 400 `InvalidRevocationContent` whose Message is the fixed sentence "Provided revocation file is not in the correct format" — the whole message is fixed text, it interpolates nothing from the request. Adding revocation contents past the trust store's revocation-content quota is refused 400 `TooManyTrustStoreRevocationEntries` whose Message is "Trust store can have up to <n> revocation contents", where the fixed text is "Trust store can have up to " + " revocation contents" and <n> is the account's (adjustable) revocation-content quota, not a constant. Recorded behavior only: a single call carrying more than one RevocationContents entry is refused with a request-shape ValidationError "Up to 1 revocation contents can be specified". A just-written S3 object may legitimately answer `RevocationContentNotFound` until it is visible, so that code is retryable and must not be substituted for the invalid-content refusal. After a refusal nothing is added: DescribeTrustStoreRevocations stays empty and DescribeTrustStores reports TotalRevokedEntries 0; after a valid CRL signed by the trust store's own CA, one revoked entry exists.
Emulator: every AddTrustStoreRevocations call is answered HTTP 501 with error code `NotImplemented`, so each check fails on its first assertion — "expected InvalidRevocationContent, got NotImplemented" (three checks) and "expected TooManyTrustStoreRevocationEntries, got NotImplemented" (one check). No revocation is ever added either, so the positive controls in the same checks cannot run.
Why: a client cannot add a CRL to a trust store at all, so no mTLS revocation workflow works. Worse for correctness, `NotImplemented` is not a modeled member of this operation's error union and arrives as a 501: SDK retry policies treat a 501/unknown server fault differently from a 400 client refusal, so a caller that branches on InvalidRevocationContent (fix the file) versus TooManyTrustStoreRevocationEntries (raise the quota or delete an entry) versus RevocationContentNotFound (retry) takes none of those branches and reports a service outage instead of a bad input.
Fix: the operation's handler, its CRL content validation, and the trust-store revocation state it writes (the revocation list that DescribeTrustStoreRevocations reads and the TotalRevokedEntries counter DescribeTrustStores reports); the per-call limit of one RevocationContents entry belongs in the operation's input validation.
Traces: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/add_trust_store_revocations.rs line 161 returns `crate::scaffolded::Scaffolded::not_implemented("AddTrustStoreRevocations")`, and line 4 still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker. Example checks: elbv2.add-trust-store-revocation-invalid-content-rejected, elbv2.add-trust-store-revocations-invalid-content-fails, elbv2.add-trust-store-revocations-malformed-crl-rejected, elbv2.add-trust-store-revocations-too-many-entries-fails.

Cause 2 of 2: GetTrustStoreCaCertificatesBundle answers 501 NotImplemented instead of refusing a non-existent trust store with TrustStoreNotFound.
Failing check id: elbv2.trust-store-not-found-family.
AWS: Smithy (com.amazonaws.elasticloadbalancingv2#GetTrustStoreCaCertificatesBundle) takes the required TrustStoreArn, returns `Location` (the CA bundle's Amazon S3 URI, a pre-signed URI active for ten minutes) and models exactly one error: TrustStoreNotFoundException (awsQueryError code `TrustStoreNotFound`, HTTP 400 — "The specified trust store does not exist"). Recorded behavior for the message text: the Message is "Trust store ARN '<arn>' not found", where "Trust store ARN '" and "' not found" are fixed text and <arn> is the TrustStoreArn taken verbatim from the request — the same template the sibling trust-store reads use. For a trust store that does exist the call succeeds with a non-empty Location.
Emulator: the call is answered HTTP 501 with error code `NotImplemented`, for a fabricated ARN and for a real one alike — "expected TrustStoreNotFound reading the CA bundle of a fabricated trust store, got 'NotImplemented'". The divergence is isolated to this operation: in the same check ModifyTrustStore already refuses the identical fabricated ARN with 400 TrustStoreNotFound and the expected message, and that assertion passes before this one fails.
Why: the not-found family is inconsistent across sibling operations — a client probing whether a trust store exists gets a 400 client refusal from ModifyTrustStore and a 501 server fault from GetTrustStoreCaCertificatesBundle for the very same ARN, so existence checks and error handling that key on TrustStoreNotFound misclassify a missing resource as an emulator outage. Callers also cannot read a trust store's CA bundle at all, which breaks bundle round-trip and byte-equality verification.
Fix: the operation's handler — resolve TrustStoreArn against the trust-store state, return the modeled TrustStoreNotFoundException with the message template above when it resolves to nothing, and otherwise serialize `Location` as a pre-signed S3 URI for the stored bundle.
Traces: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/get_trust_store_ca_certificates_bundle.rs line 151 returns `crate::scaffolded::Scaffolded::not_implemented("GetTrustStoreCaCertificatesBundle")`, and line 4 still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker. Example check: elbv2.trust-store-not-found-family.

## Evidence
Cause 1 — AddTrustStoreRevocations.
Expected (AWS): HTTP 400, Error.Code `InvalidRevocationContent`, Error.Message "Provided revocation file is not in the correct format" when the referenced S3 object is not a CRL; HTTP 400, Error.Code `TooManyTrustStoreRevocationEntries`, Error.Message "Trust store can have up to <n> revocation contents" once the trust store's revocation-content quota is reached; nothing added in either case (DescribeTrustStoreRevocations == [], DescribeTrustStores[0].TotalRevokedEntries == 0); HTTP 200 with one TrustStoreRevocations entry for a well-formed CRL signed by the trust store's own CA.
Actual (emulator): HTTP 501, Error.Code `NotImplemented` for every AddTrustStoreRevocations call.
  - elbv2.add-trust-store-revocation-invalid-content-rejected: AssertionError: expected InvalidRevocationContent, got NotImplemented (check line 50)
  - elbv2.add-trust-store-revocations-invalid-content-fails: AssertionError: expected InvalidRevocationContent, got NotImplemented (check line 35)
  - elbv2.add-trust-store-revocations-malformed-crl-rejected: AssertionError: expected InvalidRevocationContent, got NotImplemented (check line 56)
  - elbv2.add-trust-store-revocations-too-many-entries-fails: AssertionError: expected TooManyTrustStoreRevocationEntries, got NotImplemented (check line 57)
Handler: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/add_trust_store_revocations.rs:161 — `Err(crate::scaffolded::Scaffolded::not_implemented("AddTrustStoreRevocations"))`.

Cause 2 — GetTrustStoreCaCertificatesBundle.
Expected (AWS): HTTP 400, Error.Code `TrustStoreNotFound`, Error.Message "Trust store ARN '<arn>' not found" (<arn> echoed from the request) for an ARN no trust store has; HTTP 200 with a non-empty `Location` for a real ACTIVE trust store.
Actual (emulator): HTTP 501, Error.Code `NotImplemented`.
  - elbv2.trust-store-not-found-family: AssertionError: expected TrustStoreNotFound reading the CA bundle of a fabricated trust store, got 'NotImplemented' (check line 39)
Control inside the same check: ModifyTrustStore with the identical fabricated ARN already answers 400 `TrustStoreNotFound` / "Trust store ARN '<arn>' not found" and passes, so the 501 is specific to this operation and not to the fabricated ARN.
Handler: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/get_trust_store_ca_certificates_bundle.rs:151 — `Err(crate::scaffolded::Scaffolded::not_implemented("GetTrustStoreCaCertificatesBundle"))`.

Related open issues that do NOT carry these checks: 20261111T160347Z-elastic-load-balancing-v2-addtruststorerevocations-stub.md, 20261111T160347Z-elastic-load-balancing-v2-gettruststorecacertificatesbundle-stub.md, 20261111T160347Z-elbv2-trust-store-plane-4-net-new-causes.md.

## Steps to Reproduce
Cause 1 (AddTrustStoreRevocations):
1. Create an S3 bucket; PutObject a PEM CA bundle at `ca-bundle.pem` and arbitrary non-CRL bytes at `not-a-crl.pem`.
2. elbv2 CreateTrustStore(Name=<name>, CaCertificatesBundleS3Bucket=<bucket>, CaCertificatesBundleS3Key=ca-bundle.pem); wait until the trust store is ACTIVE.
3. elbv2 AddTrustStoreRevocations(TrustStoreArn=<arn>, RevocationContents=[{S3Bucket:<bucket>, S3Key:not-a-crl.pem, RevocationType:CRL}]).
   Expected: HTTP 400 InvalidRevocationContent, Message "Provided revocation file is not in the correct format" (retry once on RevocationContentNotFound, which is only S3 visibility). Observed: HTTP 501 NotImplemented.
4. DescribeTrustStoreRevocations(TrustStoreArn=<arn>) and DescribeTrustStores(TrustStoreArns=[<arn>]): expected [] and TotalRevokedEntries 0.
5. Generate a CRL signed by the trust store's own CA, PutObject it, and loop AddTrustStoreRevocations one entry at a time until the quota refuses one.
   Expected: eventually HTTP 400 TooManyTrustStoreRevocationEntries, Message "Trust store can have up to <n> revocation contents". Observed: HTTP 501 NotImplemented on the first call.

Cause 2 (GetTrustStoreCaCertificatesBundle):
1. Create an S3 bucket and PutObject a PEM CA bundle.
2. Build a syntactically valid but non-existent trust-store ARN, e.g. arn:aws:elasticloadbalancing:<region>:<account>:truststore/<name>/<id>.
3. elbv2 ModifyTrustStore(TrustStoreArn=<fabricated>, CaCertificatesBundleS3Bucket=<bucket>, CaCertificatesBundleS3Key=<key>) — observe HTTP 400 TrustStoreNotFound / "Trust store ARN '<fabricated>' not found" (this already matches AWS).
4. elbv2 GetTrustStoreCaCertificatesBundle(TrustStoreArn=<fabricated>).
   Expected: HTTP 400 TrustStoreNotFound, Message "Trust store ARN '<fabricated>' not found". Observed: HTTP 501 NotImplemented.
5. CreateTrustStore and wait for ACTIVE, then GetTrustStoreCaCertificatesBundle(TrustStoreArn=<real>): expected HTTP 200 with a non-empty Location (pre-signed S3 URI). Observed: HTTP 501 NotImplemented.
