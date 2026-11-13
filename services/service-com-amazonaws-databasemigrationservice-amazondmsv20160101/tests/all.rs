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
#[path = "all/endpoint_projection.rs"]
mod endpoint_projection;
#[path = "all/migration_task_slice.rs"]
mod migration_task_slice;
#[path = "all/rejection_paths.rs"]
mod rejection_paths;
#[path = "all/service_access_role_resolution.rs"]
mod service_access_role_resolution;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subnet_group_coverage_message.rs"]
mod subnet_group_coverage_message;
#[path = "all/subnet_group_handle.rs"]
mod subnet_group_handle;
#[path = "all/subnet_group_placement.rs"]
mod subnet_group_placement;
