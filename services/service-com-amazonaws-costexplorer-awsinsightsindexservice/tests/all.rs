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
#[path = "all/anomaly_monitor_delete.rs"]
mod anomaly_monitor_delete;
#[path = "all/anomaly_monitors.rs"]
mod anomaly_monitors;
#[path = "all/anomaly_subscription_update.rs"]
mod anomaly_subscription_update;
#[path = "all/anomaly_subscriptions.rs"]
mod anomaly_subscriptions;
#[path = "all/cost_category_definitions.rs"]
mod cost_category_definitions;
#[path = "all/get_dimension_values_context.rs"]
mod get_dimension_values_context;
#[path = "all/granularity_refusal.rs"]
mod granularity_refusal;
// The shared HTTP harness every suite above drives the service through. Declared before
// nothing in particular — a test-target crate root's `mod`s are order-independent — but
// kept with the suites so a new suite sees it.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unknown_monitor_references.rs"]
mod unknown_monitor_references;
