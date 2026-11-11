// SPDX-License-Identifier: CC0-1.0

//! Intentionally empty, and not a module of `crate::state`.
//!
//! The artifact-store credentials a dispatched job carries live where the job and the
//! store that bounds them do: `state::jobs::mint_artifact_credentials` mints one,
//! `state::ComAmazonawsCodepipelineCodepipeline20150709State::set_artifact_credentials`
//! records it against the job, and `due_artifact_credentials` releases the ones the
//! clock has ended. This file is the vestige of a second implementation of that same
//! rule and carries no code, so nothing can drift from it.
