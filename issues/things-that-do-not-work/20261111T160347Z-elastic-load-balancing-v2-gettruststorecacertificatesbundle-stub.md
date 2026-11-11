Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.get-trust-store-ca-bundle-byte-equality elbv2.get-trust-store-ca-bundle-byte-equality-round-trip elbv2.get-trust-store-ca-bundle-byte-fidelity elbv2.get-trust-store-ca-bundle-returns-full-uploaded-bundle elbv2.get-trust-store-ca-bundle-unknown-truststore-fails elbv2.modify-trust-store-nonexistent-bundle-rejected elbv2.modify-trust-store-updates-ca-bundle elbv2.trust-store-ca-bundle-binary-content-byte-equality elbv2.trust-store-ca-bundle-retrieval elbv2.trust-store-lifecycle-create-modify-delete elbv2.trust-store-modify-updates-bundle
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elastic-load-balancing-v2|GetTrustStoreCaCertificatesBundle|NotImplemented
Issue-Commit: 0e97705e9904239bd55bed92c4be71546a783c93

# Implement elastic-load-balancing-v2 GetTrustStoreCaCertificatesBundle: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0e97705e9904239bd55bed92c4be71546a783c93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTrustStoreCaCertificatesBundle

## Description
`GetTrustStoreCaCertificatesBundle` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/get_trust_store_ca_certificates_bundle.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `GetTrustStoreCaCertificatesBundle` call, so each one unblocks only when `GetTrustStoreCaCertificatesBundle` works: `elbv2.get-trust-store-ca-bundle-byte-equality`, `elbv2.get-trust-store-ca-bundle-byte-equality-round-trip`, `elbv2.get-trust-store-ca-bundle-byte-fidelity`, `elbv2.get-trust-store-ca-bundle-returns-full-uploaded-bundle`, `elbv2.get-trust-store-ca-bundle-unknown-truststore-fails`, `elbv2.modify-trust-store-nonexistent-bundle-rejected`, `elbv2.modify-trust-store-updates-ca-bundle`, `elbv2.trust-store-ca-bundle-binary-content-byte-equality`, `elbv2.trust-store-ca-bundle-retrieval`, `elbv2.trust-store-lifecycle-create-modify-delete`, `elbv2.trust-store-modify-updates-bundle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/get_trust_store_ca_certificates_bundle.rs`. Replaying the corpus, the emulator refused the `GetTrustStoreCaCertificatesBundle` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetTrustStoreCaCertificatesBundle operation: operation GetTrustStoreCaCertificatesBundle is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elastic-load-balancing-v2 emulator
Call GetTrustStoreCaCertificatesBundle and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
