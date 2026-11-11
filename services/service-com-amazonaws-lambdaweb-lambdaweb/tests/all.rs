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
#[path = "all/endpoint_data_plane.rs"]
mod endpoint_data_plane;
#[path = "all/enum_constraint_message.rs"]
mod enum_constraint_message;
#[path = "all/logging_config_defaults.rs"]
mod logging_config_defaults;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/revision_build_outcome.rs"]
mod revision_build_outcome;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tags.rs"]
mod tags;
#[path = "all/web_account_settings.rs"]
mod web_account_settings;
#[path = "all/web_function_endpoint.rs"]
mod web_function_endpoint;
#[path = "all/web_function_lifecycle.rs"]
mod web_function_lifecycle;
