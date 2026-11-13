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
#[path = "all/advertised_pattern_expressions.rs"]
mod advertised_pattern_expressions;
#[path = "all/announced_media_type.rs"]
mod announced_media_type;
#[path = "all/app_monitor_data.rs"]
mod app_monitor_data;
#[path = "all/app_monitor_data_paging.rs"]
mod app_monitor_data_paging;
#[path = "all/app_monitor_lifecycle.rs"]
mod app_monitor_lifecycle;
#[path = "all/authorization.rs"]
mod authorization;
#[path = "all/batch_create_metric_definitions_echo.rs"]
mod batch_create_metric_definitions_echo;
#[path = "all/batch_create_metric_definitions_targeting.rs"]
mod batch_create_metric_definitions_targeting;
#[path = "all/batch_delete_metric_definitions.rs"]
mod batch_delete_metric_definitions;
#[path = "all/batch_get_metric_definitions_reads.rs"]
mod batch_get_metric_definitions_reads;
#[path = "all/constraint_refusals.rs"]
mod constraint_refusals;
#[path = "all/create_app_monitor_tag_key_wording.rs"]
mod create_app_monitor_tag_key_wording;
#[path = "all/create_app_monitor_tags.rs"]
mod create_app_monitor_tags;
#[path = "all/custom_metric_event_fields.rs"]
mod custom_metric_event_fields;
#[path = "all/cw_log_delivery.rs"]
mod cw_log_delivery;
#[path = "all/cw_log_event_copies.rs"]
mod cw_log_event_copies;
#[path = "all/data_plane_denial_identifier.rs"]
mod data_plane_denial_identifier;
#[path = "all/deleted_app_monitor_addressing.rs"]
mod deleted_app_monitor_addressing;
#[path = "all/deobfuscation_source_maps.rs"]
mod deobfuscation_source_maps;
#[path = "all/destination_scoped_definition_members.rs"]
mod destination_scoped_definition_members;
#[path = "all/dimension_keys_constraints.rs"]
mod dimension_keys_constraints;
#[path = "all/established_generated_values.rs"]
mod established_generated_values;
#[path = "all/event_instant_round_trip.rs"]
mod event_instant_round_trip;
#[path = "all/extended_metric_dimension_entries.rs"]
mod extended_metric_dimension_entries;
#[path = "all/iam_denial_wording.rs"]
mod iam_denial_wording;
#[path = "all/identity_policy_sources.rs"]
mod identity_policy_sources;
#[path = "all/list_app_monitors_page_size.rs"]
mod list_app_monitors_page_size;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/member_check_precedence.rs"]
mod member_check_precedence;
#[path = "all/metric_definition_answers.rs"]
mod metric_definition_answers;
#[path = "all/metric_definition_conflicts.rs"]
mod metric_definition_conflicts;
#[path = "all/metric_definition_declared_members.rs"]
mod metric_definition_declared_members;
#[path = "all/metric_definition_identity.rs"]
mod metric_definition_identity;
#[path = "all/metric_definition_listing_order.rs"]
mod metric_definition_listing_order;
#[path = "all/metric_definition_validation.rs"]
mod metric_definition_validation;
#[path = "all/metric_definitions.rs"]
mod metric_definitions;
#[path = "all/metric_destination_summary_members.rs"]
mod metric_destination_summary_members;
#[path = "all/metrics_destination_pass_role.rs"]
mod metrics_destination_pass_role;
#[path = "all/metrics_destination_removal.rs"]
mod metrics_destination_removal;
#[path = "all/metrics_destinations.rs"]
mod metrics_destinations;
#[path = "all/mixed_constraint_envelope.rs"]
mod mixed_constraint_envelope;
#[path = "all/nested_configuration_defaults.rs"]
mod nested_configuration_defaults;
#[path = "all/not_found_and_conflict_bodies.rs"]
mod not_found_and_conflict_bodies;
#[path = "all/pagination_token_check_precedence.rs"]
mod pagination_token_check_precedence;
#[path = "all/pagination_token_region_scope.rs"]
mod pagination_token_region_scope;
#[path = "all/pagination_token_spelling.rs"]
mod pagination_token_spelling;
#[path = "all/platform_domain_authority.rs"]
mod platform_domain_authority;
#[path = "all/policy_revision_refusal_wording.rs"]
mod policy_revision_refusal_wording;
#[path = "all/put_rum_events_unresolvable_id.rs"]
mod put_rum_events_unresolvable_id;
#[path = "all/query_bound_required_members.rs"]
mod query_bound_required_members;
#[path = "all/query_window_bounds.rs"]
mod query_window_bounds;
#[path = "all/refused_create_side_effects.rs"]
mod refused_create_side_effects;
#[path = "all/region_scoped_app_monitors.rs"]
mod region_scoped_app_monitors;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rum_event_documents.rs"]
mod rum_event_documents;
#[path = "all/shared_destination_arn_rule.rs"]
mod shared_destination_arn_rule;
#[path = "all/shared_page_size_rule.rs"]
mod shared_page_size_rule;
#[path = "all/shared_tagging_refusal_rule.rs"]
mod shared_tagging_refusal_rule;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/tag_key_clauses.rs"]
mod tag_key_clauses;
#[path = "all/tagging_authorization.rs"]
mod tagging_authorization;
#[path = "all/tagging_frontend_media_type.rs"]
mod tagging_frontend_media_type;
#[path = "all/tagging_no_content.rs"]
mod tagging_no_content;
#[path = "all/vended_cw_log_group.rs"]
mod vended_cw_log_group;
