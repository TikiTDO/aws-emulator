// SPDX-License-Identifier: CC0-1.0

//! Deliberately empty: this file held the probe that located the trigger of the data
//! protected log group's rolled-back stack while it was being written, and the suite that
//! now pins that rule lives under the name it deserves,
//! `logs_group_children_resources.rs`.
//!
//! The file stays declared in `tests/all.rs` because this crate's build script requires
//! every file under `tests/all/` to be declared with a module of the same name, and the
//! fixer tooling that wrote it can create and rewrite a file but cannot remove one — the
//! same reason `scratch.rs` beside it stays.
