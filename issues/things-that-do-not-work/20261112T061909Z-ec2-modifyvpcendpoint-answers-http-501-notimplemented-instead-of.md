Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.vpc-endpoint-gateway-create-reject-bad-service
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.vpc-endpoint-gateway-create-reject-bad-service|run#25|AssertionError|assert modify_code == "InvalidVpcEndpointId.NotFound", (
Issue-Commit: d9e20b4dbdf06191d17fc8b16aa8f32f57ea4689

# ec2 ModifyVpcEndpoint answers HTTP 501 NotImplemented instead of refusing a deleted endpoint id with InvalidVpcEndpointId.NotFound

> _Note: this issue was identified against the code base at commit `d9e20b4dbdf06191d17fc8b16aa8f32f57ea4689`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVpcEndpoint

## Description
ONE net-new root cause in the EC2 VPC-endpoint control plane.

Cause 1 of 1: ModifyVpcEndpoint is an unimplemented scaffold handler, so every call to it — including the one AWS refuses because the endpoint id no longer exists — comes back as HTTP 501 NotImplemented.

AWS: the Smithy model (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models ModifyVpcEndpoint with input member VpcEndpointId (target VpcEndpointId, required) plus PolicyDocument (String), ResetPolicy, Add/RemoveRouteTableIds, Add/RemoveSubnetIds, Add/RemoveSecurityGroupIds, IpAddressType, DnsOptions, PrivateDnsEnabled, SubnetConfigurations, DryRun, and output member Return (boolean, xmlName `return`); errors: [] because EC2 is an ec2Query service whose error codes are wire-only, so the refusal classes are recorded behavior only. Recorded behavior: once a VPC endpoint has been deleted (DeleteVpcEndpoints succeeded and the id is no longer describable, i.e. it has left state `deleted`), a ModifyVpcEndpoint naming that id is refused with HTTP 400 and error code `InvalidVpcEndpointId.NotFound`, message "The Vpc Endpoint Id '<VpcEndpointId>' does not exist". Message template: the fixed text is "The Vpc Endpoint Id '" … "' does not exist"; the only variable part is the VpcEndpointId copied verbatim from the request (the vpce- id of any one run fills the template and is not part of it). A successful ModifyVpcEndpoint on a live endpoint answers HTTP 200 with Return=true and persists the change (e.g. the new PolicyDocument is visible to a subsequent DescribeVpcEndpoints).

Emulator: the operation is a scaffold stub — services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_endpoint.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and its handle() body is `Err(crate::scaffolded::Scaffolded::not_implemented("ModifyVpcEndpoint"))`, so the call answers HTTP 501 with error code `NotImplemented` (message "operation ModifyVpcEndpoint is scaffolded but not implemented in this emulator") for every input, valid or not. The replayed check sees error code `NotImplemented` where `InvalidVpcEndpointId.NotFound` is required: "AssertionError: expected the deleted endpoint id to be unusable, got NotImplemented".

Why: a client cannot modify a VPC endpoint at all — no policy document update, no route-table/subnet/security-group add or remove, no private-DNS or IP-address-type change ever takes effect, and nothing is persisted. Worse for negative paths: code `NotImplemented` with status 501 is a server-fault class, so an SDK retries it and a caller that branches on `InvalidVpcEndpointId.NotFound` (to decide the endpoint is already gone) never takes that branch, while any caller that asserts the endpoint-not-found refusal reads a 501 as an emulator outage rather than the documented client error.

Fix: implement ModifyVpcEndpoint in its handler, services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_endpoint.rs (the only file for this operation): resolve VpcEndpointId against the vpc-endpoint state, refuse an unknown or already-deleted id with HTTP 400 / `InvalidVpcEndpointId.NotFound` and the message template above, apply the policy/route-table/subnet/security-group/DNS mutations to the stored endpoint record for a live id, answer 200 with Return=true, and delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line once implemented.

Related but NOT covering: open issue 20261102T094631Z-ec2-modifyvpcendpoint-stub.md asks for the same operation to be implemented, but its replay key names only `ec2__lambda__s3__sqs.interface-endpoint-second-subnet-in-an-occupied-zone-rejected`; the check below is not carried by it (the suite reports this failure as neither ALREADY FILED nor OWNED), so this issue records the second recorded unit that only unblocks when ModifyVpcEndpoint works, and in particular its deleted-id refusal class and message. Also related but distinct: 20261109T225653Z-ec2-vpc-endpoint-plane-missing-endpoint-policy-refusal.md (CreateVpcEndpoint policy refusal and VpcEndpoint projection) and 20261105T144252Z-ec2-route-table-and-vpc-endpoint-lifecycle-operations.md.

Traces: ec2.vpc-endpoint-gateway-create-reject-bad-service.

## Evidence
Behavioral suite replay against the emulator built from this checkout (suite simparity-behavioral-aws-v1, bootstrap_signal false, 868/2070 checks passed). The failing check is the only one in the report marked neither "ALREADY FILED" nor "OWNED by":

### ec2
#### [mismatch] ec2.vpc-endpoint-gateway-create-reject-bad-service
Error: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_ec2_vpc_endpoint_gateway_create_reject_bad_service.py", line 110, in run
    assert modify_code == "InvalidVpcEndpointId.NotFound", (
AssertionError: expected the deleted endpoint id to be unusable, got NotImplemented

What the check does before that point (and which the emulator satisfied): CreateVpcEndpoint with a bogus service name is refused with InvalidServiceName / "The Vpc Endpoint Service '<name>' does not exist"; a Gateway endpoint for com.amazonaws.<region>.s3 is created, reaches state available on an independent DescribeVpcEndpoints, and DeleteVpcEndpoints returns Unsuccessful == []. It then polls until the id is no longer describable (state "deleted" or InvalidVpcEndpointId.NotFound) and calls:

    ec2.modify_vpc_endpoint(VpcEndpointId=ep_id, PolicyDocument=json.dumps({...full access...}))

Expected (AWS): error code InvalidVpcEndpointId.NotFound, message f"The Vpc Endpoint Id '{ep_id}' does not exist" (HTTP 400).
Actual (emulator): error code NotImplemented (HTTP 501).

Handler evidence in the checkout: services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_endpoint.rs lines 3-6 carry "//! @op ModifyVpcEndpoint / //! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`" and line 175 is `Err(crate::scaffolded::Scaffolded::not_implemented("ModifyVpcEndpoint"))`.

Model evidence: smithy_query(query="operation", operation="ModifyVpcEndpoint", model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json") -> input {VpcEndpointId required, PolicyDocument, ResetPolicy, AddRouteTableIds, RemoveRouteTableIds, AddSubnetIds, RemoveSubnetIds, AddSecurityGroupIds, RemoveSecurityGroupIds, IpAddressType, DnsOptions, PrivateDnsEnabled, SubnetConfigurations, DryRun}, output {Return (xmlName `return`)}, errors [].

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from this checkout.
2. Replay the behavioral suite and re-run check ec2.vpc-endpoint-gateway-create-reject-bad-service (service heading: ec2).
3. Manually: CreateVpc (10.103.0.0/16), wait for state available; CreateVpcEndpoint(VpcId=<vpc>, ServiceName=com.amazonaws.<region>.s3, VpcEndpointType=Gateway); wait until DescribeVpcEndpoints reports state available; DeleteVpcEndpoints([<vpce-id>]) and poll until the id reports state "deleted" or InvalidVpcEndpointId.NotFound.
4. Call ModifyVpcEndpoint(VpcEndpointId=<the deleted vpce-id>, PolicyDocument='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}]}').
   Observe HTTP 501 with error code NotImplemented instead of HTTP 400 with code InvalidVpcEndpointId.NotFound and message "The Vpc Endpoint Id '<vpce-id>' does not exist".
5. Also call ModifyVpcEndpoint on a LIVE gateway endpoint (same PolicyDocument): observe the same 501 and that a following DescribeVpcEndpoints shows the policy unchanged, where AWS answers 200 with Return=true and persists it.
