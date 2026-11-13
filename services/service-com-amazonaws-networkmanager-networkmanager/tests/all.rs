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
#[path = "all/arn_list_filter_form.rs"]
mod arn_list_filter_form;
#[path = "all/arn_scoping.rs"]
mod arn_scoping;
#[path = "all/associate_customer_gateway_scoping.rs"]
mod associate_customer_gateway_scoping;
#[path = "all/associate_customer_gateway_vpn_attachment_carrier.rs"]
mod associate_customer_gateway_vpn_attachment_carrier;
#[path = "all/associate_link_member_agreement.rs"]
mod associate_link_member_agreement;
#[path = "all/associate_link_refusals.rs"]
mod associate_link_refusals;
#[path = "all/attachment_acceptance.rs"]
mod attachment_acceptance;
#[path = "all/attachment_lifecycle.rs"]
mod attachment_lifecycle;
#[path = "all/attachment_rejection.rs"]
mod attachment_rejection;
#[path = "all/attachment_routing_policy_associations.rs"]
mod attachment_routing_policy_associations;
#[path = "all/attachment_routing_policy_label.rs"]
mod attachment_routing_policy_label;
#[path = "all/attachment_routing_policy_label_idempotency.rs"]
mod attachment_routing_policy_label_idempotency;
#[path = "all/attachment_routing_policy_label_removal.rs"]
mod attachment_routing_policy_label_removal;
#[path = "all/build_on_created_attachment.rs"]
mod build_on_created_attachment;
#[path = "all/client_token_idempotency.rs"]
mod client_token_idempotency;
#[path = "all/connect_peer_association_lifecycle.rs"]
mod connect_peer_association_lifecycle;
#[path = "all/connect_peer_delete.rs"]
mod connect_peer_delete;
#[path = "all/connect_peer_delete_cascade.rs"]
mod connect_peer_delete_cascade;
#[path = "all/connect_peer_lifecycle.rs"]
mod connect_peer_lifecycle;
#[path = "all/connect_peer_orphan_prevention.rs"]
mod connect_peer_orphan_prevention;
#[path = "all/connect_peer_protocol_scoped_members.rs"]
mod connect_peer_protocol_scoped_members;
#[path = "all/connect_peer_tie_removal_refusals.rs"]
mod connect_peer_tie_removal_refusals;
#[path = "all/connect_peer_tunnel_address_family.rs"]
mod connect_peer_tunnel_address_family;
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/connection_teardown_order.rs"]
mod connection_teardown_order;
#[path = "all/core_network_change_events.rs"]
mod core_network_change_events;
#[path = "all/core_network_change_free_update.rs"]
mod core_network_change_free_update;
#[path = "all/core_network_change_set.rs"]
mod core_network_change_set;
#[path = "all/core_network_change_set_execution.rs"]
mod core_network_change_set_execution;
#[path = "all/core_network_change_set_segment_actions.rs"]
mod core_network_change_set_segment_actions;
#[path = "all/core_network_lifecycle.rs"]
mod core_network_lifecycle;
#[path = "all/core_network_policy_version_listing.rs"]
mod core_network_policy_version_listing;
#[path = "all/core_network_policy_version_restore.rs"]
mod core_network_policy_version_restore;
#[path = "all/core_network_policy_versions.rs"]
mod core_network_policy_versions;
#[path = "all/core_network_prefix_list_association.rs"]
mod core_network_prefix_list_association;
#[path = "all/core_network_prefix_list_associations.rs"]
mod core_network_prefix_list_associations;
#[path = "all/core_network_routing_information.rs"]
mod core_network_routing_information;
#[path = "all/core_network_segment_actions.rs"]
mod core_network_segment_actions;
#[path = "all/create_connect_peer_bounds.rs"]
mod create_connect_peer_bounds;
#[path = "all/create_connection_bounds.rs"]
mod create_connection_bounds;
#[path = "all/create_time_tag_keys.rs"]
mod create_time_tag_keys;
#[path = "all/create_vpc_attachment_bounds.rs"]
mod create_vpc_attachment_bounds;
#[path = "all/customer_gateway_association_lifecycle.rs"]
mod customer_gateway_association_lifecycle;
#[path = "all/delete_attachment.rs"]
mod delete_attachment;
#[path = "all/delete_connection.rs"]
mod delete_connection;
#[path = "all/delete_dependency_refusal.rs"]
mod delete_dependency_refusal;
#[path = "all/delete_dependent_resource_refusal.rs"]
mod delete_dependent_resource_refusal;
#[path = "all/delete_device.rs"]
mod delete_device;
#[path = "all/delete_global_network_route_analyses.rs"]
mod delete_global_network_route_analyses;
#[path = "all/delete_global_network_transit_gateway_precondition.rs"]
mod delete_global_network_transit_gateway_precondition;
#[path = "all/delete_link.rs"]
mod delete_link;
#[path = "all/delete_live_reference_refusal.rs"]
mod delete_live_reference_refusal;
#[path = "all/delete_peering.rs"]
mod delete_peering;
#[path = "all/describe_global_networks_id_resolution.rs"]
mod describe_global_networks_id_resolution;
#[path = "all/device_lifecycle.rs"]
mod device_lifecycle;
#[path = "all/direct_connect_gateway_attachment_lifecycle.rs"]
mod direct_connect_gateway_attachment_lifecycle;
#[path = "all/disassociate_connect_peer.rs"]
mod disassociate_connect_peer;
#[path = "all/disassociate_link_tie_scope.rs"]
mod disassociate_link_tie_scope;
#[path = "all/edge_location_in_force.rs"]
mod edge_location_in_force;
#[path = "all/empty_optional_reference.rs"]
mod empty_optional_reference;
#[path = "all/empty_page_token.rs"]
mod empty_page_token;
#[path = "all/empty_reference_filter.rs"]
mod empty_reference_filter;
#[path = "all/filter_id_list_maximum.rs"]
mod filter_id_list_maximum;
#[path = "all/filter_maximum_table.rs"]
mod filter_maximum_table;
#[path = "all/get_connect_attachment.rs"]
mod get_connect_attachment;
#[path = "all/get_connections.rs"]
mod get_connections;
#[path = "all/get_core_network_policy_selection.rs"]
mod get_core_network_policy_selection;
#[path = "all/get_customer_gateway_associations_paging.rs"]
mod get_customer_gateway_associations_paging;
#[path = "all/get_devices_ids_filter_maximum.rs"]
mod get_devices_ids_filter_maximum;
#[path = "all/get_direct_connect_gateway_attachment.rs"]
mod get_direct_connect_gateway_attachment;
#[path = "all/get_link_associations_selector_absence.rs"]
mod get_link_associations_selector_absence;
#[path = "all/get_links_site_exclusion.rs"]
mod get_links_site_exclusion;
#[path = "all/get_network_routes.rs"]
mod get_network_routes;
#[path = "all/get_transit_gateway_connect_peer_associations_paging.rs"]
mod get_transit_gateway_connect_peer_associations_paging;
#[path = "all/get_transit_gateway_registrations_paging.rs"]
mod get_transit_gateway_registrations_paging;
#[path = "all/get_vpc_attachment.rs"]
mod get_vpc_attachment;
#[path = "all/global_network_lifecycle.rs"]
mod global_network_lifecycle;
#[path = "all/link_association_lifecycle.rs"]
mod link_association_lifecycle;
#[path = "all/link_ids_filter_maximum.rs"]
mod link_ids_filter_maximum;
#[path = "all/link_lifecycle.rs"]
mod link_lifecycle;
#[path = "all/list_attachments.rs"]
mod list_attachments;
#[path = "all/list_peerings.rs"]
mod list_peerings;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/malformed_resource_arn_refusal.rs"]
mod malformed_resource_arn_refusal;
#[path = "all/modification_errors.rs"]
mod modification_errors;
#[path = "all/network_resource_counts.rs"]
mod network_resource_counts;
#[path = "all/network_resource_metadata.rs"]
mod network_resource_metadata;
#[path = "all/network_resource_regions.rs"]
mod network_resource_regions;
#[path = "all/network_resource_relationships.rs"]
mod network_resource_relationships;
#[path = "all/network_resources.rs"]
mod network_resources;
#[path = "all/network_telemetry.rs"]
mod network_telemetry;
#[path = "all/optional_reference_one_home.rs"]
mod optional_reference_one_home;
#[path = "all/organization_service_access.rs"]
mod organization_service_access;
#[path = "all/page_token_canonical_spelling.rs"]
mod page_token_canonical_spelling;
#[path = "all/page_token_listing_scope.rs"]
mod page_token_listing_scope;
#[path = "all/policy_topology_derivation.rs"]
mod policy_topology_derivation;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/register_transit_gateway_account_ownership.rs"]
mod register_transit_gateway_account_ownership;
#[path = "all/resource_not_found_message.rs"]
mod resource_not_found_message;
#[path = "all/resource_policy_lifecycle.rs"]
mod resource_policy_lifecycle;
#[path = "all/resource_state_machine.rs"]
mod resource_state_machine;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/route_analysis.rs"]
mod route_analysis;
#[path = "all/route_analysis_empty_endpoint_reference.rs"]
mod route_analysis_empty_endpoint_reference;
#[path = "all/site_ids_filter_maximum.rs"]
mod site_ids_filter_maximum;
#[path = "all/site_to_site_vpn_attachment_lifecycle.rs"]
mod site_to_site_vpn_attachment_lifecycle;
#[path = "all/site_to_site_vpn_attachment_telemetry.rs"]
mod site_to_site_vpn_attachment_telemetry;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_dispatch_agreement.rs"]
mod tag_dispatch_agreement;
#[path = "all/transit_gateway_connect_peer_association_lifecycle.rs"]
mod transit_gateway_connect_peer_association_lifecycle;
#[path = "all/transit_gateway_connect_peer_association_quota.rs"]
mod transit_gateway_connect_peer_association_quota;
#[path = "all/transit_gateway_peering.rs"]
mod transit_gateway_peering;
#[path = "all/transit_gateway_peering_create_bounds.rs"]
mod transit_gateway_peering_create_bounds;
#[path = "all/transit_gateway_registration_lifecycle.rs"]
mod transit_gateway_registration_lifecycle;
#[path = "all/transit_gateway_route_table_attachment.rs"]
mod transit_gateway_route_table_attachment;
#[path = "all/transit_gateway_route_table_attachment_lifecycle.rs"]
mod transit_gateway_route_table_attachment_lifecycle;
#[path = "all/unknown_global_network_on_a_new_record.rs"]
mod unknown_global_network_on_a_new_record;
#[path = "all/unknown_parent_reference.rs"]
mod unknown_parent_reference;
#[path = "all/unminted_page_token_refusal.rs"]
mod unminted_page_token_refusal;
#[path = "all/unpublished_filter_maximum.rs"]
mod unpublished_filter_maximum;
#[path = "all/unresolvable_reference_field_naming.rs"]
mod unresolvable_reference_field_naming;
#[path = "all/unsupported_resource_type_refusal.rs"]
mod unsupported_resource_type_refusal;
#[path = "all/update_core_network.rs"]
mod update_core_network;
#[path = "all/update_device.rs"]
mod update_device;
#[path = "all/update_empty_optional_reference.rs"]
mod update_empty_optional_reference;
#[path = "all/update_link.rs"]
mod update_link;
#[path = "all/update_no_op.rs"]
mod update_no_op;
#[path = "all/update_no_op_while_creating.rs"]
mod update_no_op_while_creating;
#[path = "all/update_site.rs"]
mod update_site;
#[path = "all/vpc_attachment_lifecycle.rs"]
mod vpc_attachment_lifecycle;
#[path = "all/vpc_attachment_options_spelling.rs"]
mod vpc_attachment_options_spelling;
