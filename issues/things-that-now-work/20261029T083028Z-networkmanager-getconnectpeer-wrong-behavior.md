Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2afcd9ff504122666b9644d9cff6b63f96e63ba3

# "No modification error" is reported as an ABSENT `LastModificationErrors` for a Connect peer and as an EMPTY LIST for every attachment and peering

_Filed: 2026-10-29T08:30:28Z_
_Operation: GetConnectPeer | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2afcd9ff504122666b9644d9cff6b63f96e63ba3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`LastModificationErrors` is modelled the same way on `ConnectPeer`, `Attachment` and `Peering`: an optional list of length 0..20 documented as "Describes the error associated with the … request". This crate renders the identical situation — a resource this service accepted and stored, which has no failed modification — two opposite ways in the same file, each justified by a comment that contradicts the other:

* `ConnectPeerRecord::to_shape_with_state` answers `last_modification_errors: None`, i.e. the member is absent, with a comment stating the member must be "absent rather than an empty list of errors nothing produced".
* `PeeringRecord::to_peering_with_state` and `AttachmentRecord::to_attachment_at` answer `Some(Vec::new())`, i.e. `[]`, each with a comment stating "the list is empty rather than carrying an error nothing produced".

So `GetConnectPeer` / `DeleteConnectPeer` omit the member while `GetVpcAttachment`, `GetConnectAttachment`, `GetSiteToSiteVpnAttachment`, `GetDirectConnectGatewayAttachment`, `GetTransitGatewayRouteTableAttachment`, `ListAttachments`, `GetTransitGatewayPeering` and `DeletePeering` all report `[]` for the same state of affairs. A client that reads the member has to special-case the Connect peer. Nothing in the model distinguishes the two cases, and the crate's two tests assert the two opposite outputs with the SAME stated reason, so the reason cannot justify the divergence — the divergence is the defect: one rule has to reach both renderings.

## Evidence

The odd one out — src/state.rs:3196-3203 (reached by src/ops/get_connect_peer.rs:142-145 `.map(|record| record.to_shape(...))`):
```
            tags: Some(self.tags.clone()),
            subnet_arn: self.subnet_arn.clone(),
            // "Describes the error associated with the attachment request" (`smithy_query
            // shape=ConnectPeer`): a peer this service accepted and stored has no failed
            // modification to report, so the member is absent rather than an empty list of
            // errors nothing produced.
            last_modification_errors: None,
```
The opposite rule, for the same situation — src/state.rs:2562-2566:
```
            created_at: Some(epoch_seconds(self.created_at)),
            // No modification of this peering has failed: the list is empty rather than
            // carrying an error nothing produced.
            last_modification_errors: Some(Vec::new()),
```
and src/state.rs:2944-2947:
```
            updated_at: Some(epoch_seconds(updated_at)),
            // No modification of this attachment has failed: the list is empty rather than
            // carrying an error nothing produced.
            last_modification_errors: Some(Vec::new()),
```
The model draws no distinction — `smithy_query query="shape" shape="ConnectPeer,Peering"`: `ConnectPeer$LastModificationErrors` = `{"target": "ConnectPeerErrorList", "length": {"min": 0, "max": 20}, "type": "list", "doc": "<p>Describes the error associated with the attachment request.</p>"}` and `Peering$LastModificationErrors` = `{"target": "PeeringErrorList", "length": {"min": 0, "max": 20}, "type": "list", "doc": "<p>Describes the error associated with the Connect peer request.</p>"}` — same optionality, same bounds, same meaning.

The two tests that pin the two outputs with the same stated reason — tests/all/connect_peer_lifecycle.rs:446-449:
```
    assert!(
        peer["LastModificationErrors"].is_null(),
        "case {name:?}: a peer this service accepted has no modification error, got {peer}"
    );
```
versus tests/all/transit_gateway_peering.rs:279-283:
```
    assert_eq!(
        envelope["LastModificationErrors"],
        json!([]),
        "case {name:?}: a peering this service accepted has no modification error, got {peering}"
    );
```
(and the same `json!([])` expectation in tests/all/get_vpc_attachment.rs:337, tests/all/get_connect_attachment.rs:333, tests/all/attachment_lifecycle.rs:282, tests/all/site_to_site_vpn_attachment_lifecycle.rs:279, tests/all/direct_connect_gateway_attachment_lifecycle.rs:294, tests/all/transit_gateway_route_table_attachment_lifecycle.rs:378,505).

## Steps to Reproduce

1. Create a global network, a core network with an edge, a VPC attachment on it and a Connect attachment over that VPC attachment, then CreateConnectPeer on the Connect attachment
2. GET /connect-peers/{ConnectPeerId} -> the payload carries NO `LastModificationErrors` member
3. GET /vpc-attachments/{AttachmentId} for the transport attachment -> the payload carries `"LastModificationErrors": []`
4. CreateTransitGatewayPeering on the same core network, then GET /transit-gateway-peerings/{PeeringId} -> `"LastModificationErrors": []`
5. Compare: three resources in the identical state ("accepted, nothing failed") report the same modelled member two different ways
