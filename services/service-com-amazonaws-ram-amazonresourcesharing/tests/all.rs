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
#[path = "all/aws_managed_permission_catalog.rs"]
mod aws_managed_permission_catalog;
#[path = "all/client_token_mismatch_wording.rs"]
mod client_token_mismatch_wording;
#[path = "all/document_entries_and_removal_reports.rs"]
mod document_entries_and_removal_reports;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/invitation_reads.rs"]
mod invitation_reads;
#[path = "all/managed_permissions.rs"]
mod managed_permissions;
#[path = "all/mutation_reports.rs"]
mod mutation_reports;
#[path = "all/pending_invitation_resources.rs"]
mod pending_invitation_resources;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_shares.rs"]
mod resource_shares;
#[path = "all/response_payload_fidelity.rs"]
mod response_payload_fidelity;
#[path = "all/share_permission_attachment.rs"]
mod share_permission_attachment;
#[path = "all/share_permissions.rs"]
mod share_permissions;
#[path = "all/share_tags_and_configuration.rs"]
mod share_tags_and_configuration;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
