// SPDX-License-Identifier: CC0-1.0

//! REMOVED — this file is intentionally empty. It carries no test; it stays declared in
//! `tests/all.rs` only because the crate's layout check refuses an undeclared file under
//! `tests/all/`, and an empty module compiles to nothing.
//!
//! It briefly held a throw-away diagnostic that walked the metric-stream export pipeline
//! stage by stage — the stack, the role IAM holds for it, the two `SimulatePrincipalPolicy`
//! answers, the delivery stream, a direct `PutRecord` and the destination listing — to find
//! which stage of `all/cloudformation_metric_stream_exports_to_s3.rs` was not delivering.
//! The answer was the suite's own buffering: a destination declaring
//! `IntervalInSeconds: 60` is delivered 60 s after the put, which that suite's delivery
//! budget does not cover, so nothing was wrong with the pipeline. That suite now asks for
//! the shortest buffering there is, exactly as its sibling delivery suites do, and nothing
//! here is needed to pin it.
//!
//! The content is gone rather than the file because this fixer's tool set can write a file
//! but not remove one; the next change to touch this directory should delete it.
