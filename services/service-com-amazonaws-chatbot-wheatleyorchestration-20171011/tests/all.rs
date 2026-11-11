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
#[path = "all/account_preferences.rs"]
mod account_preferences;
#[path = "all/associations.rs"]
mod associations;
#[path = "all/chime_webhook_configuration.rs"]
mod chime_webhook_configuration;
#[path = "all/chime_webhook_configuration_delete.rs"]
mod chime_webhook_configuration_delete;
#[path = "all/chime_webhook_configuration_delete_reach.rs"]
mod chime_webhook_configuration_delete_reach;
#[path = "all/chime_webhook_configuration_update.rs"]
mod chime_webhook_configuration_update;
#[path = "all/chime_webhook_configuration_update_refusals.rs"]
mod chime_webhook_configuration_update_refusals;
#[path = "all/custom_action.rs"]
mod custom_action;
#[path = "all/custom_action_authorization.rs"]
mod custom_action_authorization;
#[path = "all/custom_action_delete.rs"]
mod custom_action_delete;
#[path = "all/custom_action_fail_closed.rs"]
mod custom_action_fail_closed;
#[path = "all/custom_action_listing.rs"]
mod custom_action_listing;
#[path = "all/custom_action_listing_pagination.rs"]
mod custom_action_listing_pagination;
#[path = "all/custom_action_update.rs"]
mod custom_action_update;
#[path = "all/error_message_member.rs"]
mod error_message_member;
#[path = "all/error_response_contract.rs"]
mod error_response_contract;
#[path = "all/input_constraint_refusals.rs"]
mod input_constraint_refusals;
#[path = "all/list_associations_pagination.rs"]
mod list_associations_pagination;
#[path = "all/microsoft_teams_channel_configuration.rs"]
mod microsoft_teams_channel_configuration;
#[path = "all/microsoft_teams_configured_teams.rs"]
mod microsoft_teams_configured_teams;
#[path = "all/slack_channel_configuration.rs"]
mod slack_channel_configuration;
#[path = "all/slack_workspace_authorization.rs"]
mod slack_workspace_authorization;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/success_response_wire_shape.rs"]
mod success_response_wire_shape;
#[path = "all/tag_resource_cap.rs"]
mod tag_resource_cap;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/wire.rs"]
mod wire;
