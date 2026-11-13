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
#[path = "all/account_notification_listing.rs"]
mod account_notification_listing;
#[path = "all/account_scoping.rs"]
mod account_scoping;
#[path = "all/action_arn_name_boundary.rs"]
mod action_arn_name_boundary;
#[path = "all/attached_notification_identity.rs"]
mod attached_notification_identity;
#[path = "all/budget_action_approval_gate.rs"]
mod budget_action_approval_gate;
#[path = "all/budget_action_definition_effect.rs"]
mod budget_action_definition_effect;
#[path = "all/budget_action_execution.rs"]
mod budget_action_execution;
#[path = "all/budget_action_execution_role.rs"]
mod budget_action_execution_role;
#[path = "all/budget_action_execution_role_trust.rs"]
mod budget_action_execution_role_trust;
#[path = "all/budget_action_history.rs"]
mod budget_action_history;
#[path = "all/budget_action_history_wording.rs"]
mod budget_action_history_wording;
#[path = "all/budget_action_lifecycle.rs"]
mod budget_action_lifecycle;
#[path = "all/budget_action_lock.rs"]
mod budget_action_lock;
#[path = "all/budget_action_read_purity.rs"]
mod budget_action_read_purity;
#[path = "all/budget_action_refusal_fidelity.rs"]
mod budget_action_refusal_fidelity;
#[path = "all/budget_action_role_account_scope.rs"]
mod budget_action_role_account_scope;
#[path = "all/budget_action_settle_history.rs"]
mod budget_action_settle_history;
#[path = "all/budget_action_subscriber_limits.rs"]
mod budget_action_subscriber_limits;
#[path = "all/budget_action_update_scope.rs"]
mod budget_action_update_scope;
#[path = "all/budget_actions_for_account.rs"]
mod budget_actions_for_account;
#[path = "all/budget_actions_for_budget.rs"]
mod budget_actions_for_budget;
#[path = "all/budget_cost_types_by_type.rs"]
mod budget_cost_types_by_type;
#[path = "all/budget_creation_limits.rs"]
mod budget_creation_limits;
#[path = "all/budget_end_date_expiry.rs"]
mod budget_end_date_expiry;
#[path = "all/budget_history_period_amounts.rs"]
mod budget_history_period_amounts;
#[path = "all/budget_lifecycle.rs"]
mod budget_lifecycle;
#[path = "all/budget_performance_history.rs"]
mod budget_performance_history;
#[path = "all/budget_read_projection.rs"]
mod budget_read_projection;
#[path = "all/budget_rule_conjunction.rs"]
mod budget_rule_conjunction;
#[path = "all/budget_stored_period_ordering.rs"]
mod budget_stored_period_ordering;
#[path = "all/budget_validation_rules.rs"]
mod budget_validation_rules;
#[path = "all/calculated_spend.rs"]
mod calculated_spend;
#[path = "all/client_error_envelopes.rs"]
mod client_error_envelopes;
#[path = "all/create_resource_tag_keys.rs"]
mod create_resource_tag_keys;
#[path = "all/created_action_trigger_status.rs"]
mod created_action_trigger_status;
#[path = "all/dead_notification_subscriber_reads.rs"]
mod dead_notification_subscriber_reads;
#[path = "all/default_page_sizes.rs"]
mod default_page_sizes;
#[path = "all/empty_collection_selections.rs"]
mod empty_collection_selections;
#[path = "all/error_message_fidelity.rs"]
mod error_message_fidelity;
#[path = "all/filter_expression_projection.rs"]
mod filter_expression_projection;
#[path = "all/notification_alarm_state.rs"]
mod notification_alarm_state;
#[path = "all/notification_alert_delivery.rs"]
mod notification_alert_delivery;
#[path = "all/notification_deletion.rs"]
mod notification_deletion;
#[path = "all/notification_lifecycle.rs"]
mod notification_lifecycle;
#[path = "all/notification_threshold_identity.rs"]
mod notification_threshold_identity;
#[path = "all/notification_update.rs"]
mod notification_update;
#[path = "all/page_size_resolution.rs"]
mod page_size_resolution;
#[path = "all/pagination_token_opacity.rs"]
mod pagination_token_opacity;
#[path = "all/per_notification_page_sizes.rs"]
mod per_notification_page_sizes;
#[path = "all/percentage_budget_limit_amount.rs"]
mod percentage_budget_limit_amount;
#[path = "all/request_borne_refusal_precedence.rs"]
mod request_borne_refusal_precedence;
#[path = "all/resource_tag_deletion.rs"]
mod resource_tag_deletion;
#[path = "all/resource_tag_reads.rs"]
mod resource_tag_reads;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_owned_budget_members.rs"]
mod service_owned_budget_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subscriber_address_validation.rs"]
mod subscriber_address_validation;
#[path = "all/subscriber_creation.rs"]
mod subscriber_creation;
#[path = "all/subscriber_deletion.rs"]
mod subscriber_deletion;
#[path = "all/subscriber_growth_address_rule.rs"]
mod subscriber_growth_address_rule;
#[path = "all/subscriber_notification_reads.rs"]
mod subscriber_notification_reads;
#[path = "all/subscriber_set_identity.rs"]
mod subscriber_set_identity;
#[path = "all/subscriber_sns_topic_region.rs"]
mod subscriber_sns_topic_region;
#[path = "all/subscriber_topic_account.rs"]
mod subscriber_topic_account;
#[path = "all/subscriber_update.rs"]
mod subscriber_update;
