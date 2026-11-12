// SPDX-License-Identifier: CC0-1.0

//! `WorkspaceProperties`: the properties of a WorkSpace, as AWS carries them.
//!
//! This is the ONE `WorkspaceProperties` type of this crate — the shape
//! `WorkspaceRequest`, `Workspace` and `ModifyWorkspacePropertiesRequest` all
//! reach — so what it declares is what every operation of this service parses,
//! persists and reports.
//!
//! It is declared here rather than beside the other shared shapes because AWS
//! answers with one member the published model snapshot does not declare, and
//! that snapshot is kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `RecycleMode` — a request that submits it gets it back, recorded in the
//!   `CreateWorkspaces` echo of the `simparity-wire-aws-v1` wire fixtures. The
//!   snapshot declares neither the member nor a shape of that name.
//!
//! So the type is generated, like every other modelled shape, from a model file
//! — the small overlay `smithy-model/local-additions.json`, which restates the
//! nine members the snapshot declares, with their own targets so every enum and
//! list constraint is captured exactly as the snapshot spells it, and adds the
//! tenth. Nothing about the wire is hand-written: the member names, the shape id
//! and the constraints all come from that file.
//!
//! `tests/all/workspace_properties_local_members.rs` pins the member on the wire
//! and the snapshot's own member list, which this declaration must cover in
//! full.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.workspaces#WorkspaceProperties")]
#[derive(Debug, Clone, Default)]
pub struct WorkspaceProperties {
    pub running_mode: Option<String>,
    pub running_mode_auto_stop_timeout_in_minutes: Option<i32>,
    pub root_volume_size_gib: Option<i32>,
    pub user_volume_size_gib: Option<i32>,
    pub compute_type_name: Option<String>,
    pub protocols: Option<Vec<String>>,
    pub operating_system_name: Option<String>,
    pub global_accelerator: Option<GlobalAcceleratorForWorkSpace>,
    pub nested_virtualization_enabled: Option<bool>,
    /// The member the snapshot does not declare. An open string: whatever a
    /// request submits is what this service stores and reports back, so no value
    /// set is invented for it.
    pub recycle_mode: Option<String>,
}
