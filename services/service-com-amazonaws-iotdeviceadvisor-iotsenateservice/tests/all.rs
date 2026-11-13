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
#[path = "all/announced_endpoint.rs"]
mod announced_endpoint;

#[path = "all/audit_trail.rs"]
mod audit_trail;

#[path = "all/child_resource_scope.rs"]
mod child_resource_scope;

#[path = "all/definition_configuration_arn_rules.rs"]
mod definition_configuration_arn_rules;

#[path = "all/definition_live_configuration.rs"]
mod definition_live_configuration;

#[path = "all/definition_versions.rs"]
mod definition_versions;

#[path = "all/device_identity_validation.rs"]
mod device_identity_validation;

#[path = "all/endpoint.rs"]
mod endpoint;

#[path = "all/page_token_positions.rs"]
mod page_token_positions;

#[path = "all/parallel_runs.rs"]
mod parallel_runs;

#[path = "all/refusal_message_bounds.rs"]
mod refusal_message_bounds;

#[path = "all/report_url_endpoint.rs"]
mod report_url_endpoint;

#[path = "all/resource_tagging.rs"]
mod resource_tagging;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/shared_paging_rule.rs"]
mod shared_paging_rule;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/stopped_run_timestamps.rs"]
mod stopped_run_timestamps;

#[path = "all/stored_configuration_and_tag_responses.rs"]
mod stored_configuration_and_tag_responses;

#[path = "all/suite_lifecycle.rs"]
mod suite_lifecycle;

#[path = "all/suite_run_report.rs"]
mod suite_run_report;

#[path = "all/suite_runs_listing_and_update.rs"]
mod suite_runs_listing_and_update;

#[path = "all/tag_merge_outcome.rs"]
mod tag_merge_outcome;
