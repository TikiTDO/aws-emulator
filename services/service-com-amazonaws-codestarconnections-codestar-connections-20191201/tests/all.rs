// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/host_arn_rules.rs"]
mod host_arn_rules;
#[path = "all/host_deletion.rs"]
mod host_deletion;
#[path = "all/host_lifecycle.rs"]
mod host_lifecycle;
#[path = "all/host_listing.rs"]
mod host_listing;
#[path = "all/host_reads.rs"]
mod host_reads;
#[path = "all/host_reference_resolution.rs"]
mod host_reference_resolution;
#[path = "all/host_status_descriptions.rs"]
mod host_status_descriptions;
#[path = "all/host_update.rs"]
mod host_update;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/repository_link_lifecycle.rs"]
mod repository_link_lifecycle;
#[path = "all/repository_link_listing.rs"]
mod repository_link_listing;
#[path = "all/repository_link_reads.rs"]
mod repository_link_reads;
#[path = "all/repository_link_update.rs"]
mod repository_link_update;
#[path = "all/repository_sync_definitions.rs"]
mod repository_sync_definitions;
#[path = "all/repository_sync_status.rs"]
mod repository_sync_status;
#[path = "all/resource_sync_status.rs"]
mod resource_sync_status;
#[path = "all/sdk_target_dispatch.rs"]
mod sdk_target_dispatch;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/sync_blocker_resolution.rs"]
mod sync_blocker_resolution;
#[path = "all/sync_blocker_summary.rs"]
mod sync_blocker_summary;
#[path = "all/target_prefix_forms.rs"]
mod target_prefix_forms;
#[path = "all/wire_target_routing.rs"]
mod wire_target_routing;
