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
#[path = "all/alias_creation_reference_precedence.rs"]
mod alias_creation_reference_precedence;
#[path = "all/alias_name_prefix.rs"]
mod alias_name_prefix;
#[path = "all/alias_name_prefix_precedence.rs"]
mod alias_name_prefix_precedence;
#[path = "all/alias_region_scope.rs"]
mod alias_region_scope;
#[path = "all/batch_get_policy.rs"]
mod batch_get_policy;
#[path = "all/batch_is_authorized.rs"]
mod batch_is_authorized;
#[path = "all/batch_is_authorized_with_token.rs"]
mod batch_is_authorized_with_token;
#[path = "all/create_time_tag_limit.rs"]
mod create_time_tag_limit;
#[path = "all/date_time_wire_format.rs"]
mod date_time_wire_format;
#[path = "all/delete_policy_idempotency.rs"]
mod delete_policy_idempotency;
#[path = "all/delete_policy_template.rs"]
mod delete_policy_template;
#[path = "all/entities_exclude_action_entities.rs"]
mod entities_exclude_action_entities;
#[path = "all/entity_list_refusal_pointers.rs"]
mod entity_list_refusal_pointers;
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/evaluation_errors.rs"]
mod evaluation_errors;
#[path = "all/get_policy_store_tag_presence.rs"]
mod get_policy_store_tag_presence;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/idempotency_parameter_escaping.rs"]
mod idempotency_parameter_escaping;
#[path = "all/identity_source_configuration_writers.rs"]
mod identity_source_configuration_writers;
#[path = "all/identity_source_group_entity_type.rs"]
mod identity_source_group_entity_type;
#[path = "all/identity_source_pool_refusal_message.rs"]
mod identity_source_pool_refusal_message;
#[path = "all/identity_source_pool_refusal_stages.rs"]
mod identity_source_pool_refusal_stages;
#[path = "all/identity_source_principal_entity_type.rs"]
mod identity_source_principal_entity_type;
#[path = "all/identity_source_refusal_pointers.rs"]
mod identity_source_refusal_pointers;
#[path = "all/identity_source_user_pool_existence.rs"]
mod identity_source_user_pool_existence;
#[path = "all/identity_sources.rs"]
mod identity_sources;
#[path = "all/is_authorized_with_token.rs"]
mod is_authorized_with_token;
#[path = "all/list_cursor_refusal_follows_store_lookup.rs"]
mod list_cursor_refusal_follows_store_lookup;
#[path = "all/list_identity_sources.rs"]
mod list_identity_sources;
#[path = "all/list_pagination_rule.rs"]
mod list_pagination_rule;
#[path = "all/list_policies_template_reference_filter.rs"]
mod list_policies_template_reference_filter;
#[path = "all/list_policy_store_aliases.rs"]
mod list_policy_store_aliases;
#[path = "all/list_policy_stores.rs"]
mod list_policy_stores;
#[path = "all/list_policy_templates.rs"]
mod list_policy_templates;
#[path = "all/name_conflict_precedes_capacity.rs"]
mod name_conflict_precedes_capacity;
#[path = "all/not_found_precedes_payload_refusal.rs"]
mod not_found_precedes_payload_refusal;
#[path = "all/oidc_identity_source_authorization.rs"]
mod oidc_identity_source_authorization;
#[path = "all/oidc_token_audience_claim.rs"]
mod oidc_token_audience_claim;
#[path = "all/policy_store_alias.rs"]
mod policy_store_alias;
#[path = "all/policy_store_authorization.rs"]
mod policy_store_authorization;
#[path = "all/policy_store_encryption_settings.rs"]
mod policy_store_encryption_settings;
#[path = "all/policy_store_id_only_references.rs"]
mod policy_store_id_only_references;
#[path = "all/policy_store_record_stamp.rs"]
mod policy_store_record_stamp;
#[path = "all/policy_store_settings.rs"]
mod policy_store_settings;
#[path = "all/policy_template_name_assignment.rs"]
mod policy_template_name_assignment;
#[path = "all/policy_templates.rs"]
mod policy_templates;
#[path = "all/policy_update.rs"]
mod policy_update;
#[path = "all/policy_validation.rs"]
mod policy_validation;
#[path = "all/policy_writer_reference_precedence.rs"]
mod policy_writer_reference_precedence;
#[path = "all/repeated_entity_identifiers.rs"]
mod repeated_entity_identifiers;
#[path = "all/resource_names.rs"]
mod resource_names;
#[path = "all/response_fidelity.rs"]
mod response_fidelity;
#[path = "all/schema.rs"]
mod schema;
#[path = "all/schema_deletion.rs"]
mod schema_deletion;
#[path = "all/schema_lifecycle.rs"]
mod schema_lifecycle;
#[path = "all/scope_entity_type_immutability.rs"]
mod scope_entity_type_immutability;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_validation_settings.rs"]
mod store_validation_settings;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/template_linked_policies.rs"]
mod template_linked_policies;
#[path = "all/template_placeholder_immutability.rs"]
mod template_placeholder_immutability;
#[path = "all/update_and_delete_lifecycle.rs"]
mod update_and_delete_lifecycle;
#[path = "all/validation_exception_contents.rs"]
mod validation_exception_contents;
#[path = "all/wire_fidelity.rs"]
mod wire_fidelity;
