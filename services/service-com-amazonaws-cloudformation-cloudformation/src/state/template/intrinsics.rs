// SPDX-License-Identifier: CC0-1.0

//! Evaluating a template's INTRINSIC FUNCTIONS lives in [`super`], beside the stages and the
//! naming rules that read a resolved value, and the per-type return values live in
//! [`super::return_values`].
//!
//! This module is deliberately empty, and is not declared: an earlier revision of the
//! Elastic Load Balancing v2 provisioning fix moved the intrinsic evaluator here to keep
//! `template.rs` inside the length a change to this repository may touch, and the split that
//! landed on the mainline instead took the NAMING RULES out of that file
//! (`super::naming_rules`), which leaves the evaluator where every caller of it already is.
