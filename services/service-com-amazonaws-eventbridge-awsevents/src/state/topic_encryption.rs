// SPDX-License-Identifier: CC0-1.0

//! TODO: delete this file. It is no part of the crate — nothing declares it as a
//! module — and it is kept here only because the change that superseded it could
//! not remove a file.
//!
//! Superseded by `target_encryption.rs`, which holds this concern for EVERY
//! target kind whose service encrypts what it is handed — an Amazon SQS queue as
//! well as an Amazon SNS topic — so the module is named for the target rather
//! than for one of them. The decision moved there unchanged, with the
//! via-service the use arrives through as a parameter.
