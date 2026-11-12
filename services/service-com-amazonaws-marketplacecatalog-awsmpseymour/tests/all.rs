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
#[path = "all/assessment_identifier_length.rs"]
mod assessment_identifier_length;
#[path = "all/assessment_production.rs"]
mod assessment_production;
#[path = "all/assessments.rs"]
mod assessments;
#[path = "all/cancel_change_set_window.rs"]
mod cancel_change_set_window;
#[path = "all/change_pair_uniqueness.rs"]
mod change_pair_uniqueness;
#[path = "all/change_reference_resolution.rs"]
mod change_reference_resolution;
#[path = "all/change_set_failures.rs"]
mod change_set_failures;
#[path = "all/change_set_lifecycle.rs"]
mod change_set_lifecycle;
#[path = "all/change_type_validation.rs"]
mod change_type_validation;
#[path = "all/default_page_size.rs"]
mod default_page_size;
#[path = "all/delete_resource_policy_existence.rs"]
mod delete_resource_policy_existence;
#[path = "all/details_length_bound.rs"]
mod details_length_bound;
#[path = "all/documented_entity_types.rs"]
mod documented_entity_types;
#[path = "all/entity_id_list_contract.rs"]
mod entity_id_list_contract;
#[path = "all/entity_lock_in_flight.rs"]
mod entity_lock_in_flight;
#[path = "all/entity_read_agreement.rs"]
mod entity_read_agreement;
#[path = "all/entity_reads.rs"]
mod entity_reads;
#[path = "all/entity_type_projections.rs"]
mod entity_type_projections;
#[path = "all/filter_validation.rs"]
mod filter_validation;
#[path = "all/filter_value_forms.rs"]
mod filter_value_forms;
#[path = "all/framework_id_validation.rs"]
mod framework_id_validation;
#[path = "all/model_declared_sort_attributes.rs"]
mod model_declared_sort_attributes;
#[path = "all/modelled_answer_bounds.rs"]
mod modelled_answer_bounds;
#[path = "all/modelled_member_lengths.rs"]
mod modelled_member_lengths;
#[path = "all/page_size_rule.rs"]
mod page_size_rule;
#[path = "all/page_token_property.rs"]
mod page_token_property;
#[path = "all/page_token_rule.rs"]
mod page_token_rule;
#[path = "all/page_token_spelling.rs"]
mod page_token_spelling;
#[path = "all/pair_uniqueness_under_every_intent.rs"]
mod pair_uniqueness_under_every_intent;
#[path = "all/request_validation.rs"]
mod request_validation;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_policy_and_tagging.rs"]
mod resource_policy_and_tagging;
#[path = "all/resource_policy_entity_only.rs"]
mod resource_policy_entity_only;
#[path = "all/resource_policy_lifecycle.rs"]
mod resource_policy_lifecycle;
#[path = "all/resource_policy_reads.rs"]
mod resource_policy_reads;
#[path = "all/resource_policy_validation.rs"]
mod resource_policy_validation;
#[path = "all/sort_order_direction.rs"]
mod sort_order_direction;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/summary_member_contracts.rs"]
mod summary_member_contracts;
#[path = "all/tag_set_uniqueness.rs"]
mod tag_set_uniqueness;
#[path = "all/validate_intent_verdict.rs"]
mod validate_intent_verdict;
#[path = "all/validation_refusal_class.rs"]
mod validation_refusal_class;
