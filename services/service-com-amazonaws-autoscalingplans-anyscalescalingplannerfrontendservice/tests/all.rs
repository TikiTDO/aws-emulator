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
#[path = "all/absent_plan_reads.rs"]
mod absent_plan_reads;
#[path = "all/application_source_exclusivity.rs"]
mod application_source_exclusivity;
#[path = "all/applied_policies_and_status_narratives.rs"]
mod applied_policies_and_status_narratives;
#[path = "all/aws_worded_client_refusals.rs"]
mod aws_worded_client_refusals;
#[path = "all/concurrent_update_refusal.rs"]
mod concurrent_update_refusal;
#[path = "all/customized_metric_statistic_rules.rs"]
mod customized_metric_statistic_rules;
#[path = "all/describe_scaling_plans_selector_bounds.rs"]
mod describe_scaling_plans_selector_bounds;
#[path = "all/describe_scaling_plans_version_member.rs"]
mod describe_scaling_plans_version_member;
#[path = "all/documented_entry_count_wording.rs"]
mod documented_entry_count_wording;
#[path = "all/documented_member_defaults.rs"]
mod documented_member_defaults;
#[path = "all/error_message_wording.rs"]
mod error_message_wording;
#[path = "all/forecast_data.rs"]
mod forecast_data;
#[path = "all/forecast_datapoints.rs"]
mod forecast_datapoints;
#[path = "all/forecast_resource_triple_consistency.rs"]
mod forecast_resource_triple_consistency;
#[path = "all/forecast_window_point_membership.rs"]
mod forecast_window_point_membership;
#[path = "all/forecast_window_precision.rs"]
mod forecast_window_precision;
#[path = "all/identity_key_separators.rs"]
mod identity_key_separators;
#[path = "all/mutating_plan_admission.rs"]
mod mutating_plan_admission;
#[path = "all/next_token_authenticity.rs"]
mod next_token_authenticity;
#[path = "all/off_enum_member_refusals.rs"]
mod off_enum_member_refusals;
#[path = "all/plan_version_member_validation.rs"]
mod plan_version_member_validation;
#[path = "all/plan_version_selects_a_plan.rs"]
mod plan_version_selects_a_plan;
#[path = "all/policy_name_identity.rs"]
mod policy_name_identity;
#[path = "all/predefined_metric_resource_label_identity.rs"]
mod predefined_metric_resource_label_identity;
#[path = "all/predictive_scaling_predicate.rs"]
mod predictive_scaling_predicate;
#[path = "all/resource_id_identifies_a_resource.rs"]
mod resource_id_identifies_a_resource;
#[path = "all/resource_identity_definition.rs"]
mod resource_identity_definition;
#[path = "all/resource_label_format.rs"]
mod resource_label_format;
#[path = "all/resource_narrative_matches_its_policies.rs"]
mod resource_narrative_matches_its_policies;
#[path = "all/scaling_instruction_validation.rs"]
mod scaling_instruction_validation;
#[path = "all/scaling_mode_conditional_members.rs"]
mod scaling_mode_conditional_members;
#[path = "all/scaling_plan_lifecycle.rs"]
mod scaling_plan_lifecycle;
#[path = "all/scaling_plan_resource_paging.rs"]
mod scaling_plan_resource_paging;
#[path = "all/scaling_plan_resource_policies.rs"]
mod scaling_plan_resource_policies;
#[path = "all/scheduled_action_buffer_time.rs"]
mod scheduled_action_buffer_time;
#[path = "all/scheduled_action_capacity_band.rs"]
mod scheduled_action_capacity_band;
#[path = "all/set_identity_multiplicity.rs"]
mod set_identity_multiplicity;
#[path = "all/shared_paged_walk.rs"]
mod shared_paged_walk;
#[path = "all/stack_arn_predicate_consistency.rs"]
mod stack_arn_predicate_consistency;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_filter_entry_bounds.rs"]
mod tag_filter_entry_bounds;
#[path = "all/tag_filter_names_a_tag.rs"]
mod tag_filter_names_a_tag;
#[path = "all/update_target_value_without_an_applied_policy.rs"]
mod update_target_value_without_an_applied_policy;
