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
#[path = "all/batch_aggregate_refusal_reaches_the_operation.rs"]
mod batch_aggregate_refusal_reaches_the_operation;
#[path = "all/batch_entry_faults.rs"]
mod batch_entry_faults;
#[path = "all/batch_payload_ceiling.rs"]
mod batch_payload_ceiling;
#[path = "all/cancel_message_move_task.rs"]
mod cancel_message_move_task;
#[path = "all/dead_letter_drain_cycle.rs"]
mod dead_letter_drain_cycle;
#[path = "all/dead_letter_instants.rs"]
mod dead_letter_instants;
#[path = "all/dead_letter_move_on_visibility_change.rs"]
mod dead_letter_move_on_visibility_change;
#[path = "all/dead_letter_redrive.rs"]
mod dead_letter_redrive;
#[path = "all/dead_letter_source_attribution.rs"]
mod dead_letter_source_attribution;
#[path = "all/delete_message_batch.rs"]
mod delete_message_batch;
#[path = "all/fifo_group_rotation.rs"]
mod fifo_group_rotation;
#[path = "all/fifo_queue.rs"]
mod fifo_queue;
#[path = "all/fifo_queue_designation.rs"]
mod fifo_queue_designation;
#[path = "all/fifo_receive_idempotency.rs"]
mod fifo_receive_idempotency;
#[path = "all/generated_queue_policy_document.rs"]
mod generated_queue_policy_document;
#[path = "all/list_dead_letter_source_queues.rs"]
mod list_dead_letter_source_queues;
#[path = "all/list_message_move_tasks.rs"]
mod list_message_move_tasks;
#[path = "all/message_attribute_rules.rs"]
mod message_attribute_rules;
#[path = "all/message_lifecycle.rs"]
mod message_lifecycle;
#[path = "all/message_move_task.rs"]
mod message_move_task;
#[path = "all/message_move_task_queue_type.rs"]
mod message_move_task_queue_type;
#[path = "all/message_plane.rs"]
mod message_plane;
#[path = "all/message_visibility_changes.rs"]
mod message_visibility_changes;
#[path = "all/opaque_response_tokens.rs"]
mod opaque_response_tokens;
#[path = "all/pagination_token_refusal.rs"]
mod pagination_token_refusal;
#[path = "all/policy_statement_limit.rs"]
mod policy_statement_limit;
#[path = "all/purge_queue.rs"]
mod purge_queue;
#[path = "all/queue_configuration.rs"]
mod queue_configuration;
#[path = "all/queue_depth_metrics.rs"]
mod queue_depth_metrics;
#[path = "all/queue_does_not_exist.rs"]
mod queue_does_not_exist;
#[path = "all/queue_encryption.rs"]
mod queue_encryption;
#[path = "all/queue_lifecycle.rs"]
mod queue_lifecycle;
#[path = "all/queue_permissions.rs"]
mod queue_permissions;
#[path = "all/queue_policy_authorization.rs"]
mod queue_policy_authorization;
#[path = "all/queue_policy_condition_keys.rs"]
mod queue_policy_condition_keys;
#[path = "all/queue_tags.rs"]
mod queue_tags;
#[path = "all/queue_type_immutability.rs"]
mod queue_type_immutability;
#[path = "all/queue_url_endpoint_form.rs"]
mod queue_url_endpoint_form;
#[path = "all/query_compatible_error_envelope.rs"]
mod query_compatible_error_envelope;
#[path = "all/receipt_handle_leases.rs"]
mod receipt_handle_leases;
#[path = "all/receipt_handle_lifetime.rs"]
mod receipt_handle_lifetime;
#[path = "all/receipt_handle_provenance.rs"]
mod receipt_handle_provenance;
#[path = "all/receive_message_attribute_names.rs"]
mod receive_message_attribute_names;
#[path = "all/recreate_and_tag_rules.rs"]
mod recreate_and_tag_rules;
#[path = "all/redrive_allow_policy.rs"]
mod redrive_allow_policy;
#[path = "all/redrive_allow_policy_document_rules.rs"]
mod redrive_allow_policy_document_rules;
#[path = "all/redrive_dlq_type_reason.rs"]
mod redrive_dlq_type_reason;
#[path = "all/redrive_policy_document.rs"]
mod redrive_policy_document;
#[path = "all/redrive_region_reasons.rs"]
mod redrive_region_reasons;
#[path = "all/refusal_message_reasons.rs"]
mod refusal_message_reasons;
#[path = "all/refusal_taxonomy.rs"]
mod refusal_taxonomy;
#[path = "all/regional_queue_namespace.rs"]
mod regional_queue_namespace;
#[path = "all/request_size_transport_bound.rs"]
mod request_size_transport_bound;
#[path = "all/scratch_probe.rs"]
mod scratch_probe;
#[path = "all/send_message_batch.rs"]
mod send_message_batch;
#[path = "all/send_message_size_refusals.rs"]
mod send_message_size_refusals;
#[path = "all/sender_attribution.rs"]
mod sender_attribution;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/time_driven_queue_semantics.rs"]
mod time_driven_queue_semantics;
#[path = "all/unknown_attribute_names.rs"]
mod unknown_attribute_names;
#[path = "all/visibility_change.rs"]
mod visibility_change;
