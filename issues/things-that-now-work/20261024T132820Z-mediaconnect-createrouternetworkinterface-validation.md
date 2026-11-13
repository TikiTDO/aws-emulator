Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# A VPC router network interface is created for a subnet and security groups that do not exist

_Filed: 2026-10-24T13:28:20Z_
_Operation: CreateRouterNetworkInterface | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`VpcRouterNetworkInterfaceConfiguration` requires `SubnetId` ("the ID of the subnet within the VPC to associate the router network interface with") and `SecurityGroupIds` ("the IDs of the security groups to associate with the router network interface within the VPC"). These are EC2 resources: MediaConnect places a real elastic network interface in that subnet, so AWS refuses a create whose subnet or security group cannot be resolved in the caller's account, and refuses a value that is not even an EC2 id (HTTP 400 BadRequestException, which this operation models).

Expected: `{"configuration":{"vpc":{"subnetId":"not-a-subnet","securityGroupIds":["nope"]}}}` is refused 400 BadRequestException and no interface is stored; a well-formed but unknown `subnet-0123456789abcdef0` is refused too.

Actual: the handler validates only the Region token, the tag bounds and the store ceiling, then stores `input.configuration.clone()` verbatim (line 284). Any string is accepted, HTTP 201 is returned, and BatchGetRouterNetworkInterface then reports an ACTIVE VPC interface attached to a subnet that exists nowhere — after which router inputs/outputs can be bound to it. The PUBLIC variant's CIDRs are at least pattern-constrained by the model; the VPC variant has no constraint at all, so nothing in the request path checks it.

## Evidence

Spec: `smithy_query(query="shape",shape="VpcRouterNetworkInterfaceConfiguration")` — both members required, `SubnetId` a plain String with no pattern, documented as the ID of a subnet within the VPC; `smithy_query(query="operation",operation="CreateRouterNetworkInterface")` models BadRequestException (400). Well-established AWS behaviour: a service that attaches an ENI validates the subnet/security-group references (cf. the analogous filed gaps for other emulated services whose subnet/security-group inputs are never resolved).

Handler: `src/ops/create_router_network_interface.rs:181-318` — region token check (195-203), tag checks (205-220), ClientToken replay (229-248), quota (254-268), then `configuration: input.configuration.clone()` at line 284. No EC2 lookup and no id-form check anywhere.

Test coverage: `tests/all/router_network_interfaces.rs:176-185,279-300,954-970` builds VPC configurations from synthetic ids and asserts success for 1 and 5 security groups, and only the MODELLED list-length bounds (0/6 security groups) are asserted as refusals — nothing covers an unresolvable subnet or a malformed id.

## Steps to Reproduce

1. POST /v1/routerNetworkInterface with {"name":"rni-vpc","configuration":{"vpc":{"subnetId":"not-a-subnet","securityGroupIds":["nope"]}}}
2. Observe HTTP 201 with a stored interface, where AWS answers 400 BadRequestException
3. POST /v1/batchGetRouterNetworkInterface?arns=<the ARN> — the interface is reported (settling to ACTIVE) for a subnet that exists in no EC2 state
4. Repeat with a well-formed but unknown subnet id subnet-0123456789abcdef0 — same unconditional acceptance
