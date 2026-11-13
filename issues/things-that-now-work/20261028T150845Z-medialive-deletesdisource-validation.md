Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# DeleteSdiSource enforces only the "not attached to any input" half of its documented precondition: a source a node's SdiSourceMapping names is deleted

_Filed: 2026-10-28T15:08:45Z_
_Operation: DeleteSdiSource | Class: validation_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Smithy documentation for DeleteSdiSource states TWO preconditions: the source must not be part of any SdiSourceMapping AND must not be attached to any input. The handler quotes that very sentence in its own comment but checks only the second half: `state.sdi_source_is_in_use(...)`, which is defined as `!inputs_using_sdi_source(id).is_empty()` — a walk of the INPUT store only. Nothing consults the `sdi_source_mappings` that `UpdateNode` stores on a node, so a source that a node maps to a card/channel port is deleted with 200 and reaches the terminal DELETED state.

That leaves the store in a shape the crate's own node writer refuses to create: `described_sdi_source_mappings` resolves every mapping's `sdiSource` against the live sources and refuses a deleted one with `UnknownSdiSource` ("sdiSourceMappings references SdiSource <id>, which this account does not hold"), yet after the delete `DescribeNode`/`ListNodes`/`UpdateNodeState` keep reporting `sdiSourceMappings` naming exactly such a source. The operation models ConflictException (409) — the answer it already gives for the input half — so the refusal has a modelled home.

## Evidence

Smithy (the rule, verbatim): smithy_query(query="doc", shape="DeleteSdiSource") -> "Delete an SdiSource. The SdiSource must not be part of any SidSourceMapping and must not be attached to any input."

Handler quotes the same sentence but implements half of it — services/service-com-amazonaws-medialive-medialive/src/ops/delete_sdi_source.rs:173-184:
```
        // "The SdiSource must not be part of any SdiSourceMapping and must not be
        // attached to any input": a source an input is using is a conflict, not a
        // deletion.
        if state.sdi_source_is_in_use(&input.sdi_source_id) {
            return Err(DeleteSdiSourceErrorModeled::ConflictException(ConflictException {
```

The check it calls looks only at inputs — src/state.rs:2722-2727:
```
    /// Whether an input is attached to the SdiSource `id`, which is what
    /// `DeleteSdiSource` refuses on ("The SdiSource must ... not be attached to
    /// any input").
    pub fn sdi_source_is_in_use(&self, id: &str) -> bool {
        !self.inputs_using_sdi_source(id).is_empty()
    }
```
(and `inputs_using_sdi_source`, src/state.rs:2704-2720, iterates `self.inputs` only).

The crate's node writer treats a mapping onto a deleted source as impossible — src/state/nodes.rs:813-852:
```
    /// ... name an SdiSource this account holds ("The ID of the SdiSource to associate with this port on this
    /// card. ..."), ... a
    /// deleted source is held by the account no longer, so it is nothing a new
    /// mapping can connect a port to.
...
            if self
                .sdi_sources
                .get(&sdi_source)
                .is_none_or(SdiSourceRecord::is_deleted)
            {
                return Err(NodeUpdateRefusal::UnknownSdiSource(sdi_source));
```
and the node summary reports the stored mappings unfiltered — src/state/nodes.rs:613: `sdi_source_mappings: Some(node.sdi_source_mappings.clone()),`

Test coverage today: only the INPUT half is pinned — tests/all/sdi_source_lifecycle.rs:418-427 ("so a source an input is using cannot be deleted (\"The SdiSource must not be part of any SdiSourceMapping and must not be attached to any input\")", `a_source_an_input_uses_is_in_use_by_that_input`), and tests/all/node_settings_update.rs:580-585 pins the UpdateNode refusal "sdiSourceMappings references SdiSource {doomed}, which this account does not hold". No test exercises deleting a source a node maps, so the mapping half is unenforced and unpinned.

## Steps to Reproduce

1. POST /prod/sdiSources with {"name":"wired","type":"SINGLE"} and note the returned sdiSource.id (call it S).
2. POST /prod/clusters to create a cluster C, then POST /prod/clusters/C/nodes to create a node N.
3. PUT /prod/clusters/C/nodes/N with {"sdiSourceMappings":[{"cardNumber":1,"channelNumber":1,"sdiSource":"S"}]} — accepted, the node now stores the mapping.
4. DELETE /prod/sdiSources/S — expected: refused, because "The SdiSource must not be part of any SidSourceMapping" (the modelled ConflictException, as the input half already answers). Actual: 200 with sdiSource.state DELETED.
5. GET /prod/clusters/C/nodes/N — sdiSourceMappings still names S, a deleted source; re-sending the same mapping through PUT /prod/clusters/C/nodes/N is now refused 400 "sdiSourceMappings references SdiSource S, which this account does not hold", so the store holds a node configuration the service itself says cannot exist.
