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
#[path = "all/batch_item_error_messages.rs"]
mod batch_item_error_messages;
#[path = "all/date_time_wire_form.rs"]
mod date_time_wire_form;
#[path = "all/device_position_history.rs"]
mod device_position_history;
#[path = "all/geofence_collections.rs"]
mod geofence_collections;
#[path = "all/geofences.rs"]
mod geofences;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/listing_entry_members.rs"]
mod listing_entry_members;
#[path = "all/maps.rs"]
mod maps;
#[path = "all/resource_plane_fidelity.rs"]
mod resource_plane_fidelity;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/route_calculators.rs"]
mod route_calculators;
#[path = "all/state_memory_bounds.rs"]
mod state_memory_bounds;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/trackers.rs"]
mod trackers;
