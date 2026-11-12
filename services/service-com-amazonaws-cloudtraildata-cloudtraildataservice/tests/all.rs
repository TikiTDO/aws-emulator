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
#[path = "all/batch_length_clause_value.rs"]
mod batch_length_clause_value;
#[path = "all/channel_arn_refusals.rs"]
mod channel_arn_refusals;
#[path = "all/channel_arn_syntax.rs"]
mod channel_arn_syntax;
#[path = "all/clause_value_slot.rs"]
mod clause_value_slot;
#[path = "all/constraint_refusals.rs"]
mod constraint_refusals;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/put_audit_events.rs"]
mod put_audit_events;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
