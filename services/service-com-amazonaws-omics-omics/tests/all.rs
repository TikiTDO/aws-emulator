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
#[path = "all/multipart_read_set_uploads.rs"]
mod multipart_read_set_uploads;
#[path = "all/private_workflow_runs.rs"]
mod private_workflow_runs;
#[path = "all/response_member_wire_form.rs"]
mod response_member_wire_form;
#[path = "all/run_caches.rs"]
mod run_caches;
#[path = "all/run_groups.rs"]
mod run_groups;
#[path = "all/sequence_stores.rs"]
mod sequence_stores;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/workflow_versions.rs"]
mod workflow_versions;
