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
#[path = "all/access_log_group_names.rs"]
mod access_log_group_names;
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/cmaf_child_manifest_ids_are_unique.rs"]
mod cmaf_child_manifest_ids_are_unique;
#[path = "all/cmaf_child_manifest_name.rs"]
mod cmaf_child_manifest_name;
#[path = "all/cmaf_child_manifest_name_follows_the_endpoint.rs"]
mod cmaf_child_manifest_name_follows_the_endpoint;
#[path = "all/created_initialization_vector.rs"]
mod created_initialization_vector;
#[path = "all/dash_live_manifest_defaults.rs"]
mod dash_live_manifest_defaults;
#[path = "all/data_plane.rs"]
mod data_plane;
#[path = "all/duration_floor_one_implementation.rs"]
mod duration_floor_one_implementation;
#[path = "all/empty_response_framing.rs"]
mod empty_response_framing;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/harvest_destination.rs"]
mod harvest_destination;
#[path = "all/harvest_export.rs"]
mod harvest_export;
#[path = "all/harvest_job_rejections.rs"]
mod harvest_job_rejections;
#[path = "all/harvest_jobs.rs"]
mod harvest_jobs;
#[path = "all/harvest_source_deleted.rs"]
mod harvest_source_deleted;
#[path = "all/harvest_source_window_closed.rs"]
mod harvest_source_window_closed;
#[path = "all/harvest_startover.rs"]
mod harvest_startover;
#[path = "all/ingest_credentials.rs"]
mod ingest_credentials;
#[path = "all/mutation_persistence.rs"]
mod mutation_persistence;
#[path = "all/mutually_exclusive_list_filters.rs"]
mod mutually_exclusive_list_filters;
#[path = "all/origin_endpoint_contents.rs"]
mod origin_endpoint_contents;
#[path = "all/origin_endpoint_defaults.rs"]
mod origin_endpoint_defaults;
#[path = "all/origin_endpoint_durations.rs"]
mod origin_endpoint_durations;
#[path = "all/origin_endpoint_packaging_bounds.rs"]
mod origin_endpoint_packaging_bounds;
#[path = "all/packaging_combination_rules.rs"]
mod packaging_combination_rules;
#[path = "all/page_token_authenticity.rs"]
mod page_token_authenticity;
#[path = "all/playback_path_is_one_rule.rs"]
mod playback_path_is_one_rule;
#[path = "all/refusal_order.rs"]
mod refusal_order;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/reserved_and_malformed_tags.rs"]
mod reserved_and_malformed_tags;
#[path = "all/resource_id_rule.rs"]
mod resource_id_rule;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rotate_channel_credentials.rs"]
mod rotate_channel_credentials;
#[path = "all/rotate_ingest_endpoint_credentials.rs"]
mod rotate_ingest_endpoint_credentials;
#[path = "all/startover_window_bound.rs"]
mod startover_window_bound;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_content_judged_before_the_lookup.rs"]
mod tag_content_judged_before_the_lookup;
#[path = "all/tag_key_length_is_one_rule.rs"]
mod tag_key_length_is_one_rule;
#[path = "all/tag_resolution_is_one_rule.rs"]
mod tag_resolution_is_one_rule;
