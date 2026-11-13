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
#[path = "all/absent_attribute_columns.rs"]
mod absent_attribute_columns;
#[path = "all/action_filter_value_forms.rs"]
mod action_filter_value_forms;
#[path = "all/arn_resource_type.rs"]
mod arn_resource_type;
#[path = "all/comparison_operands.rs"]
mod comparison_operands;
#[path = "all/empty_string_member_constraints.rs"]
mod empty_string_member_constraints;
#[path = "all/filter_subject.rs"]
mod filter_subject;
#[path = "all/filter_value_quoting.rs"]
mod filter_value_quoting;
#[path = "all/identity_and_account_authorization.rs"]
mod identity_and_account_authorization;
#[path = "all/inference.rs"]
mod inference;
#[path = "all/metadata_member_presence.rs"]
mod metadata_member_presence;
#[path = "all/no_inference_target_refusal.rs"]
mod no_inference_target_refusal;
#[path = "all/optional_input_refusal_order.rs"]
mod optional_input_refusal_order;
#[path = "all/promotion_share.rs"]
mod promotion_share;
#[path = "all/recipe_type_serving_operation.rs"]
mod recipe_type_serving_operation;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/repeated_input_list_ids.rs"]
mod repeated_input_list_ids;
#[path = "all/request_only_refusal_order.rs"]
mod request_only_refusal_order;
#[path = "all/resolution_precedes_optional_input.rs"]
mod resolution_precedes_optional_input;
#[path = "all/score_denominator.rs"]
mod score_denominator;
#[path = "all/seed_part_boundaries.rs"]
mod seed_part_boundaries;
#[path = "all/shared_score_seed.rs"]
mod shared_score_seed;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/tie_break_order.rs"]
mod tie_break_order;
#[path = "all/unbound_filter_values.rs"]
mod unbound_filter_values;
#[path = "all/unresolved_resource_arns.rs"]
mod unresolved_resource_arns;
#[path = "all/unused_personalization_input.rs"]
mod unused_personalization_input;
