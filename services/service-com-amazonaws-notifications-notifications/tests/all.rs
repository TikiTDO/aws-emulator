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
#[path = "all/channel_associations.rs"]
mod channel_associations;
#[path = "all/channel_not_found_message.rs"]
mod channel_not_found_message;
#[path = "all/configuration_status.rs"]
mod configuration_status;
#[path = "all/event_rule_deletes.rs"]
mod event_rule_deletes;
#[path = "all/event_rule_pair_uniqueness.rs"]
mod event_rule_pair_uniqueness;
#[path = "all/event_rule_updates.rs"]
mod event_rule_updates;
#[path = "all/event_rules.rs"]
mod event_rules;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/managed_catalog_membership.rs"]
mod managed_catalog_membership;
#[path = "all/managed_catalog_published_text.rs"]
mod managed_catalog_published_text;
#[path = "all/managed_notification_catalog.rs"]
mod managed_notification_catalog;
#[path = "all/managed_notification_configurations.rs"]
mod managed_notification_configurations;
#[path = "all/no_content_framing.rs"]
mod no_content_framing;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/notification_configurations.rs"]
mod notification_configurations;
#[path = "all/notification_events.rs"]
mod notification_events;
#[path = "all/notification_hubs.rs"]
mod notification_hubs;
#[path = "all/organization_access.rs"]
mod organization_access;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_mutation_status.rs"]
mod tag_mutation_status;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
