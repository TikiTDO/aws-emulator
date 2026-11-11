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
#[path = "all/announced_hosting_address.rs"]
mod announced_hosting_address;
#[path = "all/app_branch_lifecycle.rs"]
mod app_branch_lifecycle;
#[path = "all/app_branch_projection.rs"]
mod app_branch_projection;
#[path = "all/app_branch_updates.rs"]
mod app_branch_updates;
#[path = "all/backend_environment_listing.rs"]
mod backend_environment_listing;
#[path = "all/backend_environments.rs"]
mod backend_environments;
#[path = "all/branch_deletion_visibility.rs"]
mod branch_deletion_visibility;
#[path = "all/branch_deployments.rs"]
mod branch_deployments;
#[path = "all/branch_duplicate_refusal.rs"]
mod branch_duplicate_refusal;
#[path = "all/branch_job_listing.rs"]
mod branch_job_listing;
#[path = "all/branch_job_stopping.rs"]
mod branch_job_stopping;
#[path = "all/branch_jobs.rs"]
mod branch_jobs;
#[path = "all/branch_revision_echo.rs"]
mod branch_revision_echo;
#[path = "all/deployment_upload_plane.rs"]
mod deployment_upload_plane;
#[path = "all/domain_association_certificate_lifecycle.rs"]
mod domain_association_certificate_lifecycle;
#[path = "all/domain_associations.rs"]
mod domain_associations;
#[path = "all/empty_response_status.rs"]
mod empty_response_status;
#[path = "all/hosted_branch_rewrites.rs"]
mod hosted_branch_rewrites;
#[path = "all/hosted_branch_site.rs"]
mod hosted_branch_site;
#[path = "all/hosted_site_access_control.rs"]
mod hosted_site_access_control;
#[path = "all/hosted_site_custom_headers.rs"]
mod hosted_site_custom_headers;
#[path = "all/hosted_site_custom_rules.rs"]
mod hosted_site_custom_rules;
#[path = "all/job_deletion.rs"]
mod job_deletion;
#[path = "all/job_stop_terminal_wording.rs"]
mod job_stop_terminal_wording;
#[path = "all/manual_deployment_preconditions.rs"]
mod manual_deployment_preconditions;
#[path = "all/request_refusal_wording.rs"]
mod request_refusal_wording;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/s3_sourced_deployments.rs"]
mod s3_sourced_deployments;
#[path = "all/start_job_repository_precondition.rs"]
mod start_job_repository_precondition;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/webhook_lifecycle.rs"]
mod webhook_lifecycle;
#[path = "all/webhook_listing.rs"]
mod webhook_listing;
#[path = "all/webhook_updates.rs"]
mod webhook_updates;
