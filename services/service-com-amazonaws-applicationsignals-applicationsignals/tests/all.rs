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
#[path = "all/account_scoped_reads.rs"]
mod account_scoped_reads;
#[path = "all/audit_findings.rs"]
mod audit_findings;
#[path = "all/batch_instrumentation_configuration_deletion.rs"]
mod batch_instrumentation_configuration_deletion;
#[path = "all/budget_report_error_entries.rs"]
mod budget_report_error_entries;
#[path = "all/budget_reports.rs"]
mod budget_reports;
#[path = "all/budget_status_agreement.rs"]
mod budget_status_agreement;
#[path = "all/change_event_attribution.rs"]
mod change_event_attribution;
#[path = "all/code_location_class_name.rs"]
mod code_location_class_name;
#[path = "all/defaulted_page_bound.rs"]
mod defaulted_page_bound;
#[path = "all/discovered_metric_references.rs"]
mod discovered_metric_references;
#[path = "all/discovery_enablement.rs"]
mod discovery_enablement;
#[path = "all/edge_end_identity_agreement.rs"]
mod edge_end_identity_agreement;
#[path = "all/entity_events.rs"]
mod entity_events;
#[path = "all/entity_identity.rs"]
mod entity_identity;
#[path = "all/entity_map_account_scope.rs"]
mod entity_map_account_scope;
#[path = "all/error_identity.rs"]
mod error_identity;
#[path = "all/exclusion_window_page_stability.rs"]
mod exclusion_window_page_stability;
#[path = "all/exclusion_window_reads.rs"]
mod exclusion_window_reads;
#[path = "all/exclusion_window_removal.rs"]
mod exclusion_window_removal;
#[path = "all/exclusion_window_start_time.rs"]
mod exclusion_window_start_time;
#[path = "all/exclusion_windows.rs"]
mod exclusion_windows;
#[path = "all/expires_at_lifetime_window.rs"]
mod expires_at_lifetime_window;
#[path = "all/get_instrumentation_configuration.rs"]
mod get_instrumentation_configuration;
#[path = "all/get_service.rs"]
mod get_service;
#[path = "all/grouping_configuration.rs"]
mod grouping_configuration;
#[path = "all/handler_enforced_constraint_wording.rs"]
mod handler_enforced_constraint_wording;
#[path = "all/instrumentation_capture_defaults.rs"]
mod instrumentation_capture_defaults;
#[path = "all/instrumentation_configuration_arn.rs"]
mod instrumentation_configuration_arn;
#[path = "all/instrumentation_configuration_deletion.rs"]
mod instrumentation_configuration_deletion;
#[path = "all/instrumentation_configuration_identity.rs"]
mod instrumentation_configuration_identity;
#[path = "all/instrumentation_configuration_removal.rs"]
mod instrumentation_configuration_removal;
#[path = "all/instrumentation_configuration_status.rs"]
mod instrumentation_configuration_status;
#[path = "all/instrumentation_configuration_tags.rs"]
mod instrumentation_configuration_tags;
#[path = "all/instrumentation_configurations.rs"]
mod instrumentation_configurations;
#[path = "all/instrumentation_status_minute.rs"]
mod instrumentation_status_minute;
#[path = "all/instrumentation_status_transitions.rs"]
mod instrumentation_status_transitions;
#[path = "all/instrumentation_status_wire_spelling.rs"]
mod instrumentation_status_wire_spelling;
#[path = "all/issued_key_boundary.rs"]
mod issued_key_boundary;
#[path = "all/issued_page_position.rs"]
mod issued_page_position;
#[path = "all/list_service_states.rs"]
mod list_service_states;
#[path = "all/list_services.rs"]
mod list_services;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/max_results_range.rs"]
mod max_results_range;
#[path = "all/observed_entity_identity_agreement.rs"]
mod observed_entity_identity_agreement;
#[path = "all/optional_page_bound.rs"]
mod optional_page_bound;
#[path = "all/page_bounds.rs"]
mod page_bounds;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/paged_cursor_selection_binding.rs"]
mod paged_cursor_selection_binding;
#[path = "all/paged_cursor_subject_binding.rs"]
mod paged_cursor_subject_binding;
#[path = "all/refusal_prose.rs"]
mod refusal_prose;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/resource_entity_metric_dimensions.rs"]
mod resource_entity_metric_dimensions;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_dependency_metrics.rs"]
mod service_dependency_metrics;
#[path = "all/service_groups.rs"]
mod service_groups;
#[path = "all/service_operation_metric_references.rs"]
mod service_operation_metric_references;
#[path = "all/service_state_latest_change.rs"]
mod service_state_latest_change;
#[path = "all/sli_metric_query.rs"]
mod sli_metric_query;
#[path = "all/sli_metric_type.rs"]
mod sli_metric_type;
#[path = "all/slo_evaluation_kind.rs"]
mod slo_evaluation_kind;
#[path = "all/slo_goal_retention.rs"]
mod slo_goal_retention;
#[path = "all/slo_id_account_scope.rs"]
mod slo_id_account_scope;
#[path = "all/slo_id_spelling_identity.rs"]
mod slo_id_spelling_identity;
#[path = "all/slo_ids_id_form.rs"]
mod slo_ids_id_form;
#[path = "all/slo_indicator_required.rs"]
mod slo_indicator_required;
#[path = "all/slo_lifecycle.rs"]
mod slo_lifecycle;
#[path = "all/slo_list_selection_binding.rs"]
mod slo_list_selection_binding;
#[path = "all/slo_metric_source_filter.rs"]
mod slo_metric_source_filter;
#[path = "all/slo_metric_source_kind.rs"]
mod slo_metric_source_kind;
#[path = "all/slo_writer_constraint_wording.rs"]
mod slo_writer_constraint_wording;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stated_instant_rendering.rs"]
mod stated_instant_rendering;
#[path = "all/status_event_default_window.rs"]
mod status_event_default_window;
#[path = "all/tag_attachment.rs"]
mod tag_attachment;
#[path = "all/tag_count_rule.rs"]
mod tag_count_rule;
#[path = "all/tagged_arn_identity.rs"]
mod tagged_arn_identity;
#[path = "all/time_ranged_reads.rs"]
mod time_ranged_reads;
#[path = "all/unissued_next_token.rs"]
mod unissued_next_token;
#[path = "all/untag_resource_kinds.rs"]
mod untag_resource_kinds;
#[path = "all/window_begun_one_decision.rs"]
mod window_begun_one_decision;
#[path = "all/window_instant_resolution.rs"]
mod window_instant_resolution;
#[path = "all/windowed_read_page_bound.rs"]
mod windowed_read_page_bound;
