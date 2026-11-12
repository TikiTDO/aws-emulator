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
#[path = "all/blueprint_lifecycle.rs"]
mod blueprint_lifecycle;
#[path = "all/blueprint_optimization.rs"]
mod blueprint_optimization;
#[path = "all/blueprint_stage_copy.rs"]
mod blueprint_stage_copy;
#[path = "all/constraint_requirement_wording.rs"]
mod constraint_requirement_wording;
#[path = "all/data_automation_library_lifecycle.rs"]
mod data_automation_library_lifecycle;
#[path = "all/data_automation_library_listing.rs"]
mod data_automation_library_listing;
#[path = "all/data_automation_project_lifecycle.rs"]
mod data_automation_project_lifecycle;
#[path = "all/data_automation_project_listing.rs"]
mod data_automation_project_listing;
#[path = "all/data_automation_project_update.rs"]
mod data_automation_project_update;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/library_ingestion_jobs.rs"]
mod library_ingestion_jobs;
#[path = "all/optimization_profile_resolution.rs"]
mod optimization_profile_resolution;
#[path = "all/project_effective_configuration.rs"]
mod project_effective_configuration;
#[path = "all/project_not_found_wording.rs"]
mod project_not_found_wording;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/timestamp_wire_form.rs"]
mod timestamp_wire_form;
