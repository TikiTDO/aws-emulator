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
#[path = "all/ca_arn_resolution_stages.rs"]
mod ca_arn_resolution_stages;
#[path = "all/ca_certificate_import.rs"]
mod ca_certificate_import;
#[path = "all/certificate_authority_audit_reports.rs"]
mod certificate_authority_audit_reports;
#[path = "all/certificate_authority_key_material.rs"]
mod certificate_authority_key_material;
#[path = "all/certificate_authority_listing.rs"]
mod certificate_authority_listing;
#[path = "all/certificate_authority_tags.rs"]
mod certificate_authority_tags;
#[path = "all/certificate_issuance_rules.rs"]
mod certificate_issuance_rules;
#[path = "all/certificate_revocation.rs"]
mod certificate_revocation;
#[path = "all/constraint_refusal_messages.rs"]
mod constraint_refusal_messages;
#[path = "all/create_idempotency.rs"]
mod create_idempotency;
#[path = "all/crl_bucket_resolution.rs"]
mod crl_bucket_resolution;
#[path = "all/crl_publication.rs"]
mod crl_publication;
#[path = "all/delete_certificate_authority.rs"]
mod delete_certificate_authority;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/issuance_metrics.rs"]
mod issuance_metrics;
#[path = "all/issued_certificate_reads.rs"]
mod issued_certificate_reads;
#[path = "all/permission_action_set.rs"]
mod permission_action_set;
#[path = "all/permissions.rs"]
mod permissions;
#[path = "all/policy_documents.rs"]
mod policy_documents;
#[path = "all/private_ca_lifecycle.rs"]
mod private_ca_lifecycle;
#[path = "all/refusal_message_text.rs"]
mod refusal_message_text;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_policy_read.rs"]
mod resource_policy_read;
#[path = "all/resource_policy_rules.rs"]
mod resource_policy_rules;
#[path = "all/restore_certificate_authority.rs"]
mod restore_certificate_authority;
#[path = "all/root_ca_self_signing.rs"]
mod root_ca_self_signing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_listing.rs"]
mod tag_listing;
#[path = "all/update_certificate_authority.rs"]
mod update_certificate_authority;
#[path = "all/validation_stage_refusals.rs"]
mod validation_stage_refusals;
