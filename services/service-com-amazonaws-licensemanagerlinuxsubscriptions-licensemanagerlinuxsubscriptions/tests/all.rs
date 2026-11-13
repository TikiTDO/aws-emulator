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
#[path = "all/discovered_fleet_selection.rs"]
mod discovered_fleet_selection;
#[path = "all/discovered_region_set.rs"]
mod discovered_region_set;
#[path = "all/discovery_from_ec2.rs"]
mod discovery_from_ec2;
#[path = "all/discovery_pass_settings_change.rs"]
mod discovery_pass_settings_change;
#[path = "all/discovery_resumes_where_it_stopped.rs"]
mod discovery_resumes_where_it_stopped;
#[path = "all/discovery_switch_off.rs"]
mod discovery_switch_off;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/error_taxonomy.rs"]
mod error_taxonomy;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/linux_subscriptions.rs"]
mod linux_subscriptions;
#[path = "all/page_token_length_bound.rs"]
mod page_token_length_bound;
#[path = "all/paged_walk_under_writes.rs"]
mod paged_walk_under_writes;
#[path = "all/provider_credential_validation.rs"]
mod provider_credential_validation;
#[path = "all/provider_status_transition.rs"]
mod provider_status_transition;
#[path = "all/provider_token_refusal_message.rs"]
mod provider_token_refusal_message;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_settings_status.rs"]
mod service_settings_status;
#[path = "all/shared_discovery_report.rs"]
mod shared_discovery_report;
#[path = "all/shared_harness.rs"]
mod shared_harness;
#[path = "all/shared_refusal_rendering.rs"]
mod shared_refusal_rendering;
#[path = "all/shared_tag_merge.rs"]
mod shared_tag_merge;
#[path = "all/shared_tag_projection.rs"]
mod shared_tag_projection;
#[path = "all/signed_region_catalog.rs"]
mod signed_region_catalog;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subscription_provider_registration.rs"]
mod subscription_provider_registration;
#[path = "all/subscription_providers.rs"]
mod subscription_providers;
#[path = "all/tag_api_refusal_code.rs"]
mod tag_api_refusal_code;
#[path = "all/tag_not_found_status.rs"]
mod tag_not_found_status;
#[path = "all/tag_plane_authorization.rs"]
mod tag_plane_authorization;
#[path = "all/tag_removal.rs"]
mod tag_removal;
#[path = "all/tag_selection_limit.rs"]
mod tag_selection_limit;
#[path = "all/walk_under_a_discovery_pass.rs"]
mod walk_under_a_discovery_pass;
