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
#[path = "all/baseline_catalog.rs"]
mod baseline_catalog;
#[path = "all/baseline_operations.rs"]
mod baseline_operations;
#[path = "all/catalog_identifier_equivalence.rs"]
mod catalog_identifier_equivalence;
#[path = "all/control_operations.rs"]
mod control_operations;
#[path = "all/delete_landing_zone_idempotence.rs"]
mod delete_landing_zone_idempotence;
#[path = "all/disable_baseline.rs"]
mod disable_baseline;
#[path = "all/disable_control.rs"]
mod disable_control;
#[path = "all/disable_control_target_reference.rs"]
mod disable_control_target_reference;
#[path = "all/drift_summary_projection.rs"]
mod drift_summary_projection;
#[path = "all/enable_baseline_persistence.rs"]
mod enable_baseline_persistence;
#[path = "all/enable_baseline_validation.rs"]
mod enable_baseline_validation;
#[path = "all/enable_control_persistence.rs"]
mod enable_control_persistence;
#[path = "all/enable_control_validation.rs"]
mod enable_control_validation;
#[path = "all/enabled_baseline_drift.rs"]
mod enabled_baseline_drift;
#[path = "all/enabled_baseline_pending_transitions.rs"]
mod enabled_baseline_pending_transitions;
#[path = "all/enabled_baseline_reset.rs"]
mod enabled_baseline_reset;
#[path = "all/enabled_baseline_update.rs"]
mod enabled_baseline_update;
#[path = "all/enabled_baseline_update_validation.rs"]
mod enabled_baseline_update_validation;
#[path = "all/enabled_control_lifecycle.rs"]
mod enabled_control_lifecycle;
#[path = "all/enabled_control_pending_transitions.rs"]
mod enabled_control_pending_transitions;
#[path = "all/enabled_control_reset.rs"]
mod enabled_control_reset;
#[path = "all/enabled_control_target_regions.rs"]
mod enabled_control_target_regions;
#[path = "all/enabled_control_update.rs"]
mod enabled_control_update;
#[path = "all/enablement_write_precondition.rs"]
mod enablement_write_precondition;
#[path = "all/first_deployment_drift.rs"]
mod first_deployment_drift;
#[path = "all/front_door_validation.rs"]
mod front_door_validation;
#[path = "all/get_control_operation.rs"]
mod get_control_operation;
#[path = "all/get_enabled_baseline.rs"]
mod get_enabled_baseline;
#[path = "all/get_enabled_control.rs"]
mod get_enabled_control;
#[path = "all/governed_target_mandatory_controls.rs"]
mod governed_target_mandatory_controls;
#[path = "all/in_flight_configuration_change_drift.rs"]
mod in_flight_configuration_change_drift;
#[path = "all/in_flight_control_change_reporting.rs"]
mod in_flight_control_change_reporting;
#[path = "all/in_flight_control_drift_reporting.rs"]
mod in_flight_control_drift_reporting;
#[path = "all/in_flight_teardown_drift.rs"]
mod in_flight_teardown_drift;
#[path = "all/inherited_enabled_baselines.rs"]
mod inherited_enabled_baselines;
#[path = "all/inherited_enabled_controls.rs"]
mod inherited_enabled_controls;
#[path = "all/landing_zone_decommission.rs"]
mod landing_zone_decommission;
#[path = "all/landing_zone_decommission_cascade.rs"]
mod landing_zone_decommission_cascade;
#[path = "all/landing_zone_identifier.rs"]
mod landing_zone_identifier;
#[path = "all/landing_zone_lifecycle.rs"]
mod landing_zone_lifecycle;
#[path = "all/landing_zone_operations.rs"]
mod landing_zone_operations;
#[path = "all/landing_zone_pending_transitions.rs"]
mod landing_zone_pending_transitions;
#[path = "all/landing_zone_precondition.rs"]
mod landing_zone_precondition;
#[path = "all/landing_zone_reset.rs"]
mod landing_zone_reset;
#[path = "all/landing_zone_update.rs"]
mod landing_zone_update;
#[path = "all/landing_zone_update_refusal_order.rs"]
mod landing_zone_update_refusal_order;
#[path = "all/landing_zone_validation.rs"]
mod landing_zone_validation;
#[path = "all/landing_zone_version_order.rs"]
mod landing_zone_version_order;
#[path = "all/list_baselines.rs"]
mod list_baselines;
#[path = "all/list_enabled_baselines.rs"]
mod list_enabled_baselines;
#[path = "all/list_filter_narrowing.rs"]
mod list_filter_narrowing;
#[path = "all/list_pagination.rs"]
mod list_pagination;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/mandatory_control_is_the_services.rs"]
mod mandatory_control_is_the_services;
#[path = "all/manifest_region_names.rs"]
mod manifest_region_names;
#[path = "all/minted_arn_region_segment.rs"]
mod minted_arn_region_segment;
#[path = "all/non_canonical_page_token_spellings.rs"]
mod non_canonical_page_token_spellings;
#[path = "all/one_enabled_baseline_per_target.rs"]
mod one_enabled_baseline_per_target;
#[path = "all/one_enabled_control_per_target.rs"]
mod one_enabled_control_per_target;
#[path = "all/operation_detail_availability_window.rs"]
mod operation_detail_availability_window;
#[path = "all/page_token_listing_scope.rs"]
mod page_token_listing_scope;
#[path = "all/published_baseline_catalog.rs"]
mod published_baseline_catalog;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_identifier_form.rs"]
mod resource_identifier_form;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/unknown_landing_zone_identifier.rs"]
mod unknown_landing_zone_identifier;
#[path = "all/update_refusal_order.rs"]
mod update_refusal_order;
