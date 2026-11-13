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
#[path = "all/analyzable_servers.rs"]
mod analyzable_servers;

#[path = "all/application_component_config.rs"]
mod application_component_config;

#[path = "all/application_component_details.rs"]
mod application_component_details;

#[path = "all/application_component_listing.rs"]
mod application_component_listing;

#[path = "all/application_component_strategies.rs"]
mod application_component_strategies;

#[path = "all/assessment_analysis.rs"]
mod assessment_analysis;

#[path = "all/assessment_error_paths.rs"]
mod assessment_error_paths;

#[path = "all/assessment_server_status.rs"]
mod assessment_server_status;

#[path = "all/assessment_targets.rs"]
mod assessment_targets;

#[path = "all/clock_owed_before_a_config_update.rs"]
mod clock_owed_before_a_config_update;

#[path = "all/clock_owed_before_an_assessment_completion.rs"]
mod clock_owed_before_an_assessment_completion;

#[path = "all/clock_owed_before_an_inventory_count.rs"]
mod clock_owed_before_an_inventory_count;

#[path = "all/collector_inventory.rs"]
mod collector_inventory;

#[path = "all/declared_error_surface.rs"]
mod declared_error_surface;

#[path = "all/enum_membership.rs"]
mod enum_membership;

#[path = "all/enum_membership_other_operations.rs"]
mod enum_membership_other_operations;

#[path = "all/enum_support.rs"]
mod enum_support;

#[path = "all/id_pattern_probe.rs"]
mod id_pattern_probe;

#[path = "all/import_and_assess.rs"]
mod import_and_assess;

#[path = "all/import_file_task_listing.rs"]
mod import_file_task_listing;

#[path = "all/import_ingestion.rs"]
mod import_ingestion;

#[path = "all/import_status_report.rs"]
mod import_status_report;

#[path = "all/input_less_reads.rs"]
mod input_less_reads;

#[path = "all/page_size_bound.rs"]
mod page_size_bound;

#[path = "all/portfolio_preferences.rs"]
mod portfolio_preferences;

#[path = "all/portfolio_summary.rs"]
mod portfolio_summary;

#[path = "all/preferences_conflict.rs"]
mod preferences_conflict;

#[path = "all/recommendation_report.rs"]
mod recommendation_report;

#[path = "all/recommendation_report_file.rs"]
mod recommendation_report_file;

#[path = "all/refusal_messages.rs"]
mod refusal_messages;

#[path = "all/regional_isolation.rs"]
mod regional_isolation;

#[path = "all/report_generation_conflict.rs"]
mod report_generation_conflict;

#[path = "all/server_config.rs"]
mod server_config;

#[path = "all/server_details.rs"]
mod server_details;

#[path = "all/server_inventory.rs"]
mod server_inventory;

#[path = "all/server_strategies.rs"]
mod server_strategies;

#[path = "all/start_call_bucket_existence.rs"]
mod start_call_bucket_existence;

#[path = "all/start_call_bucket_reachability.rs"]
mod start_call_bucket_reachability;

#[path = "all/start_call_member_constraints.rs"]
mod start_call_member_constraints;

#[path = "all/start_call_preconditions.rs"]
mod start_call_preconditions;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/strategy_option_acceptance.rs"]
mod strategy_option_acceptance;

#[path = "all/whole_value_patterns.rs"]
mod whole_value_patterns;
