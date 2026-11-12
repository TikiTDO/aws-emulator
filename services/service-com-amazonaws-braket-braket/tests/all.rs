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
#[path = "all/arn_syntax.rs"]
mod arn_syntax;
#[path = "all/client_error_wording.rs"]
mod client_error_wording;
#[path = "all/device_capabilities.rs"]
mod device_capabilities;
#[path = "all/device_refusal_wording.rs"]
mod device_refusal_wording;
#[path = "all/execution_role_resolution.rs"]
mod execution_role_resolution;
#[path = "all/hybrid_job_loop.rs"]
mod hybrid_job_loop;
#[path = "all/quantum_task_loop.rs"]
mod quantum_task_loop;
#[path = "all/quantum_task_not_found_wording.rs"]
mod quantum_task_not_found_wording;
#[path = "all/request_body_rejection.rs"]
mod request_body_rejection;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/spending_limit_deletes.rs"]
mod spending_limit_deletes;
#[path = "all/spending_limit_updates.rs"]
mod spending_limit_updates;
#[path = "all/spending_limits.rs"]
mod spending_limits;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
