Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-local-gateway-create-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-local-gateway-create-error-paths ec2-local-gateway-vif-group-local-gateway-not-found ec2-local-gateway-virtual-interface-dry-run-errors ec2-local-gateway-virtual-interface-group-route-table-association
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:CreateLocalGatewayVirtualInterfaceGroup
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 CreateLocalGatewayVirtualInterfaceGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLocalGatewayVirtualInterfaceGroup

## Description
`CreateLocalGatewayVirtualInterfaceGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_local_gateway_virtual_interface_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `CreateLocalGatewayVirtualInterfaceGroup` call, so each one unblocks only when `CreateLocalGatewayVirtualInterfaceGroup` works: `ec2-local-gateway-create-error-paths#task#3`, `ec2-local-gateway-vif-group-local-gateway-not-found#task#1`, `ec2-local-gateway-virtual-interface-dry-run-errors#task#1`, `ec2-local-gateway-virtual-interface-group-route-table-association#task#2`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_local_gateway_virtual_interface_group.rs`. Replaying the corpus, the emulator refused the `CreateLocalGatewayVirtualInterfaceGroup` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidLocalGatewayID.NotFound', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected "The local-gateway ID 'lgw-0123456789abcdef0' does not exist", got 'operation CreateLocalGatewayVirtualInterfaceGroup is scaffolded but not implemented in this emulator')
$.Response.RequestID: missing (expected '2f2c0925-6a78-490b-af18-89a3e2392821', got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateLocalGatewayVirtualInterfaceGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
