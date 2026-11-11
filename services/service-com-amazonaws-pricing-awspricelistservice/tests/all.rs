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
#[path = "all/empty_selection.rs"]
mod empty_selection;
#[path = "all/get_products_offers.rs"]
mod get_products_offers;
#[path = "all/pattern_requirement_wording.rs"]
mod pattern_requirement_wording;
#[path = "all/pricing_catalog.rs"]
mod pricing_catalog;
#[path = "all/pricing_fidelity.rs"]
mod pricing_fidelity;
#[path = "all/pricing_support.rs"]
mod pricing_support;
#[path = "all/published_catalog.rs"]
mod published_catalog;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
