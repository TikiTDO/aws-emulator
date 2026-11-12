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
#[path = "all/collaboration_configured_model_algorithm_associations.rs"]
mod collaboration_configured_model_algorithm_associations;
#[path = "all/collaboration_ml_input_channels.rs"]
mod collaboration_ml_input_channels;
#[path = "all/collaboration_trained_model_inference_jobs.rs"]
mod collaboration_trained_model_inference_jobs;
#[path = "all/collaboration_trained_models.rs"]
mod collaboration_trained_models;
#[path = "all/configured_model_algorithm.rs"]
mod configured_model_algorithm;
#[path = "all/configured_model_algorithm_associations.rs"]
mod configured_model_algorithm_associations;
#[path = "all/ops_name_bindings.rs"]
mod ops_name_bindings;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/training_dataset_audience_model_lifecycle.rs"]
mod training_dataset_audience_model_lifecycle;
#[path = "all/training_dataset_input_config_projection.rs"]
mod training_dataset_input_config_projection;
