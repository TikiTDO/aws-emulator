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
#[path = "all/app_assessment.rs"]
mod app_assessment;
#[path = "all/app_component_and_resource_identity.rs"]
mod app_component_and_resource_identity;
#[path = "all/app_template_document.rs"]
mod app_template_document;
#[path = "all/app_version_app_components.rs"]
mod app_version_app_components;
#[path = "all/app_version_resource_listing.rs"]
mod app_version_resource_listing;
#[path = "all/app_version_resource_resolution.rs"]
mod app_version_resource_resolution;
#[path = "all/app_version_resources.rs"]
mod app_version_resources;
#[path = "all/app_version_template.rs"]
mod app_version_template;
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;
#[path = "all/assessment_score_breakdown.rs"]
mod assessment_score_breakdown;
#[path = "all/list_app_versions.rs"]
mod list_app_versions;
#[path = "all/minted_identifier_shape.rs"]
mod minted_identifier_shape;
#[path = "all/publish_app_version.rs"]
mod publish_app_version;
#[path = "all/published_template_label_independence.rs"]
mod published_template_label_independence;
#[path = "all/published_version_alias_addressing.rs"]
mod published_version_alias_addressing;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/reported_output_members.rs"]
mod reported_output_members;
#[path = "all/resiliency_policy_deletion.rs"]
mod resiliency_policy_deletion;
#[path = "all/resiliency_policy_update.rs"]
mod resiliency_policy_update;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
