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
#[path = "all/absent_group_refusal_messages.rs"]
mod absent_group_refusal_messages;
#[path = "all/account_level_reads_publish_service_data.rs"]
mod account_level_reads_publish_service_data;
#[path = "all/account_wide_read_authorization.rs"]
mod account_wide_read_authorization;
#[path = "all/auto_scaling_group_lifecycle.rs"]
mod auto_scaling_group_lifecycle;
#[path = "all/cancel_instance_refresh.rs"]
mod cancel_instance_refresh;
#[path = "all/client_error_refusal_wording.rs"]
mod client_error_refusal_wording;
#[path = "all/describe_tags.rs"]
mod describe_tags;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/instance_lifecycle_events.rs"]
mod instance_lifecycle_events;
#[path = "all/instance_refresh_lifecycle.rs"]
mod instance_refresh_lifecycle;
#[path = "all/instance_scale_in_protection.rs"]
mod instance_scale_in_protection;
#[path = "all/launch_configurations_are_unavailable.rs"]
mod launch_configurations_are_unavailable;
#[path = "all/launch_template_instance_profiles.rs"]
mod launch_template_instance_profiles;
#[path = "all/launch_template_specifications.rs"]
mod launch_template_specifications;
#[path = "all/legacy_target_group_detach.rs"]
mod legacy_target_group_detach;
#[path = "all/metric_collection_catalog.rs"]
mod metric_collection_catalog;
#[path = "all/reference_data_catalogs.rs"]
mod reference_data_catalogs;
#[path = "all/rollback_instance_refresh.rs"]
mod rollback_instance_refresh;
#[path = "all/scale_out_launches_ec2_instances.rs"]
mod scale_out_launches_ec2_instances;
#[path = "all/scaling_activity_log.rs"]
mod scaling_activity_log;
#[path = "all/set_desired_capacity_on_subnet_placed_groups.rs"]
mod set_desired_capacity_on_subnet_placed_groups;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// The shared harness the suites above drive the service through: the awsQuery POST
// and the XML readers, defined once.
#[path = "all/support.rs"]
mod support;
#[path = "all/vpc_zone_identifier_zones.rs"]
mod vpc_zone_identifier_zones;
