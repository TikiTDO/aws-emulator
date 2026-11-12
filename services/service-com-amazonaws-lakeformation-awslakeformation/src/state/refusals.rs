// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal: the message each error class carries.
//!
//! An HTTP status, an error code and an error message are all part of the contract a client
//! branches on, so the wording of a refusal lives here — one constant per error class,
//! shared by every operation that answers it — rather than being spelled at each site that
//! raises it.
//!
//! Lake Formation words "the thing you addressed is not there" with a sentence that names
//! neither the name the request carried nor the kind of thing it addressed, so each of
//! these is wholly fixed text.

/// What a read or an update of a data cells filter the store does not hold answers.
///
/// Wholly fixed text: it names neither the filter nor the table, so the same sentence
/// answers every such call.
pub const FILTER_ENTITY_NOT_FOUND: &str = "Entity Not Found";

/// What a read of a data-lake location that is not registered answers.
///
/// Wholly fixed text, like [`FILTER_ENTITY_NOT_FOUND`] — and in its own casing, which is
/// the one this service words that resource's absence in.
pub const RESOURCE_ENTITY_NOT_FOUND: &str = "Entity not found";

/// What an operation addressing an LF-tag key the Data Catalog does not hold answers: the
/// readers, the update and the delete alike.
///
/// Wholly fixed text: the key the request carried is not echoed.
pub const TAG_KEY_DOES_NOT_EXIST: &str = "Tag key does not exist";

/// What a create of a data cells filter the store already holds answers.
///
/// Wholly fixed text, like the absence above: neither the filter nor the table it is on is
/// named.
pub const FILTER_ALREADY_EXISTS: &str = "Data cells filter already exists.";

/// What a register of a data-lake location that is already registered answers.
///
/// Wholly fixed text: the ARN the request carried is not echoed.
pub const RESOURCE_ALREADY_REGISTERED: &str = "Resource is already registered";

/// What an operation handed a `ResourceArn` that is not an Amazon Resource Name answers.
///
/// Wholly fixed text: the value the request carried is not echoed.
pub const UNSUPPORTED_RESOURCE_ARN: &str = "Un-supported resource arn format";

/// What a create of a data cells filter whose `RowFilter` names both of its alternatives
/// answers.
///
/// A PartiQL predicate is an expression OR the all-rows wildcard, never both, and the
/// refusal is this sentence whatever the expression was.
pub const ROW_FILTER_BOTH_SET: &str = "Filter expression and AllRowsWildcard cannot both be set.";

/// What a create of an LF-tag whose key the Data Catalog already holds answers.
///
/// Wholly fixed text: neither the key nor the catalog is named.
pub const TAG_KEY_ALREADY_EXISTS: &str = "Tag key already exists";
