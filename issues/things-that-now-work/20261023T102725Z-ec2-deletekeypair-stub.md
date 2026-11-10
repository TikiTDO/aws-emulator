Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-t12-import-key-pair-fingerprint
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-t12-import-key-pair-fingerprint
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteKeyPair
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 DeleteKeyPair: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteKeyPair

## Description
`DeleteKeyPair` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_key_pair.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `DeleteKeyPair` call, so each one unblocks only when `DeleteKeyPair` works: `ec2-t12-import-key-pair-fingerprint#task#7`, `ec2-t12-import-key-pair-fingerprint#teardown#9`, `ec2-t12-import-key-pair-fingerprint#teardown#10`, `ec2-t12-import-key-pair-fingerprint#teardown#11`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_key_pair.rs`. Replaying the corpus, the emulator refused the `DeleteKeyPair` call: status: expected 200, got 501
$.DeleteKeyPairResponse: missing (expected {'requestId': '42c7f84f-986b-4b43-bac3-28156ffc21ef', 'return': 'true', 'keyPairId': '{{key_rsa_id}}'}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteKeyPair and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
