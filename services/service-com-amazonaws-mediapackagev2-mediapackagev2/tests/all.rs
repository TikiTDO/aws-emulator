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
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/channel_listing.rs"]
mod channel_listing;
#[path = "all/channel_policy.rs"]
mod channel_policy;
#[path = "all/channel_policy_delete.rs"]
mod channel_policy_delete;
#[path = "all/channel_policy_lifecycle.rs"]
mod channel_policy_lifecycle;
#[path = "all/channel_reset.rs"]
mod channel_reset;
#[path = "all/channel_update.rs"]
mod channel_update;
#[path = "all/control_plane_validation.rs"]
mod control_plane_validation;
#[path = "all/data_plane_credentials.rs"]
mod data_plane_credentials;
#[path = "all/error_bodies.rs"]
mod error_bodies;
#[path = "all/origin_endpoint_policy.rs"]
mod origin_endpoint_policy;
#[path = "all/origin_endpoint_reset.rs"]
mod origin_endpoint_reset;
#[path = "all/refusal_text_and_entity_tags.rs"]
mod refusal_text_and_entity_tags;
#[path = "all/rejection_rule_and_negative_paths.rs"]
mod rejection_rule_and_negative_paths;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/server_materialised_members.rs"]
mod server_materialised_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_channel.rs"]
mod update_channel;
#[path = "all/update_origin_endpoint.rs"]
mod update_origin_endpoint;
#[path = "all/update_roundtrip.rs"]
mod update_roundtrip;

// Declared last, out of the sorted order above, on purpose: every new suite adds a line
// to this file, and a line inserted in the middle of the list conflicts with every other
// suite landing in the same window. Appending keeps this change out of that contended
// region. The suite itself is an ordinary `tests/all/<name>.rs` file.
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/published_data_plane_urls.rs"]
mod published_data_plane_urls;
#[path = "all/aws_refusal_sentences.rs"]
mod aws_refusal_sentences;
#[path = "all/published_data_plane_addressing.rs"]
mod published_data_plane_addressing;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/attached_multiview_channels.rs"]
mod attached_multiview_channels;
#[path = "all/delete_channel_multiview_source.rs"]
mod delete_channel_multiview_source;
#[path = "all/multiview_configuration_validation.rs"]
mod multiview_configuration_validation;
#[path = "all/regional_partitioning.rs"]
mod regional_partitioning;
#[path = "all/manifest_count_range.rs"]
mod manifest_count_range;
#[path = "all/caller_supplied_arns.rs"]
mod caller_supplied_arns;
#[path = "all/channel_reset_multiview.rs"]
mod channel_reset_multiview;
#[path = "all/harvest_job_refusal_text.rs"]
mod harvest_job_refusal_text;
#[path = "all/harvest_job_tagging.rs"]
mod harvest_job_tagging;
#[path = "all/shared_page_size.rs"]
mod shared_page_size;
#[path = "all/pagination_token_refusal.rs"]
mod pagination_token_refusal;
#[path = "all/harvest_job_idempotency.rs"]
mod harvest_job_idempotency;
#[path = "all/multiview_channel_ingest.rs"]
mod multiview_channel_ingest;
#[path = "all/quota_refusal_form.rs"]
mod quota_refusal_form;
#[path = "all/no_op_write_commits_nothing.rs"]
mod no_op_write_commits_nothing;
#[path = "all/shared_entity_tag_precondition.rs"]
mod shared_entity_tag_precondition;
#[path = "all/shared_manifest_count_builder.rs"]
mod shared_manifest_count_builder;
#[path = "all/create_idempotency_optional_members.rs"]
mod create_idempotency_optional_members;
#[path = "all/default_child_manifest_name.rs"]
mod default_child_manifest_name;
#[path = "all/harvest_job_name_namespace.rs"]
mod harvest_job_name_namespace;
#[path = "all/harvest_job_multiview_refusal.rs"]
mod harvest_job_multiview_refusal;
#[path = "all/inverted_deny_statements.rs"]
mod inverted_deny_statements;
#[path = "all/pagination_token_scope.rs"]
mod pagination_token_scope;
#[path = "all/segment_output_timestamp_mode.rs"]
mod segment_output_timestamp_mode;
#[path = "all/endpoint_member_input_type_restriction.rs"]
mod endpoint_member_input_type_restriction;
#[path = "all/harvest_job_scheduled_window.rs"]
mod harvest_job_scheduled_window;
#[path = "all/request_refusal_precedence.rs"]
mod request_refusal_precedence;
#[path = "all/restricted_channel_members.rs"]
mod restricted_channel_members;
#[path = "all/channel_members_by_input_type.rs"]
mod channel_members_by_input_type;
#[path = "all/entity_tag_precondition_precedence.rs"]
mod entity_tag_precondition_precedence;
#[path = "all/tagging_arn_refusal_parity.rs"]
mod tagging_arn_refusal_parity;
#[path = "all/channel_reset_removes_ingested_content.rs"]
mod channel_reset_removes_ingested_content;
#[path = "all/origin_endpoint_reset_content_scope.rs"]
mod origin_endpoint_reset_content_scope;
#[path = "all/harvest_job_modification_instant.rs"]
mod harvest_job_modification_instant;
#[path = "all/harvest_job_manifest_reference.rs"]
mod harvest_job_manifest_reference;
#[path = "all/tagging_arn_account_scope.rs"]
mod tagging_arn_account_scope;
#[path = "all/harvest_job_token_namespace.rs"]
mod harvest_job_token_namespace;
#[path = "all/origin_endpoint_encryption_settings.rs"]
mod origin_endpoint_encryption_settings;
#[path = "all/manifest_name_uniqueness.rs"]
mod manifest_name_uniqueness;
#[path = "all/endpoint_settings_by_input_type.rs"]
mod endpoint_settings_by_input_type;
#[path = "all/output_locking_mode_input_type.rs"]
mod output_locking_mode_input_type;
#[path = "all/multiview_epoch_locking.rs"]
mod multiview_epoch_locking;
#[path = "all/tag_key_character_bound.rs"]
mod tag_key_character_bound;
#[path = "all/sub_resource_content_scope.rs"]
mod sub_resource_content_scope;
#[path = "all/not_found_wording_per_kind.rs"]
mod not_found_wording_per_kind;
#[path = "all/empty_response_framing.rs"]
mod empty_response_framing;
#[path = "all/entity_tag_encoding.rs"]
mod entity_tag_encoding;
#[path = "all/ingest_endpoint_hosts.rs"]
mod ingest_endpoint_hosts;
#[path = "all/harvest_destination.rs"]
mod harvest_destination;
#[path = "all/delete_origin_endpoint_cascade.rs"]
mod delete_origin_endpoint_cascade;
#[path = "all/segment_service_applied_blocks.rs"]
mod segment_service_applied_blocks;
