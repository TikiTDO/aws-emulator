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
#[path = "all/assume_role_for_pod_identity.rs"]
mod assume_role_for_pod_identity;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/rejection_errors.rs"]
mod rejection_errors;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/token_claims_are_never_read.rs"]
mod token_claims_are_never_read;
#[path = "all/token_header_layers.rs"]
mod token_header_layers;
