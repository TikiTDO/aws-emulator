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
#[path = "all/campaign_lifecycle.rs"]
mod campaign_lifecycle;
#[path = "all/campaign_plane_refusals.rs"]
mod campaign_plane_refusals;
#[path = "all/campaign_rename.rs"]
mod campaign_rename;
#[path = "all/campaign_state_batch.rs"]
mod campaign_state_batch;
#[path = "all/common.rs"]
mod common;
#[path = "all/dial_request_batch.rs"]
mod dial_request_batch;
#[path = "all/input_constraint_refusals.rs"]
mod input_constraint_refusals;
#[path = "all/instance_onboarding.rs"]
mod instance_onboarding;
#[path = "all/onboarding_job_deletion.rs"]
mod onboarding_job_deletion;
#[path = "all/outbound_call_config_update.rs"]
mod outbound_call_config_update;
#[path = "all/shared_connect_instance_gate.rs"]
mod shared_connect_instance_gate;
#[path = "all/signing_name_path_claim.rs"]
mod signing_name_path_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
