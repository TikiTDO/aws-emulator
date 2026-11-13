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
#[path = "all/at_most_one_member_sets.rs"]
mod at_most_one_member_sets;
#[path = "all/attributes_measure.rs"]
mod attributes_measure;
#[path = "all/attributes_size_limit.rs"]
mod attributes_size_limit;
#[path = "all/aws_refusal_vocabulary.rs"]
mod aws_refusal_vocabulary;
#[path = "all/clock_due_replication_end_instants.rs"]
mod clock_due_replication_end_instants;
#[path = "all/composition_destination_configuration_defaults.rs"]
mod composition_destination_configuration_defaults;
#[path = "all/composition_destination_kinds.rs"]
mod composition_destination_kinds;
#[path = "all/composition_empty_stage_timeout.rs"]
mod composition_empty_stage_timeout;
#[path = "all/composition_layout_default_lifetime.rs"]
mod composition_layout_default_lifetime;
#[path = "all/composition_stage_emptied_by_replication.rs"]
mod composition_stage_emptied_by_replication;
#[path = "all/composition_stop.rs"]
mod composition_stop;
#[path = "all/composition_stop_conflict_wording.rs"]
mod composition_stop_conflict_wording;
#[path = "all/composition_stop_due_replication.rs"]
mod composition_stop_due_replication;
#[path = "all/composition_tag_lifetime.rs"]
mod composition_tag_lifetime;
#[path = "all/composition_terminal_state_retention.rs"]
mod composition_terminal_state_retention;
#[path = "all/compositions.rs"]
mod compositions;
#[path = "all/create_ingest_configuration_inputs.rs"]
mod create_ingest_configuration_inputs;
#[path = "all/create_storage_configuration_inputs.rs"]
mod create_storage_configuration_inputs;
#[path = "all/date_time_members_wire_form.rs"]
mod date_time_members_wire_form;
#[path = "all/delete_ingest_configuration.rs"]
mod delete_ingest_configuration;
#[path = "all/delete_public_key.rs"]
mod delete_public_key;
#[path = "all/delete_stage_associations.rs"]
mod delete_stage_associations;
#[path = "all/delete_stage_ends_replications.rs"]
mod delete_stage_ends_replications;
#[path = "all/delete_storage_configuration.rs"]
mod delete_storage_configuration;
#[path = "all/disconnect_participant.rs"]
mod disconnect_participant;
#[path = "all/disconnect_participant_idempotence.rs"]
mod disconnect_participant_idempotence;
#[path = "all/disconnect_participant_stage_records.rs"]
mod disconnect_participant_stage_records;
#[path = "all/documented_member_defaults.rs"]
mod documented_member_defaults;
#[path = "all/empty_response_framing.rs"]
mod empty_response_framing;
#[path = "all/encoder_configuration_in_use.rs"]
mod encoder_configuration_in_use;
#[path = "all/encoder_configurations.rs"]
mod encoder_configurations;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/float_members_wire_form.rs"]
mod float_members_wire_form;
#[path = "all/forced_ingest_configuration_delete_ends_replications.rs"]
mod forced_ingest_configuration_delete_ends_replications;
#[path = "all/foreign_page_token_wording.rs"]
mod foreign_page_token_wording;
#[path = "all/get_composition.rs"]
mod get_composition;
#[path = "all/get_encoder_configuration.rs"]
mod get_encoder_configuration;
#[path = "all/get_participant.rs"]
mod get_participant;
#[path = "all/get_stage_session.rs"]
mod get_stage_session;
#[path = "all/get_storage_configuration.rs"]
mod get_storage_configuration;
#[path = "all/import_public_key_admission.rs"]
mod import_public_key_admission;
#[path = "all/ingest_configuration_publishing_state.rs"]
mod ingest_configuration_publishing_state;
#[path = "all/ingest_configurations.rs"]
mod ingest_configurations;
#[path = "all/layout_empty_string_defaults.rs"]
mod layout_empty_string_defaults;
#[path = "all/list_compositions.rs"]
mod list_compositions;
#[path = "all/list_encoder_configurations.rs"]
mod list_encoder_configurations;
#[path = "all/list_ingest_configurations.rs"]
mod list_ingest_configurations;
#[path = "all/list_participant_events_token_participant_scope.rs"]
mod list_participant_events_token_participant_scope;
#[path = "all/list_participant_replicas_paging.rs"]
mod list_participant_replicas_paging;
#[path = "all/list_participant_replicas_token_publisher_scope.rs"]
mod list_participant_replicas_token_publisher_scope;
#[path = "all/list_participants_filtered_token_omission.rs"]
mod list_participants_filtered_token_omission;
#[path = "all/list_public_keys.rs"]
mod list_public_keys;
#[path = "all/materialised_member_defaults.rs"]
mod materialised_member_defaults;
#[path = "all/minted_identities.rs"]
mod minted_identities;
#[path = "all/page_token_filter_scope.rs"]
mod page_token_filter_scope;
#[path = "all/page_token_scope.rs"]
mod page_token_scope;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/participant_events.rs"]
mod participant_events;
#[path = "all/participant_ingest_configuration.rs"]
mod participant_ingest_configuration;
#[path = "all/participant_replication.rs"]
mod participant_replication;
#[path = "all/participant_replication_stop.rs"]
mod participant_replication_stop;
#[path = "all/participant_token_form.rs"]
mod participant_token_form;
#[path = "all/participants.rs"]
mod participants;
#[path = "all/public_keys.rs"]
mod public_keys;
#[path = "all/redundant_ingest_toggle.rs"]
mod redundant_ingest_toggle;
#[path = "all/region_scoped_resources.rs"]
mod region_scoped_resources;
#[path = "all/replica_attributes_merge_bound.rs"]
mod replica_attributes_merge_bound;
#[path = "all/replica_participant_recording.rs"]
mod replica_participant_recording;
#[path = "all/replica_rejoin_identity.rs"]
mod replica_rejoin_identity;
#[path = "all/replicated_participant_views.rs"]
mod replicated_participant_views;
#[path = "all/replication_end_recorded_in_source_session.rs"]
mod replication_end_recorded_in_source_session;
#[path = "all/replication_follows_publisher_connection.rs"]
mod replication_follows_publisher_connection;
#[path = "all/response_headers.rs"]
mod response_headers;
#[path = "all/signing_name_routing.rs"]
mod signing_name_routing;
#[path = "all/stage_sessions.rs"]
mod stage_sessions;
#[path = "all/stage_storage_configuration_reference.rs"]
mod stage_storage_configuration_reference;
#[path = "all/stages.rs"]
mod stages;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_configuration_bucket_grant.rs"]
mod storage_configuration_bucket_grant;
#[path = "all/storage_configurations.rs"]
mod storage_configurations;
#[path = "all/taggable_resource_kinds.rs"]
mod taggable_resource_kinds;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/thumbnail_interval_recording_mode.rs"]
mod thumbnail_interval_recording_mode;
#[path = "all/update_ingest_configuration.rs"]
mod update_ingest_configuration;
#[path = "all/update_stage_recording_active.rs"]
mod update_stage_recording_active;
#[path = "all/wire_probe.rs"]
mod wire_probe;
