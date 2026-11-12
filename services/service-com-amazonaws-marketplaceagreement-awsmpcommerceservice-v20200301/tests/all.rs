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
#[path = "all/accepted_amendments.rs"]
mod accepted_amendments;
#[path = "all/accepted_term_payloads.rs"]
mod accepted_term_payloads;
#[path = "all/agreement_cancellation_requests.rs"]
mod agreement_cancellation_requests;
#[path = "all/agreement_charges.rs"]
mod agreement_charges;
#[path = "all/agreement_end_dates.rs"]
mod agreement_end_dates;
#[path = "all/agreement_entitlements.rs"]
mod agreement_entitlements;
#[path = "all/agreement_expiry.rs"]
mod agreement_expiry;
#[path = "all/agreement_lifecycle.rs"]
mod agreement_lifecycle;
#[path = "all/agreement_parties.rs"]
mod agreement_parties;
#[path = "all/agreement_renewals.rs"]
mod agreement_renewals;
#[path = "all/billing_adjustment_entry_reasons.rs"]
mod billing_adjustment_entry_reasons;
#[path = "all/billing_adjustment_resolution_order.rs"]
mod billing_adjustment_resolution_order;
#[path = "all/billing_adjustments.rs"]
mod billing_adjustments;
#[path = "all/cancellation_request_decisions.rs"]
mod cancellation_request_decisions;
#[path = "all/cancellation_request_reads.rs"]
mod cancellation_request_reads;
#[path = "all/cancellation_request_rejection.rs"]
mod cancellation_request_rejection;
#[path = "all/cancellation_request_withdrawal.rs"]
mod cancellation_request_withdrawal;
#[path = "all/charge_amounts.rs"]
mod charge_amounts;
#[path = "all/client_token_scope.rs"]
mod client_token_scope;
#[path = "all/end_time_behavior_projections.rs"]
mod end_time_behavior_projections;
#[path = "all/entitlement_resources.rs"]
mod entitlement_resources;
#[path = "all/error_bodies.rs"]
mod error_bodies;
#[path = "all/idempotency_token_conflicts.rs"]
mod idempotency_token_conflicts;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/inactive_agreement_refusals.rs"]
mod inactive_agreement_refusals;
#[path = "all/intent_enum_values.rs"]
mod intent_enum_values;
#[path = "all/invoice_adjustments.rs"]
mod invoice_adjustments;
#[path = "all/invoice_line_item_listing.rs"]
mod invoice_line_item_listing;
#[path = "all/last_update_time_monotonicity.rs"]
mod last_update_time_monotonicity;
#[path = "all/license_arn_region.rs"]
mod license_arn_region;
#[path = "all/listing_resolution_order.rs"]
mod listing_resolution_order;
#[path = "all/optional_input_empty_pages.rs"]
mod optional_input_empty_pages;
#[path = "all/page_token_scopes.rs"]
mod page_token_scopes;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/payment_request_agreement_ids.rs"]
mod payment_request_agreement_ids;
#[path = "all/payment_request_approval.rs"]
mod payment_request_approval;
#[path = "all/payment_request_auto_approval.rs"]
mod payment_request_auto_approval;
#[path = "all/payment_request_balance.rs"]
mod payment_request_balance;
#[path = "all/payment_request_cancellation.rs"]
mod payment_request_cancellation;
#[path = "all/payment_request_rejection.rs"]
mod payment_request_rejection;
#[path = "all/payment_request_selector_resolution.rs"]
mod payment_request_selector_resolution;
#[path = "all/payment_requests.rs"]
mod payment_requests;
#[path = "all/purchase_order_agreement_ids.rs"]
mod purchase_order_agreement_ids;
#[path = "all/purchase_order_associations.rs"]
mod purchase_order_associations;
#[path = "all/purchase_order_charge_revisions.rs"]
mod purchase_order_charge_revisions;
#[path = "all/purchase_order_required_members.rs"]
mod purchase_order_required_members;
#[path = "all/purchase_order_update_agreement_id.rs"]
mod purchase_order_update_agreement_id;
#[path = "all/purchase_order_updates.rs"]
mod purchase_order_updates;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/repeated_dimension_keys.rs"]
mod repeated_dimension_keys;
#[path = "all/replacement_source_status.rs"]
mod replacement_source_status;
#[path = "all/search_filter_combinations.rs"]
mod search_filter_combinations;
#[path = "all/search_time_windows.rs"]
mod search_time_windows;
#[path = "all/selector_resolution.rs"]
mod selector_resolution;
#[path = "all/selector_values.rs"]
mod selector_values;
#[path = "all/seller_submitted_request_views.rs"]
mod seller_submitted_request_views;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_full_refusals.rs"]
mod store_full_refusals;
#[path = "all/store_full_replays.rs"]
mod store_full_replays;
#[path = "all/time_windows.rs"]
mod time_windows;
#[path = "all/unknown_ids.rs"]
mod unknown_ids;
#[path = "all/unmatched_selector_pages.rs"]
mod unmatched_selector_pages;
#[path = "all/unsettled_store_adjustment_writes.rs"]
mod unsettled_store_adjustment_writes;
#[path = "all/unsettled_store_invoice_reports.rs"]
mod unsettled_store_invoice_reports;
#[path = "all/unsettled_store_listings.rs"]
mod unsettled_store_listings;
#[path = "all/unsettled_store_single_agreement_reads.rs"]
mod unsettled_store_single_agreement_reads;
#[path = "all/validation_envelope.rs"]
mod validation_envelope;
#[path = "all/validation_field_texts.rs"]
mod validation_field_texts;
#[path = "all/validation_messages.rs"]
mod validation_messages;
#[path = "all/window_token_scopes.rs"]
mod window_token_scopes;
