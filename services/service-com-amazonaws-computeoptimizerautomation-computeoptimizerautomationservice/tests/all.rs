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
#[path = "all/account_association.rs"]
mod account_association;
#[path = "all/account_disassociation.rs"]
mod account_disassociation;
#[path = "all/account_id_list_rules.rs"]
mod account_id_list_rules;
#[path = "all/account_listing.rs"]
mod account_listing;
#[path = "all/automation_event_and_tagging.rs"]
mod automation_event_and_tagging;
#[path = "all/automation_event_completion.rs"]
mod automation_event_completion;
#[path = "all/automation_event_retrieval_window.rs"]
mod automation_event_retrieval_window;
#[path = "all/automation_event_rollback.rs"]
mod automation_event_rollback;
#[path = "all/automation_event_steps.rs"]
mod automation_event_steps;
#[path = "all/automation_event_summaries.rs"]
mod automation_event_summaries;
#[path = "all/automation_events.rs"]
mod automation_events;
#[path = "all/automation_rule_lifecycle.rs"]
mod automation_rule_lifecycle;
#[path = "all/automation_rule_preview.rs"]
mod automation_rule_preview;
#[path = "all/automation_rule_preview_summaries.rs"]
mod automation_rule_preview_summaries;
#[path = "all/automation_rule_validation.rs"]
mod automation_rule_validation;
#[path = "all/client_token_request_spellings.rs"]
mod client_token_request_spellings;
#[path = "all/cross_operation_page_tokens.rs"]
mod cross_operation_page_tokens;
#[path = "all/denial_in_every_wire_protocol.rs"]
mod denial_in_every_wire_protocol;
#[path = "all/denial_principal_agrees_with_sts.rs"]
mod denial_principal_agrees_with_sts;
#[path = "all/disassociated_member_initiation.rs"]
mod disassociated_member_initiation;
#[path = "all/disassociated_member_retrieval.rs"]
mod disassociated_member_retrieval;
#[path = "all/empty_value_lists.rs"]
mod empty_value_lists;
#[path = "all/enrollment_gate.rs"]
mod enrollment_gate;
#[path = "all/enrollment_rule_mode.rs"]
mod enrollment_rule_mode;
#[path = "all/enrollment_write_timestamps.rs"]
mod enrollment_write_timestamps;
#[path = "all/enum_parameter_rejection.rs"]
mod enum_parameter_rejection;
#[path = "all/error_payloads.rs"]
mod error_payloads;
#[path = "all/event_window_floor.rs"]
mod event_window_floor;
#[path = "all/filter_name_enum_boundary.rs"]
mod filter_name_enum_boundary;
#[path = "all/filter_name_refusals.rs"]
mod filter_name_refusals;
#[path = "all/filter_value_lists.rs"]
mod filter_value_lists;
#[path = "all/member_account_association_gate.rs"]
mod member_account_association_gate;
#[path = "all/member_account_initiation_gate.rs"]
mod member_account_initiation_gate;
#[path = "all/membership_page_token_spellings.rs"]
mod membership_page_token_spellings;
#[path = "all/not_found_wording.rs"]
mod not_found_wording;
#[path = "all/organization_configuration_rule_type.rs"]
mod organization_configuration_rule_type;
#[path = "all/organization_configuration_scope_is_typed.rs"]
mod organization_configuration_scope_is_typed;
#[path = "all/organization_rule_scope_gate.rs"]
mod organization_rule_scope_gate;
#[path = "all/page_cursor_spellings.rs"]
mod page_cursor_spellings;
#[path = "all/page_size_rule.rs"]
mod page_size_rule;
#[path = "all/page_token_refusals.rs"]
mod page_token_refusals;
#[path = "all/preview_page_token_selection.rs"]
mod preview_page_token_selection;
#[path = "all/preview_selection_guard.rs"]
mod preview_selection_guard;
#[path = "all/recommended_action_inventory.rs"]
mod recommended_action_inventory;
#[path = "all/recommended_action_summaries.rs"]
mod recommended_action_summaries;
#[path = "all/recommended_actions.rs"]
mod recommended_actions;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_arn_refusal_order.rs"]
mod resource_arn_refusal_order;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rule_change_stamps.rs"]
mod rule_change_stamps;
#[path = "all/rule_name_and_priority_rules.rs"]
mod rule_name_and_priority_rules;
#[path = "all/rule_projection.rs"]
mod rule_projection;
#[path = "all/rule_value_validation.rs"]
mod rule_value_validation;
#[path = "all/schedule_expression_forms.rs"]
mod schedule_expression_forms;
#[path = "all/shared_error_wording.rs"]
mod shared_error_wording;
#[path = "all/shared_page_walk.rs"]
mod shared_page_walk;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_lock_clock_reads.rs"]
mod store_lock_clock_reads;
#[path = "all/tag_list_rules.rs"]
mod tag_list_rules;
#[path = "all/tag_write_timestamps.rs"]
mod tag_write_timestamps;
#[path = "all/unauthorized_resource_message.rs"]
mod unauthorized_resource_message;
