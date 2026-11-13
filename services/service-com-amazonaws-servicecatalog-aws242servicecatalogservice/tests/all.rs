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
#[path = "all/catalog_error_paths.rs"]
mod catalog_error_paths;
#[path = "all/catalog_resource_lifecycle.rs"]
mod catalog_resource_lifecycle;
#[path = "all/copy_lineage_and_lookup_messages.rs"]
mod copy_lineage_and_lookup_messages;
#[path = "all/end_user_product_view_messages.rs"]
mod end_user_product_view_messages;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/launch_paths.rs"]
mod launch_paths;
#[path = "all/portfolio_lifecycle.rs"]
mod portfolio_lifecycle;
#[path = "all/portfolio_principal_identifiers.rs"]
mod portfolio_principal_identifiers;
#[path = "all/portfolios_for_product.rs"]
mod portfolios_for_product;
#[path = "all/product_copies.rs"]
mod product_copies;
#[path = "all/provisioned_product_termination.rs"]
mod provisioned_product_termination;
#[path = "all/provisioned_products.rs"]
mod provisioned_products;
#[path = "all/provisioning_parameters.rs"]
mod provisioning_parameters;
#[path = "all/resources_for_tag_option.rs"]
mod resources_for_tag_option;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_option_associations.rs"]
mod tag_option_associations;
