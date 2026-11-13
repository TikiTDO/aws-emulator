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
#[path = "all/aws_assigned_system_tags.rs"]
mod aws_assigned_system_tags;
#[path = "all/aws_tag_rules_span_every_tag_pair.rs"]
mod aws_tag_rules_span_every_tag_pair;
#[path = "all/bandwidth_rate_limit_schedule.rs"]
mod bandwidth_rate_limit_schedule;
#[path = "all/bandwidth_rate_limit_schedule_consistency.rs"]
mod bandwidth_rate_limit_schedule_consistency;
#[path = "all/default_maintenance_window.rs"]
mod default_maintenance_window;
#[path = "all/element_position_counts_from_one.rs"]
mod element_position_counts_from_one;
#[path = "all/fixed_refusal_sentences.rs"]
mod fixed_refusal_sentences;
#[path = "all/gateway_activation.rs"]
mod gateway_activation;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/gateway_server_association.rs"]
mod gateway_server_association;
#[path = "all/gateway_server_disassociation.rs"]
mod gateway_server_disassociation;
#[path = "all/gateway_software_update.rs"]
mod gateway_software_update;
#[path = "all/hypervisor_configuration_test.rs"]
mod hypervisor_configuration_test;
#[path = "all/hypervisor_host_is_not_a_key.rs"]
mod hypervisor_host_is_not_a_key;
#[path = "all/hypervisor_import_concurrency.rs"]
mod hypervisor_import_concurrency;
#[path = "all/hypervisor_import_is_not_deduplicated.rs"]
mod hypervisor_import_is_not_deduplicated;
#[path = "all/hypervisor_listing.rs"]
mod hypervisor_listing;
#[path = "all/hypervisor_property_mappings.rs"]
mod hypervisor_property_mappings;
#[path = "all/hypervisor_reads.rs"]
mod hypervisor_reads;
#[path = "all/hypervisor_update.rs"]
mod hypervisor_update;
#[path = "all/maintenance_start_time.rs"]
mod maintenance_start_time;
#[path = "all/metadata_sync.rs"]
mod metadata_sync;
#[path = "all/metadata_sync_is_not_gated_on_property_mappings.rs"]
mod metadata_sync_is_not_gated_on_property_mappings;
#[path = "all/metadata_sync_over_the_wire.rs"]
mod metadata_sync_over_the_wire;
#[path = "all/metadata_sync_publication_is_one_instant.rs"]
mod metadata_sync_publication_is_one_instant;
#[path = "all/metadata_sync_requires_property_mappings.rs"]
mod metadata_sync_requires_property_mappings;
#[path = "all/metadata_sync_restart_supersedes_the_pending_sync.rs"]
mod metadata_sync_restart_supersedes_the_pending_sync;
#[path = "all/minted_arn_region_is_bounded.rs"]
mod minted_arn_region_is_bounded;
#[path = "all/minted_hypervisor_identity.rs"]
mod minted_hypervisor_identity;
#[path = "all/minted_identity_is_never_reused.rs"]
mod minted_identity_is_never_reused;
#[path = "all/minted_resource_id_form.rs"]
mod minted_resource_id_form;
#[path = "all/missing_resource_refusal_is_one_answer.rs"]
mod missing_resource_refusal_is_one_answer;
#[path = "all/negative_paths.rs"]
mod negative_paths;
#[path = "all/page_size_is_one_rule.rs"]
mod page_size_is_one_rule;
#[path = "all/page_token_provenance.rs"]
mod page_token_provenance;
#[path = "all/page_token_query_identity.rs"]
mod page_token_query_identity;
#[path = "all/page_token_refusal_wording.rs"]
mod page_token_refusal_wording;
#[path = "all/pattern_requirement_is_the_modelled_expression.rs"]
mod pattern_requirement_is_the_modelled_expression;
#[path = "all/put_bandwidth_rate_limit_schedule.rs"]
mod put_bandwidth_rate_limit_schedule;
#[path = "all/put_hypervisor_property_mappings.rs"]
mod put_hypervisor_property_mappings;
#[path = "all/put_property_mappings_one_answer_for_a_missing_hypervisor.rs"]
mod put_property_mappings_one_answer_for_a_missing_hypervisor;
#[path = "all/refusal_names_the_member_one_way.rs"]
mod refusal_names_the_member_one_way;
#[path = "all/refusals_name_the_condition.rs"]
mod refusals_name_the_condition;
#[path = "all/regional_listing_scope.rs"]
mod regional_listing_scope;
#[path = "all/regional_single_resource_scope.rs"]
mod regional_single_resource_scope;
#[path = "all/resource_tagging_spans_every_resource.rs"]
mod resource_tagging_spans_every_resource;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/synced_inventory_is_per_hypervisor_record.rs"]
mod synced_inventory_is_per_hypervisor_record;
#[path = "all/system_tag_created_date_form.rs"]
mod system_tag_created_date_form;
#[path = "all/tag_rules_precede_the_resource_lookup.rs"]
mod tag_rules_precede_the_resource_lookup;
#[path = "all/tag_value_spaces.rs"]
mod tag_value_spaces;
#[path = "all/virtual_machine_inventory_sync.rs"]
mod virtual_machine_inventory_sync;
#[path = "all/virtual_machine_listing.rs"]
mod virtual_machine_listing;
#[path = "all/virtual_machine_reads.rs"]
mod virtual_machine_reads;
