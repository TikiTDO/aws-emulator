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
#[path = "all/account_pool_listing.rs"]
mod account_pool_listing;
#[path = "all/account_pools.rs"]
mod account_pools;
#[path = "all/asset_deletion.rs"]
mod asset_deletion;
#[path = "all/asset_filters.rs"]
mod asset_filters;
#[path = "all/asset_lifecycle.rs"]
mod asset_lifecycle;
#[path = "all/asset_revision_listing.rs"]
mod asset_revision_listing;
#[path = "all/asset_revisions.rs"]
mod asset_revisions;
#[path = "all/asset_types.rs"]
mod asset_types;
#[path = "all/create_project_domain_resolution.rs"]
mod create_project_domain_resolution;
#[path = "all/delete_asset_denied_scope.rs"]
mod delete_asset_denied_scope;
#[path = "all/domain_name_conflict_and_not_found.rs"]
mod domain_name_conflict_and_not_found;
#[path = "all/domain_project_lifecycle.rs"]
mod domain_project_lifecycle;
#[path = "all/domain_scope_error_class.rs"]
mod domain_scope_error_class;
#[path = "all/domain_scoped_listing_authorization_order.rs"]
mod domain_scoped_listing_authorization_order;
#[path = "all/domain_scoped_read_authorization.rs"]
mod domain_scoped_read_authorization;
#[path = "all/domain_scoped_write_authorization.rs"]
mod domain_scoped_write_authorization;
#[path = "all/domain_unit_deletion.rs"]
mod domain_unit_deletion;
#[path = "all/domain_unit_hierarchy.rs"]
mod domain_unit_hierarchy;
#[path = "all/domain_unit_name_uniqueness.rs"]
mod domain_unit_name_uniqueness;
#[path = "all/domain_units_for_parent.rs"]
mod domain_units_for_parent;
#[path = "all/entity_owners.rs"]
mod entity_owners;
#[path = "all/failure_envelope.rs"]
mod failure_envelope;
#[path = "all/form_type_deletion.rs"]
mod form_type_deletion;
#[path = "all/form_type_revisions.rs"]
mod form_type_revisions;
#[path = "all/glossary_deletion.rs"]
mod glossary_deletion;
#[path = "all/glossary_lifecycle.rs"]
mod glossary_lifecycle;
#[path = "all/glossary_term_deletion.rs"]
mod glossary_term_deletion;
#[path = "all/glossary_terms.rs"]
mod glossary_terms;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/listing_change_set.rs"]
mod listing_change_set;
#[path = "all/policy_grants.rs"]
mod policy_grants;
#[path = "all/project_memberships.rs"]
mod project_memberships;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subscription_requests.rs"]
mod subscription_requests;
#[path = "all/update_glossary.rs"]
mod update_glossary;
#[path = "all/update_glossary_term.rs"]
mod update_glossary_term;
#[path = "all/update_project.rs"]
mod update_project;
#[path = "all/user_profiles.rs"]
mod user_profiles;
