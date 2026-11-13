Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-nat-gateway-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-nat-gateway-lifecycle ec2-nat-gateway-private-connectivity-workflow
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:CreateNatGateway
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 CreateNatGateway: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNatGateway

## Description
`CreateNatGateway` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_nat_gateway.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateNatGateway` call, so each one unblocks only when `CreateNatGateway` works: `ec2-nat-gateway-lifecycle#task#6`, `ec2-nat-gateway-private-connectivity-workflow#task#3`, `ec2-nat-gateway-private-connectivity-workflow#task#4`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_nat_gateway.rs`. Replaying the corpus, the emulator refused the `CreateNatGateway` call: status: expected 200, got 501
$.CreateNatGatewayResponse: missing (expected {'requestId': 'dbca1bae-9ad6-4e31-bf62-ea600381cccc', 'clientToken': 'd40c83e8-0560-4aa1-ba13-ff71e16fdfd9', 'natGateway': {'availabilityMode': 'zonal', 'connectivityType': 'public', 'createTime': '2026-09-08T10:05:05.000Z', 'natGatewayAddressSet': {'item': {'allocationId': 'eipalloc-00000000000000021', 'isPrimary': 'true', 'status': 'associating'}}, 'natGatewayId': '{{nat_gateway_id}}', 'state': 'pending', 'subnetId': 'subnet-00000000000000020', 'tagSet': {'item': {'key': 'Name', 'value': 'sim2real-probe-ec2-nat-4d32a745'}}, 'vpcId': 'vpc-0000000000000001c'}}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateNatGateway and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
