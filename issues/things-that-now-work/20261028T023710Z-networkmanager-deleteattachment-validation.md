Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# DeleteAttachment orphans the Connect peers carried over a Connect attachment, although the crate has the dependent count and refuses every other orphaning delete

_Filed: 2026-10-28T02:37:10Z_
_Operation: DeleteAttachment | Class: validation_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateConnectPeer` stores every peer with `connect_attachment_id` (the Connect attachment whose tunnel it runs in) and `core_network_id`, and the crate counts those peers per attachment with `connect_peer_count_of_attachment`. `DeleteAttachment` computes its dependency refusal from only ONE dependent kind — Connect attachments carried over the deleted attachment as their transport — and never counts the Connect peers carried over the attachment itself.

So deleting a Connect attachment that still carries peers succeeds, and the peers stay in the store reporting a `ConnectAttachmentId` that `GetConnectAttachment` answers 404 for. Worse, with the attachment gone the core network's attachment count drops to zero, so `DeleteCoreNetwork` (which blocks only on attachments) now succeeds too and the peers are left with a `CoreNetworkId` that resolves to nothing while `ListConnectPeers?coreNetworkId=...` and `GetConnectPeer` keep reporting them.

## Evidence

The handler's one dependent kind — src/ops/delete_attachment.rs:152-164:
```
// A Connect attachment is built on the attachment named as its transport, so
// removing that transport would leave it pointing at nothing. ...
if store.attachments.contains_key(&input.attachment_id) {
    let carried = store.connect_attachment_count_over_transport(&input.attachment_id);
    if let Some(refusal) = crate::state::dependents_block_delete(
        crate::state::ATTACHMENT_KIND, &[(crate::state::ATTACHMENT_KIND, carried)]) { ... }
}
```
The crate's own statement of the rule, right beside that helper — src/state.rs:4098-4103: "A Connect attachment is built on another attachment rather than on a resource of its own ([`AttachmentDetail::Connect::transport_attachment_id`]), so that transport is a dependent of exactly the kind every other delete of this service refuses to orphan."

The uncalled count that already exists — src/state.rs:4143-4150: "How many Connect peers are carried over the Connect attachment `attachment_id` ... `pub fn connect_peer_count_of_attachment(&self, attachment_id: &str) -> usize`" (today used only for the create-side quota, src/ops/create_connect_peer.rs:302-304).

The peer's back-reference that is left dangling — src/ops/create_connect_peer.rs:334-335: `connect_attachment_id: input.connect_attachment_id, core_network_id,` — and the listing that keeps reporting it, src/ops/list_connect_peers.rs:170-184 (filters on `record.core_network_id` / `record.connect_attachment_id` with no existence check).

Siblings that refuse the same shape of orphaning: src/ops/delete_peering.rs:156-170, src/ops/delete_core_network.rs:156-164, src/ops/delete_device.rs:192-203.

## Steps to Reproduce

1. Create a global network, a core network with a policy configuring one edge, a VPC attachment and a Connect attachment over it; wait for AVAILABLE
2. CreateConnectPeer on the Connect attachment → 200
3. DeleteAttachment(<connect attachment id>) → 200 (expected: the dependency ValidationException 400 naming the Connect peer it still carries)
4. GetConnectPeer(<peer id>) → 200 still, reporting ConnectAttachmentId of an attachment GetConnectAttachment now answers 404 for
5. DeleteAttachment the VPC attachment, then DeleteCoreNetwork → 200 (attachment count is zero): the peer now also names a core network the store no longer holds, and ListConnectPeers?coreNetworkId=<deleted> still returns it
