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
#[path = "all/header_block_ceiling.rs"]
mod header_block_ceiling;
#[path = "all/header_block_walk.rs"]
mod header_block_walk;
#[path = "all/mime_header_criteria.rs"]
mod mime_header_criteria;
#[path = "all/mime_header_field_position.rs"]
mod mime_header_field_position;
#[path = "all/mime_header_octets.rs"]
mod mime_header_octets;
#[path = "all/raw_message_content.rs"]
mod raw_message_content;
#[path = "all/regional_message_namespace.rs"]
mod regional_message_namespace;
#[path = "all/s3_object_resolution.rs"]
mod s3_object_resolution;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_access_record.rs"]
mod update_access_record;
#[path = "all/update_eligibility.rs"]
mod update_eligibility;
