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
#[path = "all/kinesis_video_stream_pool_lifecycle.rs"]
mod kinesis_video_stream_pool_lifecycle;
#[path = "all/media_capture_pipeline_lifecycle.rs"]
mod media_capture_pipeline_lifecycle;
#[path = "all/media_insights_pipeline_configuration_lifecycle.rs"]
mod media_insights_pipeline_configuration_lifecycle;
#[path = "all/media_insights_pipeline_configuration_listing.rs"]
mod media_insights_pipeline_configuration_listing;
#[path = "all/media_insights_pipeline_configuration_update.rs"]
mod media_insights_pipeline_configuration_update;
#[path = "all/media_insights_pipeline_status.rs"]
mod media_insights_pipeline_status;
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/speaker_search_task.rs"]
mod speaker_search_task;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
