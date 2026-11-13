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
#[path = "all/agent_assist_query_pipeline.rs"]
mod agent_assist_query_pipeline;
#[path = "all/assistant_association.rs"]
mod assistant_association;
#[path = "all/assistant_association_listing.rs"]
mod assistant_association_listing;
#[path = "all/assistant_lifecycle.rs"]
mod assistant_lifecycle;
#[path = "all/client_error_refusals.rs"]
mod client_error_refusals;
#[path = "all/content_precondition_message.rs"]
mod content_precondition_message;
#[path = "all/content_precondition_refusal.rs"]
mod content_precondition_refusal;
#[path = "all/content_revisions.rs"]
mod content_revisions;
#[path = "all/content_search.rs"]
mod content_search;
#[path = "all/content_summary.rs"]
mod content_summary;
#[path = "all/content_upload.rs"]
mod content_upload;
#[path = "all/idempotency_contents_and_refusal_fidelity.rs"]
mod idempotency_contents_and_refusal_fidelity;
#[path = "all/knowledge_base_lifecycle.rs"]
mod knowledge_base_lifecycle;
#[path = "all/notify_recommendations_error_text.rs"]
mod notify_recommendations_error_text;
#[path = "all/quick_response.rs"]
mod quick_response;
#[path = "all/quick_response_mutations.rs"]
mod quick_response_mutations;
#[path = "all/quick_response_search.rs"]
mod quick_response_search;
#[path = "all/relayed_knowledge_base_refusal.rs"]
mod relayed_knowledge_base_refusal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/session_recommendations.rs"]
mod session_recommendations;
#[path = "all/shared_signing_name_routing.rs"]
mod shared_signing_name_routing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
