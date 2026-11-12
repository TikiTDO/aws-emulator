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
#[path = "all/backup_index_status_gate.rs"]
mod backup_index_status_gate;
#[path = "all/backup_plane_recovery_points.rs"]
mod backup_plane_recovery_points;
#[path = "all/cancel_window_outlasts_the_client_gap.rs"]
mod cancel_window_outlasts_the_client_gap;
#[path = "all/client_token_replay_rule.rs"]
mod client_token_replay_rule;
#[path = "all/client_token_window.rs"]
mod client_token_window;
#[path = "all/completed_with_issues_status_message.rs"]
mod completed_with_issues_status_message;
#[path = "all/empty_scope_filter_states_no_requirement.rs"]
mod empty_scope_filter_states_no_requirement;
#[path = "all/encryption_key_resolution.rs"]
mod encryption_key_resolution;
#[path = "all/export_client_token_expiry.rs"]
mod export_client_token_expiry;
#[path = "all/export_jobs.rs"]
mod export_jobs;
#[path = "all/export_listing.rs"]
mod export_listing;
#[path = "all/export_listing_order.rs"]
mod export_listing_order;
#[path = "all/export_refusal_classes.rs"]
mod export_refusal_classes;
#[path = "all/export_results_object.rs"]
mod export_results_object;
#[path = "all/iam_authorization.rs"]
mod iam_authorization;
#[path = "all/job_identifier_syntax.rs"]
mod job_identifier_syntax;
#[path = "all/non_canonical_page_token_spellings.rs"]
mod non_canonical_page_token_spellings;
#[path = "all/owed_status_due_instants.rs"]
mod owed_status_due_instants;
#[path = "all/owed_transition_settle_step.rs"]
mod owed_transition_settle_step;
#[path = "all/page_context_filter_values.rs"]
mod page_context_filter_values;
#[path = "all/page_token_under_a_changing_filter.rs"]
mod page_token_under_a_changing_filter;
#[path = "all/page_token_under_a_growing_sequence.rs"]
mod page_token_under_a_growing_sequence;
#[path = "all/relayed_kms_refusal.rs"]
mod relayed_kms_refusal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scan_run_outlasts_a_client_gap.rs"]
mod scan_run_outlasts_a_client_gap;
#[path = "all/search_job_item_results.rs"]
mod search_job_item_results;
#[path = "all/search_job_outcome_and_paging.rs"]
mod search_job_outcome_and_paging;
#[path = "all/search_job_status_phases.rs"]
mod search_job_status_phases;
#[path = "all/search_jobs.rs"]
mod search_jobs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_driven_search_job_lifecycle.rs"]
mod stop_driven_search_job_lifecycle;
#[path = "all/stop_search_job_cancel_window.rs"]
mod stop_search_job_cancel_window;
#[path = "all/stop_search_job_status_gate.rs"]
mod stop_search_job_status_gate;
#[path = "all/store_cap_rule.rs"]
mod store_cap_rule;
#[path = "all/success_status.rs"]
mod success_status;
#[path = "all/tag_admission_rule.rs"]
mod tag_admission_rule;
#[path = "all/tag_arn_resolution.rs"]
mod tag_arn_resolution;
#[path = "all/tag_character_set.rs"]
mod tag_character_set;
#[path = "all/tag_not_found_refusal.rs"]
mod tag_not_found_refusal;
#[path = "all/tag_removal.rs"]
mod tag_removal;
#[path = "all/transition_span_schedule.rs"]
mod transition_span_schedule;
