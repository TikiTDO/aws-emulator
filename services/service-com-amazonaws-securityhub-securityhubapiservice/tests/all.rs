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
// Declared ahead of the sorted list on purpose: every other securityhub change edits
// that list, and a suite declared here conflicts with none of them.
#[path = "all/insight_lifecycle.rs"]
mod insight_lifecycle;
// Declared here for the same reason: a new suite conflicts with no other change.
#[path = "all/member_deletion.rs"]
mod member_deletion;
// And this one, for the same reason.
#[path = "all/error_envelope.rs"]
mod error_envelope;
// Declared here for the same reason: a new suite conflicts with no other change.
#[path = "all/member_listing.rs"]
mod member_listing;
// And this one, for the same reason.
#[path = "all/member_unknown_accounts.rs"]
mod member_unknown_accounts;
// And this one, for the same reason.
#[path = "all/standards_controls.rs"]
mod standards_controls;
// And this one, for the same reason.
#[path = "all/security_control_definitions.rs"]
mod security_control_definitions;
// And this one, for the same reason.
#[path = "all/batch_disable_standards.rs"]
mod batch_disable_standards;
#[path = "all/action_targets.rs"]
mod action_targets;
#[path = "all/automation_rules.rs"]
mod automation_rules;
#[path = "all/batch_enable_standards.rs"]
mod batch_enable_standards;
#[path = "all/describe_standards.rs"]
mod describe_standards;
#[path = "all/enabled_standards.rs"]
mod enabled_standards;
#[path = "all/finding_aggregator_lifecycle.rs"]
mod finding_aggregator_lifecycle;
#[path = "all/finding_history.rs"]
mod finding_history;
#[path = "all/finding_ingest_lifecycle.rs"]
mod finding_ingest_lifecycle;
#[path = "all/member_lifecycle.rs"]
mod member_lifecycle;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/security_hub_core_loop.rs"]
mod security_hub_core_loop;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/timestamp_wire_form.rs"]
mod timestamp_wire_form;
#[path = "all/update_findings.rs"]
mod update_findings;
