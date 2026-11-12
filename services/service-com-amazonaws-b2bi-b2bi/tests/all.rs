// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/authorization.rs"]
mod authorization;
#[path = "all/aws_worded_refusals.rs"]
mod aws_worded_refusals;
#[path = "all/canonical_document.rs"]
mod canonical_document;
#[path = "all/client_error_bodies.rs"]
mod client_error_bodies;
#[path = "all/delete_transformer.rs"]
mod delete_transformer;
#[path = "all/edi_parsing.rs"]
mod edi_parsing;
#[path = "all/generate_mapping.rs"]
mod generate_mapping;
#[path = "all/idempotent_create_in_flight_window.rs"]
mod idempotent_create_in_flight_window;
#[path = "all/jsonata_probe.rs"]
mod jsonata_probe;
#[path = "all/mapped_file_encoding.rs"]
mod mapped_file_encoding;
#[path = "all/not_found_status.rs"]
mod not_found_status;
#[path = "all/onboarding_loop.rs"]
mod onboarding_loop;
#[path = "all/profile_filter_and_job_locations.rs"]
mod profile_filter_and_job_locations;
#[path = "all/references_logging_and_quota.rs"]
mod references_logging_and_quota;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/starter_mapping_template.rs"]
mod starter_mapping_template;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tag_type_discriminator.rs"]
mod tag_type_discriminator;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/tagging_and_test_harness.rs"]
mod tagging_and_test_harness;
#[path = "all/test_conversion.rs"]
mod test_conversion;
#[path = "all/test_mapping.rs"]
mod test_mapping;
#[path = "all/test_parsing.rs"]
mod test_parsing;
#[path = "all/transformer_jobs.rs"]
mod transformer_jobs;
#[path = "all/transformer_lifecycle.rs"]
mod transformer_lifecycle;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_capability.rs"]
mod update_capability;
#[path = "all/update_partnership.rs"]
mod update_partnership;
#[path = "all/update_partnership_refusal_status.rs"]
mod update_partnership_refusal_status;
#[path = "all/update_transformer.rs"]
mod update_transformer;
#[path = "all/wire_response_text.rs"]
mod wire_response_text;
#[path = "all/wire_timestamps_and_discriminators.rs"]
mod wire_timestamps_and_discriminators;
#[path = "all/list_transformers.rs"]
mod list_transformers;
