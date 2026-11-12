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
#[path = "all/app_registry_arn.rs"]
mod app_registry_arn;
#[path = "all/application_control_conflict.rs"]
mod application_control_conflict;
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;
#[path = "all/application_reference_refusals.rs"]
mod application_reference_refusals;
#[path = "all/application_refresh.rs"]
mod application_refresh;
#[path = "all/associated_applications.rs"]
mod associated_applications;
#[path = "all/component_arn_resolution.rs"]
mod component_arn_resolution;
#[path = "all/component_cross_references.rs"]
mod component_cross_references;
#[path = "all/component_discovery.rs"]
mod component_discovery;
#[path = "all/component_listing.rs"]
mod component_listing;
#[path = "all/component_reads.rs"]
mod component_reads;
#[path = "all/configuration_check_catalogue.rs"]
mod configuration_check_catalogue;
#[path = "all/configuration_check_reads.rs"]
mod configuration_check_reads;
#[path = "all/configuration_check_rules.rs"]
mod configuration_check_rules;
#[path = "all/configuration_checks.rs"]
mod configuration_checks;
#[path = "all/configuration_checks_in_flight.rs"]
mod configuration_checks_in_flight;
#[path = "all/database_arn_resource_kind.rs"]
mod database_arn_resource_kind;
#[path = "all/database_name_case.rs"]
mod database_name_case;
#[path = "all/delete_resource_permission.rs"]
mod delete_resource_permission;
#[path = "all/deregistration_window.rs"]
mod deregistration_window;
#[path = "all/ec2_instance_shutdown.rs"]
mod ec2_instance_shutdown;
#[path = "all/filter_length_bounds.rs"]
mod filter_length_bounds;
#[path = "all/filter_validation.rs"]
mod filter_validation;
#[path = "all/get_database.rs"]
mod get_database;
#[path = "all/list_configuration_check_definitions.rs"]
mod list_configuration_check_definitions;
#[path = "all/list_configuration_check_operations.rs"]
mod list_configuration_check_operations;
#[path = "all/list_databases.rs"]
mod list_databases;
#[path = "all/list_element_validation_messages.rs"]
mod list_element_validation_messages;
#[path = "all/list_operation_events.rs"]
mod list_operation_events;
#[path = "all/list_operations.rs"]
mod list_operations;
#[path = "all/not_found_error_families.rs"]
mod not_found_error_families;
#[path = "all/operation_event_resources.rs"]
mod operation_event_resources;
#[path = "all/page_token_scope.rs"]
mod page_token_scope;
#[path = "all/pattern_violation_messages.rs"]
mod pattern_violation_messages;
#[path = "all/register_application_conditional_requirements.rs"]
mod register_application_conditional_requirements;
#[path = "all/registered_credential_identity.rs"]
mod registered_credential_identity;
#[path = "all/registered_database_arn.rs"]
mod registered_database_arn;
#[path = "all/region_partition_reclamation.rs"]
mod region_partition_reclamation;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/registration_derived_state.rs"]
mod registration_derived_state;
#[path = "all/registration_instance_resolution.rs"]
mod registration_instance_resolution;
#[path = "all/registration_operation_properties.rs"]
mod registration_operation_properties;
#[path = "all/registration_outcome.rs"]
mod registration_outcome;
#[path = "all/registration_tag_ceiling.rs"]
mod registration_tag_ceiling;
#[path = "all/reregistered_application_history.rs"]
mod reregistered_application_history;
#[path = "all/resource_permission_reads.rs"]
mod resource_permission_reads;
#[path = "all/resource_permissions.rs"]
mod resource_permissions;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/settings_destroyed_databases.rs"]
mod settings_destroyed_databases;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_connected_entity.rs"]
mod stop_connected_entity;
#[path = "all/sub_check_result_cursor.rs"]
mod sub_check_result_cursor;
#[path = "all/sub_check_results.rs"]
mod sub_check_results;
#[path = "all/tag_key_form.rs"]
mod tag_key_form;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tagging_error_channel.rs"]
mod tagging_error_channel;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_application_settings.rs"]
mod update_application_settings;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
#[path = "all/validation_refusal_wording.rs"]
mod validation_refusal_wording;
