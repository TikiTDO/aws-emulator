Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-vpc-link-errors
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-6b31aca1"].vpcLinkStatusMessage
Issue-Commit: 1243b39838b01602b7d5269139874d4fcfd3ab7e

# VPC-link AVAILABLE status message is emulator-authored prose on the GetVpcLinks list path

> _Note: this issue was identified against the code base at commit `1243b39838b01602b7d5269139874d4fcfd3ab7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVpcLinks

## Description
Cause 1 of 1: the status message the emulator derives for a VPC link that has reached the AVAILABLE status is emulator-invented prose rather than AWS's fixed sentence for that status class.

AWS: GetVpcLinks returns a collection of VpcLink representations. Smithy (com.amazonaws.apigatewayv2#VpcLink): member VpcLinkStatus -> enum com.amazonaws.apigatewayv2#VpcLinkStatus with values PENDING | AVAILABLE | DELETING | FAILED | INACTIVE, and member VpcLinkStatusMessage -> StringWithLengthBetween0And1024, jsonName 'vpcLinkStatusMessage', documented as 'A message summarizing the cause of the status of the VPC link.' The model does not enumerate the message strings, so for the exact text this is recorded behavior only: the recording shows that once a VPC link is AVAILABLE, AWS populates vpcLinkStatusMessage with the sentence 'VPC link is ready to route traffic'. For the AVAILABLE status class that whole sentence is FIXED text -- it interpolates no variable part at all (no name, id, subnet id, security group id, account, region or timestamp comes from the request or from state) -- so it is reproducible verbatim and is a requirement, not a per-run value. The equivalent REST-API shape com.amazonaws.apigateway#VpcLink carries the same pair as status/statusMessage, so the same per-status strings apply there.

Emulator: the same member is served as 'The VPC link is available for use by an integration.' HTTP status is 200 as recorded, and every other asserted member of the item (name, id, status enum, subnets, security groups) matches, so only the derived status-message text diverges.

Why: vpcLinkStatusMessage is the human-readable half of the VPC-link status contract -- it is what SDK users, CLI output, CloudFormation diagnostics and readiness/polling gates surface and assert on while waiting for a VPC link to become usable. A client that matches AWS's 'VPC link is ready to route traffic' never matches the emulator, so a link that is genuinely AVAILABLE is reported as not-yet-ready and any diagnostic echoing the message shows wording AWS never emits. Because the text is derived purely from the status and not from the request, there is no environment-dependent reason for the difference.

Fix: the VPC-link response projection/serializer for the apigateway VPC-link resource -- the code that derives statusMessage / vpcLinkStatusMessage from the stored VpcLinkStatus. The per-status message strings must be AWS's, and must be applied consistently on every path that projects a VPC link (GetVpcLinks, GetVpcLink, CreateVpcLink, UpdateVpcLink), not just on the list path.

Related already-open issues (named for the fixer; none of them covers this replay key -- the replay reports this cluster as unowned, so re-replaying this fixture/step is the only way to confirm the fix): 20261108T120854Z-apigateway-getvpclinks-reports-an-emulator-authored-vpclinkstatusmessage-for-an.md, 20261108T144540Z-apigateway-getvpclinks-reports-an-emulator-authored-vpclinkstatusmessage-for.md and 20261111T014037Z-apigateway-admits-an-unresolvable-restendpointidentifier-and-reworded-vpc.md discuss VPC-link status wording; 20261106T040418Z-apigateway-2-net-new-causes-createdomainname-accepts-an.md and 20261109T003003Z-apigatewayv2-updatevpclink-s-empty-name-refusal-answers-the.md cover VPC-link creation/refusal paths, not the status message of a successful representation.

Traces: apigatewayv2-vpc-link-errors#task#4 (GetVpcLinks after the VPC link reached AVAILABLE).

## Evidence
Replay cluster (verdict mismatch, service=apigateway, operation=GetVpcLinks, expected status 200 -> actual status 200, affected_count 1, stub_path empty, owned_by empty):
  $.items[name="sim2real-wire-vpclink-<generated>"].vpcLinkStatusMessage: value_mismatch
    expected: 'VPC link is ready to route traffic'
    actual:   'The VPC link is available for use by an integration.'

Fixed vs variable parts: for the AVAILABLE status the entire sentence 'VPC link is ready to route traffic' is fixed text; there are NO variable parts drawn from the request or from emulator state. The VPC-link name appearing in the JSONPath selector is a per-run generated name and is explicitly NOT part of the requirement -- only the message value is.

Everything else in the step matched: HTTP 200, the vpcLinkStatus enum (AVAILABLE), the link id, name, subnetIds and securityGroupIds. So the emulator's stored state and status enum are correct and the divergence is confined to the message string it synthesises for that status.

## Steps to Reproduce
1. git_clone() the repo, then replay_wire_corpus().
2. Locate fixture apigatewayv2-vpc-link-errors, step task#4 -- the GetVpcLinks call made after the VPC link has reached status AVAILABLE.
3. By hand: CreateVpcLink, poll until vpcLinkStatus is AVAILABLE, then GET the VPC-link collection and read items[].vpcLinkStatusMessage (status/statusMessage on the REST-API shape).
4. Observe the emulator answers 200 with vpcLinkStatusMessage 'The VPC link is available for use by an integration.' where AWS answers 200 with vpcLinkStatusMessage 'VPC link is ready to route traffic'.
