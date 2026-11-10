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
#[path = "all/client_error_envelope.rs"]
mod client_error_envelope;
#[path = "all/date_time_members_wire_form.rs"]
mod date_time_members_wire_form;
#[path = "all/empty_success_answers.rs"]
mod empty_success_answers;
#[path = "all/logging_configuration_lifecycle.rs"]
mod logging_configuration_lifecycle;
#[path = "all/logging_destination_resolution.rs"]
mod logging_destination_resolution;
#[path = "all/room_lifecycle.rs"]
mod room_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_rules_and_tagging.rs"]
mod validation_rules_and_tagging;
