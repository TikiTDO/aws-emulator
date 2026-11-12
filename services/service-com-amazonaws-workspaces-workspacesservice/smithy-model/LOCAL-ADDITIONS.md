# Local additions to the Smithy model snapshot

`com-amazonaws-workspaces-workspacesservice.json` in this directory is a snapshot
of the published WorkSpaces model and is kept EXACTLY as shipped: it is a
third-party artefact consumed at build time by `#[smithy_macros::smithy]`, so an
edit there would be invisible to a reader of the crate's source and lost on the
next refresh.

AWS answers some operations with members that snapshot does not declare. Each is
declared in this crate's own source instead, from the small overlay model
`local-additions.json` beside the snapshot, so the declaration is generated the
same way every modelled shape is — one member list, one set of constraint
traits, no hand-written schema or serializer. The same mechanism is used by
`services/service-com-amazonaws-iotwireless-iotwireless`
(`smithy-model/local-reported-service-profile.json` +
`src/ops/types/service_profile_report_types.rs`).

## `WorkspaceProperties$RecycleMode`

- **What AWS does**: a `WorkspaceProperties` submitted with `RecycleMode` comes
  back carrying it — recorded in the `simparity-wire-aws-v1` wire fixtures
  `workspaces-create-and-migrate-workspace-to-new-bundle` (step `setup#3`) and
  `workspaces-create-workspaces-batch-start-stop-reboot` (step `task#3`), where
  the `CreateWorkspaces` echo in `FailedRequests[].WorkspaceRequest` reports the
  `RecycleMode` the request supplied.
- **What the snapshot declares**: `com.amazonaws.workspaces#WorkspaceProperties`
  with nine members, none of them `RecycleMode`, and no `RecycleMode` shape at
  all.
- **Where it is declared**: `src/ops/types/workspace_properties_types.rs`, whose
  `WorkspaceProperties` is the single type every operation of this crate reads
  and reports those properties through (`CreateWorkspaces`,
  `DescribeWorkspaces`, `ModifyWorkspaceProperties`). It is generated from
  `local-additions.json`, which restates the nine modelled members — with their
  own targets, so every enum, range and list constraint is captured exactly as
  the snapshot spells it — and adds the tenth. An open `string`, because nothing
  publishes the value set: whatever a request carries is what comes back.
- **What else is in the overlay**: only the shapes those ten members target, and
  only as much of each as codegen reads — the enums carry their values (that is
  what validation enforces), the scalars their type. `GlobalAcceleratorForWorkSpace`
  is a structure, which codegen resolves through the Rust field's type rather
  than through the model, so the overlay declares the shape without restating
  its members; the modelled type in `shared_types.rs` is what the member reads
  and writes.

`tests/all/workspace_properties_local_members.rs` pins every half of this: the
member on the wire; the snapshot's own member list, which the local declaration
must cover in full; and each shape the overlay restates, compared declaration for
declaration against the snapshot (type, members, enum values and constraint
traits, documentation aside). If the snapshot is ever refreshed with
`RecycleMode`, with a member the overlay does not restate, or with a changed
target, value set or constraint, that suite fails — and the local declaration
should then give way to the modelled shape.
