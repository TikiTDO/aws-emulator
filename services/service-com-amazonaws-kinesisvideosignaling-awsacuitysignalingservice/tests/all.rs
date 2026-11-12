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
#[path = "all/authorization.rs"]
mod authorization;
#[path = "all/route_claims.rs"]
mod route_claims;
#[path = "all/signaling_data_plane.rs"]
mod signaling_data_plane;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/wss_signaling_plane.rs"]
mod wss_signaling_plane;
