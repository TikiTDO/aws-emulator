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
#[path = "all/accumulated_constraint_refusals.rs"]
mod accumulated_constraint_refusals;
#[path = "all/clock_sampled_under_the_store_lock.rs"]
mod clock_sampled_under_the_store_lock;
#[path = "all/constraint_refusal_reason.rs"]
mod constraint_refusal_reason;
#[path = "all/create_graph_using_import_task.rs"]
mod create_graph_using_import_task;
#[path = "all/documented_member_defaults.rs"]
mod documented_member_defaults;
#[path = "all/duplicate_name_conflicts.rs"]
mod duplicate_name_conflicts;
#[path = "all/duplicate_private_graph_endpoint_conflict.rs"]
mod duplicate_private_graph_endpoint_conflict;
#[path = "all/error_message_rendering.rs"]
mod error_message_rendering;
#[path = "all/export_task_cancellation.rs"]
mod export_task_cancellation;
#[path = "all/export_task_lifecycle.rs"]
mod export_task_lifecycle;
#[path = "all/export_task_listing.rs"]
mod export_task_listing;
#[path = "all/export_task_run.rs"]
mod export_task_run;
#[path = "all/final_snapshot_rule.rs"]
mod final_snapshot_rule;
#[path = "all/full_task_store_refusals.rs"]
mod full_task_store_refusals;
#[path = "all/graph_data_plane_gate.rs"]
mod graph_data_plane_gate;
#[path = "all/graph_deletion.rs"]
mod graph_deletion;
#[path = "all/graph_lifecycle.rs"]
mod graph_lifecycle;
#[path = "all/graph_record_encryption.rs"]
mod graph_record_encryption;
#[path = "all/graph_reset.rs"]
mod graph_reset;
#[path = "all/graph_restore_from_snapshot.rs"]
mod graph_restore_from_snapshot;
#[path = "all/graph_run_state.rs"]
mod graph_run_state;
#[path = "all/graph_snapshot_deletion.rs"]
mod graph_snapshot_deletion;
#[path = "all/graph_snapshot_lifecycle.rs"]
mod graph_snapshot_lifecycle;
#[path = "all/graph_snapshot_listing.rs"]
mod graph_snapshot_listing;
#[path = "all/graph_state_gate_messages.rs"]
mod graph_state_gate_messages;
#[path = "all/graph_summary.rs"]
mod graph_summary;
#[path = "all/import_requires_an_empty_graph.rs"]
mod import_requires_an_empty_graph;
#[path = "all/import_task_lifecycle.rs"]
mod import_task_lifecycle;
#[path = "all/import_task_listing.rs"]
mod import_task_listing;
#[path = "all/import_task_minting.rs"]
mod import_task_minting;
#[path = "all/import_task_read.rs"]
mod import_task_read;
#[path = "all/import_task_run.rs"]
mod import_task_run;
#[path = "all/listing_paging_rule.rs"]
mod listing_paging_rule;
#[path = "all/minted_snapshot_name_rule.rs"]
mod minted_snapshot_name_rule;
#[path = "all/page_token_listing_scope.rs"]
mod page_token_listing_scope;
#[path = "all/page_token_parent_scope.rs"]
mod page_token_parent_scope;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/pagination_token_codec.rs"]
mod pagination_token_codec;
#[path = "all/pass_role_authorization.rs"]
mod pass_role_authorization;
#[path = "all/pass_role_denial_principal.rs"]
mod pass_role_denial_principal;
#[path = "all/precondition_refusal_grammar.rs"]
mod precondition_refusal_grammar;
#[path = "all/private_graph_endpoint_lifecycle.rs"]
mod private_graph_endpoint_lifecycle;
#[path = "all/private_graph_endpoint_listing.rs"]
mod private_graph_endpoint_listing;
#[path = "all/private_graph_endpoint_read.rs"]
mod private_graph_endpoint_read;
#[path = "all/query_cancellation.rs"]
mod query_cancellation;
#[path = "all/query_listing.rs"]
mod query_listing;
#[path = "all/query_projection_parity.rs"]
mod query_projection_parity;
#[path = "all/query_status_reporting.rs"]
mod query_status_reporting;
#[path = "all/records_outlive_their_graph.rs"]
mod records_outlive_their_graph;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/restore_refusal_precedence.rs"]
mod restore_refusal_precedence;
#[path = "all/snapshot_of_a_graph_at_rest.rs"]
mod snapshot_of_a_graph_at_rest;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_ceiling_quota_refusals.rs"]
mod store_ceiling_quota_refusals;
#[path = "all/store_refusal_precedence.rs"]
mod store_refusal_precedence;
#[path = "all/tag_bound_refusal_wording.rs"]
mod tag_bound_refusal_wording;
#[path = "all/task_namespace_refusals.rs"]
mod task_namespace_refusals;
#[path = "all/unissued_page_token_refusal.rs"]
mod unissued_page_token_refusal;
#[path = "all/vector_search_dimension_rule.rs"]
mod vector_search_dimension_rule;
