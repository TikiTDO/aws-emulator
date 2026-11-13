Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.delete-active-ksk-blocked route53.dnssec-enable-disable-state-reflected-in-getdnssec route53.dnssec-enable-disable-zone-lifecycle route53.dnssec-enable-without-active-ksk-rejected route53.ds-record-base64-round-trip route53.e2e-dnssec-enable-verify-disable-teardown route53.e2e-dnssec-sign-activate-resolve-and-teardown route53.e2e-dnssec-signing-full-flow route53.key-signing-key-in-use-deactivation-rejected route53.ksk-activate-already-active-is-idempotent route53.ksk-concurrent-activate-race route53.ksk-concurrent-activation-race route53.ksk-deactivate-last-active-key-rejected route53.ksk-ds-record-digest-recomputes-from-the-kms-public-key route53.ksk-lifecycle-create-activate-deactivate-delete route53.ksk-public-key-material-round-trip
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route-53|CreateKeySigningKey|NotImplemented
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Implement route53 CreateKeySigningKey: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeySigningKey

## Description
`CreateKeySigningKey` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/create_key_signing_key.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

16 recorded unit(s) fail on their `CreateKeySigningKey` call, so each one unblocks only when `CreateKeySigningKey` works: `route53.delete-active-ksk-blocked`, `route53.dnssec-enable-disable-state-reflected-in-getdnssec`, `route53.dnssec-enable-disable-zone-lifecycle`, `route53.dnssec-enable-without-active-ksk-rejected`, `route53.ds-record-base64-round-trip`, `route53.e2e-dnssec-enable-verify-disable-teardown`, `route53.e2e-dnssec-sign-activate-resolve-and-teardown`, `route53.e2e-dnssec-signing-full-flow`, `route53.key-signing-key-in-use-deactivation-rejected`, `route53.ksk-activate-already-active-is-idempotent`, `route53.ksk-concurrent-activate-race`, `route53.ksk-concurrent-activation-race`, `route53.ksk-deactivate-last-active-key-rejected`, `route53.ksk-ds-record-digest-recomputes-from-the-kms-public-key`, `route53.ksk-lifecycle-create-activate-deactivate-delete`, `route53.ksk-public-key-material-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/create_key_signing_key.rs`. Replaying the corpus, the emulator refused the `CreateKeySigningKey` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateKeySigningKey operation: operation CreateKeySigningKey is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53 emulator
Call CreateKeySigningKey and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
