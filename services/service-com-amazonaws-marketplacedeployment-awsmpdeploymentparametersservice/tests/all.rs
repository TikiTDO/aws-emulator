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
#[path = "all/boundary_validation_field_name.rs"]
mod boundary_validation_field_name;
#[path = "all/catalog_allowed_values.rs"]
mod catalog_allowed_values;
#[path = "all/clock_sample_under_the_store_lock.rs"]
mod clock_sample_under_the_store_lock;
#[path = "all/contended_writes_stamp_under_the_store_lock.rs"]
mod contended_writes_stamp_under_the_store_lock;
#[path = "all/deployment_parameter_expiry.rs"]
mod deployment_parameter_expiry;
#[path = "all/deployment_parameter_identity_key.rs"]
mod deployment_parameter_identity_key;
#[path = "all/deployment_parameters.rs"]
mod deployment_parameters;
#[path = "all/enforced_character_classes.rs"]
mod enforced_character_classes;
#[path = "all/expected_vendor_sentence.rs"]
mod expected_vendor_sentence;
#[path = "all/expiration_date_wire_form.rs"]
mod expiration_date_wire_form;
#[path = "all/front_door_forbidden.rs"]
mod front_door_forbidden;
#[path = "all/not_found_error_code.rs"]
mod not_found_error_code;
#[path = "all/put_deployment_parameter_target.rs"]
mod put_deployment_parameter_target;
#[path = "all/put_deployment_parameter_unseeded.rs"]
mod put_deployment_parameter_unseeded;
#[path = "all/put_reports_the_stored_tag_set.rs"]
mod put_reports_the_stored_tag_set;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_arn_classification.rs"]
mod resource_arn_classification;
#[path = "all/resource_arn_validation.rs"]
mod resource_arn_validation;
#[path = "all/response_request_id.rs"]
mod response_request_id;
#[path = "all/sensitive_member_is_a_modelled_path.rs"]
mod sensitive_member_is_a_modelled_path;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/tag_count_front_end.rs"]
mod tag_count_front_end;
#[path = "all/tag_input_judged_before_lookup.rs"]
mod tag_input_judged_before_lookup;
#[path = "all/tag_key_judged_the_same_by_every_member.rs"]
mod tag_key_judged_the_same_by_every_member;
#[path = "all/tag_write_last_written.rs"]
mod tag_write_last_written;
#[path = "all/tagging_resolution_sequence.rs"]
mod tagging_resolution_sequence;
#[path = "all/unconstrained_tag_characters.rs"]
mod unconstrained_tag_characters;
#[path = "all/unresolved_target_precedence.rs"]
mod unresolved_target_precedence;
#[path = "all/validation_messages.rs"]
mod validation_messages;
