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
#[path = "all/active_version_derivation.rs"]
mod active_version_derivation;
#[path = "all/arn_validation_errors.rs"]
mod arn_validation_errors;
#[path = "all/child_listing_absent_parent_walk.rs"]
mod child_listing_absent_parent_walk;
#[path = "all/child_listing_cursor_scope.rs"]
mod child_listing_cursor_scope;
#[path = "all/child_listing_error_set.rs"]
mod child_listing_error_set;
#[path = "all/clock_sampled_under_the_store_lock.rs"]
mod clock_sampled_under_the_store_lock;
#[path = "all/code_snapshot_instant.rs"]
mod code_snapshot_instant;
#[path = "all/create_replay_after_later_calls.rs"]
mod create_replay_after_later_calls;
#[path = "all/cron_field_values.rs"]
mod cron_field_values;
#[path = "all/definition_declared_schedule_trigger.rs"]
mod definition_declared_schedule_trigger;
#[path = "all/deleted_workflow_run_cleanup.rs"]
mod deleted_workflow_run_cleanup;
#[path = "all/idempotency_token_ledger.rs"]
mod idempotency_token_ledger;
#[path = "all/list_page_size.rs"]
mod list_page_size;
#[path = "all/missing_resource_lookups.rs"]
mod missing_resource_lookups;
#[path = "all/not_found_resource_identity.rs"]
mod not_found_resource_identity;
#[path = "all/override_parameter_fingerprint.rs"]
mod override_parameter_fingerprint;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/resource_tag_listing.rs"]
mod resource_tag_listing;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/run_execution_outcome.rs"]
mod run_execution_outcome;
#[path = "all/run_lifecycle_progression.rs"]
mod run_lifecycle_progression;
#[path = "all/run_listing_version_filter_cursor_scope.rs"]
mod run_listing_version_filter_cursor_scope;
#[path = "all/run_log_export.rs"]
mod run_log_export;
#[path = "all/run_report_agreement.rs"]
mod run_report_agreement;
#[path = "all/run_stop.rs"]
mod run_stop;
#[path = "all/run_task_instances.rs"]
mod run_task_instances;
#[path = "all/run_version_task_graph.rs"]
mod run_version_task_graph;
#[path = "all/scheduled_workflow_runs.rs"]
mod scheduled_workflow_runs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_constraints.rs"]
mod tag_key_constraints;
#[path = "all/tag_writes_while_deleting.rs"]
mod tag_writes_while_deleting;
#[path = "all/task_instance_listing.rs"]
mod task_instance_listing;
#[path = "all/task_instance_reads.rs"]
mod task_instance_reads;
#[path = "all/task_operator_reporting.rs"]
mod task_operator_reporting;
#[path = "all/teardown_refusal_wording.rs"]
mod teardown_refusal_wording;
#[path = "all/update_configuration_dependencies.rs"]
mod update_configuration_dependencies;
#[path = "all/validation_errors.rs"]
mod validation_errors;
#[path = "all/workflow_configuration_digest.rs"]
mod workflow_configuration_digest;
#[path = "all/workflow_definition_snapshot.rs"]
mod workflow_definition_snapshot;
#[path = "all/workflow_deletion_teardown.rs"]
mod workflow_deletion_teardown;
#[path = "all/workflow_digest_member_boundaries.rs"]
mod workflow_digest_member_boundaries;
#[path = "all/workflow_lifecycle.rs"]
mod workflow_lifecycle;
#[path = "all/workflow_listing_cursor_scope.rs"]
mod workflow_listing_cursor_scope;
#[path = "all/workflow_run_listing.rs"]
mod workflow_run_listing;
#[path = "all/workflow_run_listing_order.rs"]
mod workflow_run_listing_order;
#[path = "all/workflow_update_versions.rs"]
mod workflow_update_versions;
#[path = "all/workflow_version_cursor_continuity.rs"]
mod workflow_version_cursor_continuity;
#[path = "all/workflow_version_listing.rs"]
mod workflow_version_listing;
#[path = "all/workflow_version_promotion.rs"]
mod workflow_version_promotion;
