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
#[path = "all/array_jobs_and_placement.rs"]
mod array_jobs_and_placement;
#[path = "all/bounds.rs"]
mod bounds;
#[path = "all/cancel_job.rs"]
mod cancel_job;
#[path = "all/clock_owed_spans_are_per_resource.rs"]
mod clock_owed_spans_are_per_resource;
#[path = "all/compute_environment_update.rs"]
mod compute_environment_update;
#[path = "all/consumable_resource_drain.rs"]
mod consumable_resource_drain;
#[path = "all/consumable_resource_listing.rs"]
mod consumable_resource_listing;
#[path = "all/consumable_resource_release.rs"]
mod consumable_resource_release;
#[path = "all/consumable_resource_update_concurrency.rs"]
mod consumable_resource_update_concurrency;
#[path = "all/consumable_resource_updates.rs"]
mod consumable_resource_updates;
#[path = "all/consumable_resources.rs"]
mod consumable_resources;
#[path = "all/enum_membership_refusals.rs"]
mod enum_membership_refusals;
#[path = "all/job_attempt_records.rs"]
mod job_attempt_records;
#[path = "all/job_definition_deregistration.rs"]
mod job_definition_deregistration;
#[path = "all/job_definition_registry.rs"]
mod job_definition_registry;
#[path = "all/job_definition_response_request_id.rs"]
mod job_definition_response_request_id;
#[path = "all/job_definition_validation.rs"]
mod job_definition_validation;
#[path = "all/job_lifecycle.rs"]
mod job_lifecycle;
#[path = "all/job_phase_is_per_job.rs"]
mod job_phase_is_per_job;
#[path = "all/job_queue_snapshot.rs"]
mod job_queue_snapshot;
#[path = "all/job_queue_updates.rs"]
mod job_queue_updates;
#[path = "all/job_rests_in_runnable.rs"]
mod job_rests_in_runnable;
#[path = "all/job_state_change_events.rs"]
mod job_state_change_events;
#[path = "all/job_submission_publishes_no_event.rs"]
mod job_submission_publishes_no_event;
#[path = "all/jobs_by_consumable_resource.rs"]
mod jobs_by_consumable_resource;
#[path = "all/quota_share_delete_contract.rs"]
mod quota_share_delete_contract;
#[path = "all/quota_share_deletion.rs"]
mod quota_share_deletion;
#[path = "all/quota_share_listing.rs"]
mod quota_share_listing;
#[path = "all/quota_share_updates.rs"]
mod quota_share_updates;
#[path = "all/refusal_classification.rs"]
mod refusal_classification;
#[path = "all/refusal_contract.rs"]
mod refusal_contract;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_request_id.rs"]
mod response_request_id;
#[path = "all/scheduling_policies.rs"]
mod scheduling_policies;
#[path = "all/scheduling_policy_arn_shape_refusals.rs"]
mod scheduling_policy_arn_shape_refusals;
#[path = "all/scheduling_policy_listing.rs"]
mod scheduling_policy_listing;
#[path = "all/scheduling_policy_refusal_messages.rs"]
mod scheduling_policy_refusal_messages;
#[path = "all/scheduling_policy_teardown_and_conflicts.rs"]
mod scheduling_policy_teardown_and_conflicts;
#[path = "all/scheduling_policy_update_round_trip.rs"]
mod scheduling_policy_update_round_trip;
#[path = "all/service_environment.rs"]
mod service_environment;
#[path = "all/service_job_capacity_placement.rs"]
mod service_job_capacity_placement;
#[path = "all/service_job_listing.rs"]
mod service_job_listing;
#[path = "all/service_job_priority_updates.rs"]
mod service_job_priority_updates;
#[path = "all/service_job_submission.rs"]
mod service_job_submission;
#[path = "all/service_job_termination.rs"]
mod service_job_termination;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_mutation_no_content.rs"]
mod tag_mutation_no_content;
#[path = "all/tag_mutation_wire_probe.rs"]
mod tag_mutation_wire_probe;
#[path = "all/teardown_and_quota_shares.rs"]
mod teardown_and_quota_shares;
