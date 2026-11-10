// SPDX-License-Identifier: CC0-1.0

//! Which connector PROTOCOL an operation of the connector plane can be asked of.
//!
//! A connector carries exactly one protocol configuration — "you must specify exactly
//! one configuration, either for AS2 (`As2Config`) or SFTP (`SftpConfig`)" — and part
//! of this plane acts on the partner's REMOTE SFTP SERVER rather than on an AS2
//! exchange: `StartRemoteMove` "moves or renames a file or directory on the remote SFTP
//! server", `StartRemoteDelete` "deletes a file or directory on the remote SFTP
//! server", `StartDirectoryListing` lists "a directory from a remote SFTP server", and
//! the per-file results of `ListFileTransferResults` are reported for SFTP connectors.
//! An AS2 connector configures no such server, so the service does not perform those
//! operations for one at all: the request is refused with the modelled
//! `InvalidRequestException` (HTTP 400, "thrown when the client submits a malformed
//! request").
//!
//! The gate is on the CONNECTOR's protocol, so it is decided as soon as the connector
//! resolves — BEFORE anything inside it (a `TransferId`, a path) is looked up, since an
//! operation the connector cannot perform is not a question about its records. Each
//! operation states the refusal in its own fixed text, carrying nothing the caller
//! sent.

use super::*;

/// `ListFileTransferResults` asked of an AS2 connector: the service's fixed text,
/// naming the operation that is not supported.
pub const LIST_FILE_TRANSFER_RESULTS_UNSUPPORTED: &str =
    "ListFileTransferResults is not supported for AS2 connectors";

/// `StartRemoteMove` asked of an AS2 connector: the service's fixed text, naming the
/// remote operation that is not supported.
pub const REMOTE_MOVE_UNSUPPORTED: &str = "Remote move not supported for AS2 connectors";

/// `StartRemoteDelete` asked of an AS2 connector.
///
/// UNVERIFIED wording: nothing recorded states this operation's refusal, so the text
/// follows the form the service states for the SIBLING remote operation
/// ([`REMOTE_MOVE_UNSUPPORTED`]) — the remote action, then that it is not supported for
/// AS2 connectors — with this operation's action. The service is not uniform in that form
/// ([`LIST_FILE_TRANSFER_RESULTS_UNSUPPORTED`] names the operation instead), so a
/// recording of this refusal should correct this constant; what IS established is the
/// refusal itself, its class and its status.
pub const REMOTE_DELETE_UNSUPPORTED: &str = "Remote delete not supported for AS2 connectors";

/// `StartDirectoryListing` asked of an AS2 connector, on the same form and equally
/// UNVERIFIED as [`REMOTE_DELETE_UNSUPPORTED`].
pub const DIRECTORY_LISTING_UNSUPPORTED: &str =
    "Directory listing not supported for AS2 connectors";

/// Whether `connector` is one the remote-SFTP half of this plane can be asked of, i.e.
/// configures an SFTP connection ("for an SFTP connector, the file transfer can be
/// either outbound or inbound"). A connector configures exactly one protocol, so a
/// connector that configures no SFTP connection is an AS2 one.
pub fn serves_remote_sftp_operations(connector: &Connector) -> bool {
    connector.sftp_config.is_some()
}

/// The refusal for an operation of this plane asked of an AS2 connector: the modelled
/// `InvalidRequestException` carrying that operation's fixed text.
pub fn unsupported_for_as2_connector(stated: &'static str) -> InvalidRequestException {
    InvalidRequestException {
        message: stated.to_string(),
    }
}
