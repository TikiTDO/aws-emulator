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
#[path = "all/absent_subresource_is_not_found.rs"]
mod absent_subresource_is_not_found;
#[path = "all/account_information_wire_fidelity.rs"]
mod account_information_wire_fidelity;
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/account_target_composition_is_shared.rs"]
mod account_target_composition_is_shared;
#[path = "all/account_target_resolution_is_shared.rs"]
mod account_target_resolution_is_shared;
#[path = "all/alternate_contact_delete_precedence.rs"]
mod alternate_contact_delete_precedence;
#[path = "all/contact_information.rs"]
mod contact_information;
#[path = "all/enum_input_validation.rs"]
mod enum_input_validation;
#[path = "all/gov_cloud_account_information.rs"]
mod gov_cloud_account_information;
#[path = "all/no_primary_email_update_refusal.rs"]
mod no_primary_email_update_refusal;
#[path = "all/one_time_passcode_confirmation.rs"]
mod one_time_passcode_confirmation;
#[path = "all/own_account_target_refusal.rs"]
mod own_account_target_refusal;
#[path = "all/pagination_token_spelling.rs"]
mod pagination_token_spelling;
#[path = "all/pattern_validation_messages.rs"]
mod pattern_validation_messages;
#[path = "all/phone_number_verification.rs"]
mod phone_number_verification;
#[path = "all/primary_contact_address_rules.rs"]
mod primary_contact_address_rules;
#[path = "all/primary_email_address_syntax.rs"]
mod primary_email_address_syntax;
#[path = "all/primary_email_completion_due_rule.rs"]
mod primary_email_completion_due_rule;
#[path = "all/primary_email_read.rs"]
mod primary_email_read;
#[path = "all/primary_email_update.rs"]
mod primary_email_update;
#[path = "all/primary_email_update_acceptance.rs"]
mod primary_email_update_acceptance;
#[path = "all/primary_email_update_address_in_use.rs"]
mod primary_email_update_address_in_use;
#[path = "all/primary_email_update_in_flight.rs"]
mod primary_email_update_in_flight;
#[path = "all/primary_email_update_self_reaffirmation.rs"]
mod primary_email_update_self_reaffirmation;
#[path = "all/refused_member_payload_wording.rs"]
mod refused_member_payload_wording;
#[path = "all/region_opt_in.rs"]
mod region_opt_in;
#[path = "all/region_opt_in_listing.rs"]
mod region_opt_in_listing;
#[path = "all/region_opt_target_refusal_over_the_whole_catalog.rs"]
mod region_opt_target_refusal_over_the_whole_catalog;
#[path = "all/region_opt_target_rule.rs"]
mod region_opt_target_rule;
#[path = "all/schema_finding_refusal_wording.rs"]
mod schema_finding_refusal_wording;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unknown_account_refusal.rs"]
mod unknown_account_refusal;
#[path = "all/validation_field_list_names_the_member.rs"]
mod validation_field_list_names_the_member;
