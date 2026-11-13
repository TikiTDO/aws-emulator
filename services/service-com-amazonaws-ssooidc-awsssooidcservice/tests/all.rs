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
#[path = "all/anonymous_routing.rs"]
mod anonymous_routing;
#[path = "all/aws_response_contract.rs"]
mod aws_response_contract;
#[path = "all/client_secret_binding.rs"]
mod client_secret_binding;
#[path = "all/create_token_grant_type.rs"]
mod create_token_grant_type;
#[path = "all/create_token_with_iam.rs"]
mod create_token_with_iam;
#[path = "all/create_token_with_iam_grant_type.rs"]
mod create_token_with_iam_grant_type;
#[path = "all/device_authorization_flow.rs"]
mod device_authorization_flow;
#[path = "all/device_code_approval.rs"]
mod device_code_approval;
#[path = "all/error_descriptions.rs"]
mod error_descriptions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
