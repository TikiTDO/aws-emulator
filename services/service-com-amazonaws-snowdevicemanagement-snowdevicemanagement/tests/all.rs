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
#[path = "all/account_device_gate.rs"]
mod account_device_gate;
#[path = "all/booted_inventory_precondition.rs"]
mod booted_inventory_precondition;
#[path = "all/cancel_task_dispatch_window.rs"]
mod cancel_task_dispatch_window;
#[path = "all/create_task_in_flight_quota.rs"]
mod create_task_in_flight_quota;
#[path = "all/create_task_tag_validation.rs"]
mod create_task_tag_validation;
#[path = "all/device_ec2_instance_cache.rs"]
mod device_ec2_instance_cache;
#[path = "all/device_ec2_instances.rs"]
mod device_ec2_instances;
#[path = "all/device_record_refresh_instant.rs"]
mod device_record_refresh_instant;
#[path = "all/device_region_gate.rs"]
mod device_region_gate;
#[path = "all/device_resources.rs"]
mod device_resources;
#[path = "all/device_task_lifecycle.rs"]
mod device_task_lifecycle;
#[path = "all/device_unlock_transient.rs"]
mod device_unlock_transient;
#[path = "all/empty_inventory_refusals.rs"]
mod empty_inventory_refusals;
#[path = "all/list_tasks_empty_page.rs"]
mod list_tasks_empty_page;
#[path = "all/one_execution_per_device.rs"]
mod one_execution_per_device;
#[path = "all/ordered_device_inventory.rs"]
mod ordered_device_inventory;
#[path = "all/pagination_cursor_binding.rs"]
mod pagination_cursor_binding;
#[path = "all/pagination_resume_position.rs"]
mod pagination_resume_position;
#[path = "all/reboot_cache_refresh_instant.rs"]
mod reboot_cache_refresh_instant;
#[path = "all/region_gate_error_envelope.rs"]
mod region_gate_error_envelope;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/sub_list_resolves_its_resource_first.rs"]
mod sub_list_resolves_its_resource_first;
#[path = "all/tagged_resource_tag_lookup.rs"]
mod tagged_resource_tag_lookup;
#[path = "all/tagging_region_scope.rs"]
mod tagging_region_scope;
#[path = "all/task_in_flight_predicate.rs"]
mod task_in_flight_predicate;
#[path = "all/task_terminal_completed_at.rs"]
mod task_terminal_completed_at;
#[path = "all/unknown_resource_id.rs"]
mod unknown_resource_id;
#[path = "all/untag_resource_key_validation.rs"]
mod untag_resource_key_validation;
