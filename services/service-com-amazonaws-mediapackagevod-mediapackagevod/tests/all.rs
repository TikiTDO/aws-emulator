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
#[path = "all/announced_egress_domain.rs"]
mod announced_egress_domain;
#[path = "all/arn_documented_members.rs"]
mod arn_documented_members;
#[path = "all/arn_member_validation.rs"]
mod arn_member_validation;
#[path = "all/asset_listing.rs"]
mod asset_listing;
#[path = "all/attached_asset_protection.rs"]
mod attached_asset_protection;
#[path = "all/configure_logs.rs"]
mod configure_logs;
#[path = "all/describe_packaging_configuration.rs"]
mod describe_packaging_configuration;
#[path = "all/egress_access_log_group_names.rs"]
mod egress_access_log_group_names;
#[path = "all/empty_response_framing.rs"]
mod empty_response_framing;
#[path = "all/error_responses.rs"]
mod error_responses;
#[path = "all/ingest_source_status.rs"]
mod ingest_source_status;
#[path = "all/manifest_list_required.rs"]
mod manifest_list_required;
#[path = "all/modelled_constraint_refusals.rs"]
mod modelled_constraint_refusals;
#[path = "all/packaging_configuration_deletion.rs"]
mod packaging_configuration_deletion;
#[path = "all/packaging_configuration_listing.rs"]
mod packaging_configuration_listing;
#[path = "all/packaging_group_authorization_arns.rs"]
mod packaging_group_authorization_arns;
#[path = "all/packaging_group_deletion.rs"]
mod packaging_group_deletion;
#[path = "all/packaging_group_update.rs"]
mod packaging_group_update;
#[path = "all/packaging_lifecycle.rs"]
mod packaging_lifecycle;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/resource_noun_in_refusals.rs"]
mod resource_noun_in_refusals;
#[path = "all/resource_tag_addition.rs"]
mod resource_tag_addition;
#[path = "all/resource_tag_listing.rs"]
mod resource_tag_listing;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/success_path_projections.rs"]
mod success_path_projections;
#[path = "all/tag_key_length_bounds.rs"]
mod tag_key_length_bounds;
#[path = "all/tag_restrictions.rs"]
mod tag_restrictions;
#[path = "all/unissued_pagination_cursor.rs"]
mod unissued_pagination_cursor;
