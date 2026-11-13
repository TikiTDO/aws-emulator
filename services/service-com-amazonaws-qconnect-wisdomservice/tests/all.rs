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
#[path = "all/ai_agent.rs"]
mod ai_agent;
#[path = "all/ai_agent_update.rs"]
mod ai_agent_update;
#[path = "all/ai_guardrail.rs"]
mod ai_guardrail;
#[path = "all/ai_guardrail_update.rs"]
mod ai_guardrail_update;
#[path = "all/assistant_ai_agent_binding.rs"]
mod assistant_ai_agent_binding;
#[path = "all/assistant_association.rs"]
mod assistant_association;
#[path = "all/assistant_configuration.rs"]
mod assistant_configuration;
#[path = "all/assistant_lifecycle.rs"]
mod assistant_lifecycle;
#[path = "all/authorization_precedence.rs"]
mod authorization_precedence;
#[path = "all/built_in_ai_agent_revisions.rs"]
mod built_in_ai_agent_revisions;
#[path = "all/content_precondition_message.rs"]
mod content_precondition_message;
#[path = "all/content_revisions.rs"]
mod content_revisions;
#[path = "all/content_search.rs"]
mod content_search;
#[path = "all/content_summary.rs"]
mod content_summary;
#[path = "all/content_upload.rs"]
mod content_upload;
#[path = "all/enum_value_refusal.rs"]
mod enum_value_refusal;
#[path = "all/idempotency_contents_and_refusal_fidelity.rs"]
mod idempotency_contents_and_refusal_fidelity;
#[path = "all/knowledge_base_lifecycle.rs"]
mod knowledge_base_lifecycle;
#[path = "all/knowledge_base_listing_and_queries.rs"]
mod knowledge_base_listing_and_queries;
#[path = "all/message_template.rs"]
mod message_template;
#[path = "all/message_template_versions.rs"]
mod message_template_versions;
#[path = "all/modelled_refusals.rs"]
mod modelled_refusals;
#[path = "all/notify_recommendations_error_text.rs"]
mod notify_recommendations_error_text;
#[path = "all/quick_response.rs"]
mod quick_response;
#[path = "all/quick_response_delete_reach.rs"]
mod quick_response_delete_reach;
#[path = "all/quick_response_mutations.rs"]
mod quick_response_mutations;
#[path = "all/quick_response_plane_gate.rs"]
mod quick_response_plane_gate;
#[path = "all/quick_response_search.rs"]
mod quick_response_search;
#[path = "all/relayed_knowledge_base_refusal.rs"]
mod relayed_knowledge_base_refusal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_managed_defaults.rs"]
mod service_managed_defaults;
#[path = "all/session_configuration.rs"]
mod session_configuration;
#[path = "all/session_recommendations.rs"]
mod session_recommendations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tagging_no_content_status.rs"]
mod tagging_no_content_status;
