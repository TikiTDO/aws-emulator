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
#[path = "all/device_pools.rs"]
mod device_pools;
#[path = "all/network_profile_lifecycle.rs"]
mod network_profile_lifecycle;
#[path = "all/network_profiles.rs"]
mod network_profiles;
#[path = "all/not_found_wording.rs"]
mod not_found_wording;
#[path = "all/project_vpc_config.rs"]
mod project_vpc_config;
#[path = "all/projects.rs"]
mod projects;
#[path = "all/reference_checks.rs"]
mod reference_checks;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/uploads.rs"]
mod uploads;
