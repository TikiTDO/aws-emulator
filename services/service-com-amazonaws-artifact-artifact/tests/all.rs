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
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/audit_capture.rs"]
mod audit_capture;
#[path = "all/compliance_inquiry_export.rs"]
mod compliance_inquiry_export;
#[path = "all/compliance_inquiry_export_url_resolves.rs"]
mod compliance_inquiry_export_url_resolves;
#[path = "all/compliance_inquiry_feedback.rs"]
mod compliance_inquiry_feedback;
#[path = "all/compliance_inquiry_file_submissions.rs"]
mod compliance_inquiry_file_submissions;
#[path = "all/compliance_inquiry_lifecycle.rs"]
mod compliance_inquiry_lifecycle;
#[path = "all/compliance_inquiry_listing.rs"]
mod compliance_inquiry_listing;
#[path = "all/compliance_inquiry_name_projection.rs"]
mod compliance_inquiry_name_projection;
#[path = "all/compliance_inquiry_queries.rs"]
mod compliance_inquiry_queries;
#[path = "all/compliance_inquiry_support_mode.rs"]
mod compliance_inquiry_support_mode;
#[path = "all/customer_agreements.rs"]
mod customer_agreements;
#[path = "all/document_data_plane.rs"]
mod document_data_plane;
#[path = "all/document_path_spelling.rs"]
mod document_path_spelling;
#[path = "all/document_plane_serving_rule.rs"]
mod document_plane_serving_rule;
#[path = "all/export_document_authorisation.rs"]
mod export_document_authorisation;
#[path = "all/front_door.rs"]
mod front_door;
#[path = "all/front_door_errors.rs"]
mod front_door_errors;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/implemented_inquiry_operations.rs"]
mod implemented_inquiry_operations;
#[path = "all/inquiry_last_modified.rs"]
mod inquiry_last_modified;
#[path = "all/inquiry_query_answers.rs"]
mod inquiry_query_answers;
#[path = "all/inquiry_query_identifier_rules.rs"]
mod inquiry_query_identifier_rules;
#[path = "all/inquiry_tags_reporting.rs"]
mod inquiry_tags_reporting;
#[path = "all/inquiry_target_resolution_rule.rs"]
mod inquiry_target_resolution_rule;
#[path = "all/inquiry_terminal_status_rule.rs"]
mod inquiry_terminal_status_rule;
#[path = "all/inquiry_write_audit.rs"]
mod inquiry_write_audit;
#[path = "all/minted_store_ids.rs"]
mod minted_store_ids;
#[path = "all/pagination_page_walk.rs"]
mod pagination_page_walk;
#[path = "all/pagination_token_opacity.rs"]
mod pagination_token_opacity;
#[path = "all/question_level_feedback_revision.rs"]
mod question_level_feedback_revision;
#[path = "all/report_acceptance_type.rs"]
mod report_acceptance_type;
#[path = "all/report_download_receipt_rule.rs"]
mod report_download_receipt_rule;
#[path = "all/report_projection.rs"]
mod report_projection;
#[path = "all/report_published_description.rs"]
mod report_published_description;
#[path = "all/report_target_resolution_rule.rs"]
mod report_target_resolution_rule;
#[path = "all/report_term_identity.rs"]
mod report_term_identity;
#[path = "all/report_workflow.rs"]
mod report_workflow;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tag_write_audit.rs"]
mod resource_tag_write_audit;
#[path = "all/resource_tag_writes.rs"]
mod resource_tag_writes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_stated_validation_envelope.rs"]
mod service_stated_validation_envelope;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_arn_syntax.rs"]
mod tagging_arn_syntax;
#[path = "all/target_authorized_before_absent_members.rs"]
mod target_authorized_before_absent_members;
#[path = "all/term_grant_receipt.rs"]
mod term_grant_receipt;
#[path = "all/timestamp_wire_form.rs"]
mod timestamp_wire_form;
#[path = "all/validation_exception_shape.rs"]
mod validation_exception_shape;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
