// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/app_block_builder.rs"]
mod app_block_builder;
#[path = "all/app_blocks.rs"]
mod app_blocks;
#[path = "all/applications.rs"]
mod applications;
#[path = "all/associated_fleets.rs"]
mod associated_fleets;
#[path = "all/associated_stacks.rs"]
mod associated_stacks;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/stack_fleet_lifecycle.rs"]
mod stack_fleet_lifecycle;
#[path = "all/stack_registry.rs"]
mod stack_registry;
#[path = "all/stack_theme.rs"]
mod stack_theme;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/streaming_url.rs"]
mod streaming_url;
#[path = "all/update_stack.rs"]
mod update_stack;
#[path = "all/usage_report_subscription.rs"]
mod usage_report_subscription;
#[path = "all/user_pool.rs"]
mod user_pool;
#[path = "all/user_stack_associations.rs"]
mod user_stack_associations;
#[path = "all/images.rs"]
mod images;
