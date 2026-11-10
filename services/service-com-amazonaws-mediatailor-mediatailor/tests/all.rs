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
#[path = "all/channel_delete.rs"]
mod channel_delete;
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/channel_response_projection.rs"]
mod channel_response_projection;
#[path = "all/channel_start_stop.rs"]
mod channel_start_stop;
#[path = "all/configure_logs_scope.rs"]
mod configure_logs_scope;
#[path = "all/create_channel_name_collision.rs"]
mod create_channel_name_collision;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/playback_configuration.rs"]
mod playback_configuration;
#[path = "all/playback_configuration_logging.rs"]
mod playback_configuration_logging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_wire_form.rs"]
mod tagging_wire_form;
#[path = "all/wire_diagnostic.rs"]
mod wire_diagnostic;
