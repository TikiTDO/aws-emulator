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
#[path = "all/attribute_map_keys.rs"]
mod attribute_map_keys;
#[path = "all/buffered_requests_keep_what_was_submitted.rs"]
mod buffered_requests_keep_what_was_submitted;
#[path = "all/campaign_channel_subtype_config_deletion.rs"]
mod campaign_channel_subtype_config_deletion;
#[path = "all/campaign_channel_subtype_config_update.rs"]
mod campaign_channel_subtype_config_update;
#[path = "all/campaign_communication_limits.rs"]
mod campaign_communication_limits;
#[path = "all/campaign_communication_limits_deletion.rs"]
mod campaign_communication_limits_deletion;
#[path = "all/campaign_communication_time_deletion.rs"]
mod campaign_communication_time_deletion;
#[path = "all/campaign_communication_time_update.rs"]
mod campaign_communication_time_update;
#[path = "all/campaign_config_configuring_nothing.rs"]
mod campaign_config_configuring_nothing;
#[path = "all/campaign_configuration_is_one_rule.rs"]
mod campaign_configuration_is_one_rule;
#[path = "all/campaign_entry_limits.rs"]
mod campaign_entry_limits;
#[path = "all/campaign_entry_limits_deletion.rs"]
mod campaign_entry_limits_deletion;
#[path = "all/campaign_flow_association.rs"]
mod campaign_flow_association;
#[path = "all/campaign_lifecycle.rs"]
mod campaign_lifecycle;
#[path = "all/campaign_lifecycle_is_one_rule.rs"]
mod campaign_lifecycle_is_one_rule;
#[path = "all/campaign_name_uniqueness.rs"]
mod campaign_name_uniqueness;
#[path = "all/campaign_not_found_message.rs"]
mod campaign_not_found_message;
#[path = "all/campaign_rename.rs"]
mod campaign_rename;
#[path = "all/campaign_resume.rs"]
mod campaign_resume;
#[path = "all/campaign_schedule_completion.rs"]
mod campaign_schedule_completion;
#[path = "all/campaign_schedule_update.rs"]
mod campaign_schedule_update;
#[path = "all/campaign_source_update.rs"]
mod campaign_source_update;
#[path = "all/campaign_state_batch.rs"]
mod campaign_state_batch;
#[path = "all/campaign_stop.rs"]
mod campaign_stop;
#[path = "all/channel_subtype_config_is_one_rule.rs"]
mod channel_subtype_config_is_one_rule;
#[path = "all/common.rs"]
mod common;
#[path = "all/communication_limit_range.rs"]
mod communication_limit_range;
#[path = "all/communication_limits_refusal_wording.rs"]
mod communication_limits_refusal_wording;
#[path = "all/communication_time_window_constraints.rs"]
mod communication_time_window_constraints;
#[path = "all/configuration_deletion_writes_only_a_removal.rs"]
mod configuration_deletion_writes_only_a_removal;
#[path = "all/connect_instance_config_deletion.rs"]
mod connect_instance_config_deletion;
#[path = "all/connect_instance_integration_deletion.rs"]
mod connect_instance_integration_deletion;
#[path = "all/connect_instance_integration_list_paging.rs"]
mod connect_instance_integration_list_paging;
#[path = "all/connect_instance_integrations.rs"]
mod connect_instance_integrations;
#[path = "all/connect_instance_registry_gate.rs"]
mod connect_instance_registry_gate;
#[path = "all/cursor_paging_is_one_rule.rs"]
mod cursor_paging_is_one_rule;
#[path = "all/cursor_token_is_only_the_spelling_it_minted.rs"]
mod cursor_token_is_only_the_spelling_it_minted;
#[path = "all/cursor_tokens_are_scoped_to_their_listing.rs"]
mod cursor_tokens_are_scoped_to_their_listing;
#[path = "all/enum_catalog_is_one_rule.rs"]
mod enum_catalog_is_one_rule;
#[path = "all/instance_communication_limits.rs"]
mod instance_communication_limits;
#[path = "all/instance_communication_limits_isolation.rs"]
mod instance_communication_limits_isolation;
#[path = "all/instance_mutation_audit.rs"]
mod instance_mutation_audit;
#[path = "all/instance_mutation_audit_refusal_paths.rs"]
mod instance_mutation_audit_refusal_paths;
#[path = "all/instance_onboarding.rs"]
mod instance_onboarding;
#[path = "all/instance_onboarding_requires_a_connect_instance.rs"]
mod instance_onboarding_requires_a_connect_instance;
#[path = "all/instance_onboarding_tags_and_outbound_requests.rs"]
mod instance_onboarding_tags_and_outbound_requests;
#[path = "all/instance_refusal_messages.rs"]
mod instance_refusal_messages;
#[path = "all/integration_config_map_keys.rs"]
mod integration_config_map_keys;
#[path = "all/integration_identifier_precedes_lookup.rs"]
mod integration_identifier_precedes_lookup;
#[path = "all/integration_mutation_audit.rs"]
mod integration_mutation_audit;
#[path = "all/onboarding_job_deletion.rs"]
mod onboarding_job_deletion;
#[path = "all/onboarding_job_failure.rs"]
mod onboarding_job_failure;
#[path = "all/outbound_batch_is_one_rule.rs"]
mod outbound_batch_is_one_rule;
#[path = "all/outbound_request_expiration.rs"]
mod outbound_request_expiration;
#[path = "all/profile_outbound_requests.rs"]
mod profile_outbound_requests;
#[path = "all/profile_request_event_trigger_context.rs"]
mod profile_request_event_trigger_context;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/shared_connect_instance_gate.rs"]
mod shared_connect_instance_gate;
#[path = "all/signing_name_path_claim.rs"]
mod signing_name_path_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_refusal.rs"]
mod tag_key_refusal;
#[path = "all/tag_key_rules.rs"]
mod tag_key_rules;
#[path = "all/tag_removal.rs"]
mod tag_removal;
#[path = "all/union_member_arn_constraints.rs"]
mod union_member_arn_constraints;
