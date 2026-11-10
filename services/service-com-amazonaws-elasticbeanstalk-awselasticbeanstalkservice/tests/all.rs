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
#[path = "all/abort_environment_update.rs"]
mod abort_environment_update;
#[path = "all/account_and_platform_catalog.rs"]
mod account_and_platform_catalog;
#[path = "all/application_version_processing.rs"]
mod application_version_processing;
#[path = "all/application_version_updates.rs"]
mod application_version_updates;
#[path = "all/application_versions.rs"]
mod application_versions;
#[path = "all/cname_reservation.rs"]
mod cname_reservation;
#[path = "all/configuration_options.rs"]
mod configuration_options;
#[path = "all/configuration_resolution.rs"]
mod configuration_resolution;
#[path = "all/configuration_set_deployment_status.rs"]
mod configuration_set_deployment_status;
#[path = "all/configuration_templates.rs"]
mod configuration_templates;
#[path = "all/create_platform_version_authorization.rs"]
mod create_platform_version_authorization;
#[path = "all/custom_platform_version_registration.rs"]
mod custom_platform_version_registration;
#[path = "all/custom_platform_version_tags.rs"]
mod custom_platform_version_tags;
#[path = "all/delete_configuration_template.rs"]
mod delete_configuration_template;
#[path = "all/delete_platform_version.rs"]
mod delete_platform_version;
#[path = "all/describe_application_versions.rs"]
mod describe_application_versions;
#[path = "all/dns_availability.rs"]
mod dns_availability;
#[path = "all/enhanced_health.rs"]
mod enhanced_health;
#[path = "all/environment_draft_configuration.rs"]
mod environment_draft_configuration;
#[path = "all/environment_event_notifications.rs"]
mod environment_event_notifications;
#[path = "all/environment_info.rs"]
mod environment_info;
#[path = "all/environment_info_request_shape.rs"]
mod environment_info_request_shape;
#[path = "all/environment_operations.rs"]
mod environment_operations;
#[path = "all/environment_resources.rs"]
mod environment_resources;
#[path = "all/environment_template_configuration.rs"]
mod environment_template_configuration;
#[path = "all/environment_tier_changes.rs"]
mod environment_tier_changes;
#[path = "all/events.rs"]
mod events;
// Not a suite of its own: the name the suites below import the shared harness
// by. Declared here because every file under `tests/all/` must be.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/lifecycle.rs"]
mod lifecycle;
#[path = "all/launch_configuration_options.rs"]
mod launch_configuration_options;
#[path = "all/managed_action_history.rs"]
mod managed_action_history;
#[path = "all/managed_actions.rs"]
mod managed_actions;
#[path = "all/managed_platform_catalog_rules.rs"]
mod managed_platform_catalog_rules;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/operations_role.rs"]
mod operations_role;
#[path = "all/platform_branch_catalog.rs"]
mod platform_branch_catalog;
#[path = "all/platform_branches.rs"]
mod platform_branches;
#[path = "all/platform_definition_bundle_build.rs"]
mod platform_definition_bundle_build;
#[path = "all/platform_update_availability.rs"]
mod platform_update_availability;
#[path = "all/platform_version_catalog_projection.rs"]
mod platform_version_catalog_projection;
#[path = "all/platform_versions.rs"]
mod platform_versions;
#[path = "all/published_solution_stack_validation.rs"]
mod published_solution_stack_validation;
#[path = "all/record_projections.rs"]
mod record_projections;
#[path = "all/recorded_wire_members.rs"]
mod recorded_wire_members;
#[path = "all/rejection_rules.rs"]
mod rejection_rules;
#[path = "all/reported_instant_precision.rs"]
mod reported_instant_precision;
#[path = "all/request_refusals.rs"]
mod request_refusals;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/solution_stacks.rs"]
mod solution_stacks;
#[path = "all/source_bundle_download.rs"]
mod source_bundle_download;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_location.rs"]
mod storage_location;
#[path = "all/swap_environment_cnames.rs"]
mod swap_environment_cnames;
#[path = "all/tag_batch_conflicts.rs"]
mod tag_batch_conflicts;
#[path = "all/tag_refusal_messages.rs"]
mod tag_refusal_messages;
#[path = "all/update_application.rs"]
mod update_application;
#[path = "all/update_application_lifecycle_projection.rs"]
mod update_application_lifecycle_projection;
#[path = "all/update_environment.rs"]
mod update_environment;
