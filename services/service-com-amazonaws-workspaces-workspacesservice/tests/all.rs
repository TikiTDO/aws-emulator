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
#[path = "all/account_byol_configuration.rs"]
mod account_byol_configuration;

#[path = "all/account_link_eligibility.rs"]
mod account_link_eligibility;

#[path = "all/account_link_error_messages.rs"]
mod account_link_error_messages;

#[path = "all/account_links.rs"]
mod account_links;

#[path = "all/batch_per_item_error_codes.rs"]
mod batch_per_item_error_codes;

#[path = "all/connection_alias_lifecycle.rs"]
mod connection_alias_lifecycle;

#[path = "all/create_workspaces_effective_properties.rs"]
mod create_workspaces_effective_properties;

#[path = "all/create_workspaces_validation.rs"]
mod create_workspaces_validation;

#[path = "all/ip_access_control_groups.rs"]
mod ip_access_control_groups;

#[path = "all/ip_group_and_alias_refusal_messages.rs"]
mod ip_group_and_alias_refusal_messages;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/workspace_bundles.rs"]
mod workspace_bundles;

#[path = "all/workspace_lifecycle.rs"]
mod workspace_lifecycle;

#[path = "all/workspace_properties_local_members.rs"]
mod workspace_properties_local_members;
