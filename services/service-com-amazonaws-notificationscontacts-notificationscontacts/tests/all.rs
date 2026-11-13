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
#[path = "all/activation_code_bounds.rs"]
mod activation_code_bounds;
#[path = "all/activation_refusal_turns_on_the_token.rs"]
mod activation_refusal_turns_on_the_token;
#[path = "all/activation_token_resource.rs"]
mod activation_token_resource;
#[path = "all/contact_channel_associations.rs"]
mod contact_channel_associations;
#[path = "all/email_contact_address_pattern.rs"]
mod email_contact_address_pattern;
#[path = "all/email_contact_name_pattern.rs"]
mod email_contact_name_pattern;
#[path = "all/email_contact_quota_refusal.rs"]
mod email_contact_quota_refusal;
#[path = "all/email_contact_timestamp_wire_form.rs"]
mod email_contact_timestamp_wire_form;
#[path = "all/email_contacts.rs"]
mod email_contacts;
#[path = "all/error_message_templates.rs"]
mod error_message_templates;
#[path = "all/hand_applied_pattern_refusals.rs"]
mod hand_applied_pattern_refusals;
#[path = "all/list_email_contacts_page_token.rs"]
mod list_email_contacts_page_token;
#[path = "all/recorded_instants_are_monotonic.rs"]
mod recorded_instants_are_monotonic;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/resource_tag_writes.rs"]
mod resource_tag_writes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_mutation_success_status.rs"]
mod tag_mutation_success_status;
#[path = "all/tag_key_constraints.rs"]
mod tag_key_constraints;
#[path = "all/untag_resource_tag_keys.rs"]
mod untag_resource_tag_keys;
