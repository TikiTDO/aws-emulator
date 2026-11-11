Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-accept-address-transfer-no-pending-transfer-error
Issue-Step: teardown#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-accept-address-transfer-no-pending-transfer-error ec2-allocate-address-vpc-and-unowned-address-error ec2-conflict-elastic-ip-association ec2-describe-addresses-attribute-boundary ec2-describe-addresses-lifecycle ec2-eip-allocate-returns-allocation-id-and-public-ip ec2-elastic-ip-lifecycle ec2-nat-gateway-lifecycle ec2-t12-eip-disassociate-and-domain-attribute
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:ReleaseAddress
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 ReleaseAddress: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReleaseAddress

## Description
`ReleaseAddress` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/release_address.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `ReleaseAddress` call, so each one unblocks only when `ReleaseAddress` works: `ec2-accept-address-transfer-no-pending-transfer-error#teardown#3`, `ec2-allocate-address-vpc-and-unowned-address-error#teardown#4`, `ec2-conflict-elastic-ip-association#task#9`, `ec2-conflict-elastic-ip-association#teardown#12`, `ec2-describe-addresses-attribute-boundary#teardown#4`, `ec2-describe-addresses-lifecycle#teardown#5`, `ec2-eip-allocate-returns-allocation-id-and-public-ip#teardown#3`, `ec2-elastic-ip-lifecycle#teardown#9`, `ec2-nat-gateway-lifecycle#teardown#10`, `ec2-t12-eip-disassociate-and-domain-attribute#teardown#17`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/release_address.rs`. Replaying the corpus, the emulator refused the `ReleaseAddress` call: status: expected 200, got 501
$.ReleaseAddressResponse: missing (expected {'requestId': '7ab03b8e-157d-45f8-b1a8-23369efe4a4f', 'return': 'true'}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call ReleaseAddress and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
