// SPDX-License-Identifier: CC0-1.0

//! Which connector PROTOCOL each operation of the connector plane can be asked of.
//!
//! RULE under test: the operations this service performs through a connection to the
//! partner's REMOTE SFTP SERVER — the remote move, the remote delete, the directory
//! listing, and the per-file results of a transfer — are not supported for an AS2
//! connector, which configures no such server: the request is refused with the modelled
//! `InvalidRequestException` (HTTP 400) in that operation's fixed text, as soon as the
//! connector resolves and BEFORE anything inside it is looked up, and nothing is minted
//! or recorded. The same request on an SFTP connector is served.
//!
//! The rows span the four operations, both connector kinds, and — for the operation that
//! also takes an identifier inside the connector — a `TransferId` that exists and one
//! that does not, because the gate is a fact about the CONNECTOR and so cannot depend on
//! either.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{member_of, Answer, Emulator, ROLE};

/// A `ProfileId` satisfying the modelled `^p-([0-9a-f]{17})$`, for the AS2 connector's
/// configuration.
const LOCAL_PROFILE: &str = "p-0123456789abcdef0";
const PARTNER_PROFILE: &str = "p-00000000000000001";

/// A `UserSecretId` satisfying the member's modelled `len 1..2048`.
const SECRET: &str = "arn:aws:secretsmanager:us-east-1:000000000000:secret:sftp-partner";

/// A `ConnectorId` satisfying the modelled `^c-([0-9a-f]{17})$` that no create minted.
const NEVER_MINTED_CONNECTOR: &str = "c-0000000000000dead";

/// A `TransferId` satisfying the modelled `^[0-9a-zA-Z./-]+$` that no `Start*` answered.
const NEVER_ACCEPTED_TRANSFER: &str = "0123456789abcdef";

/// Create one connector over the wire and hand back its `ConnectorId`.
async fn create_connector(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("CreateConnector", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateConnector 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectorId, got {}", answer.body))
        .to_string()
}

/// An SFTP connector: one that configures a connection to a remote SFTP server.
async fn sftp_connector(emulator: &Emulator, case: &str) -> String {
    create_connector(
        emulator,
        case,
        json!({
            "AccessRole": ROLE,
            "Url": "sftp://partner.example.com",
            "SftpConfig": {"UserSecretId": SECRET},
        }),
    )
    .await
}

/// An AS2 connector: one that configures an AS2 exchange and no SFTP server at all.
async fn as2_connector(emulator: &Emulator, case: &str) -> String {
    create_connector(
        emulator,
        case,
        json!({
            "AccessRole": ROLE,
            "Url": "https://partner.example.com/as2",
            "As2Config": {
                "LocalProfileId": LOCAL_PROFILE,
                "PartnerProfileId": PARTNER_PROFILE,
            },
        }),
    )
    .await
}

/// The `Message` an answer carries, or `""`.
fn message_of(answer: &Answer) -> String {
    member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Accept one outbound transfer on `connector_id` and hand back its `TransferId`.
///
/// An AS2 connector CAN be asked to send files ("for an AS2 connector, you specify the
/// `ConnectorId` and one or more `SendFilePaths`"), so both kinds hold records — which is
/// what lets the rows below ask for the results of a record that really exists.
async fn accept_transfer(emulator: &Emulator, case: &str, connector_id: &str) -> String {
    let answer = emulator
        .call(
            "StartFileTransfer",
            json!({"ConnectorId": connector_id, "SendFilePaths": ["bucket/invoice.edi"]}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected StartFileTransfer 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("TransferId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a TransferId, got {}", answer.body))
        .to_string()
}

/// RULE: each remote-SFTP operation of this plane is refused for an AS2 connector with
/// 400 `InvalidRequestException` and a text fixed for that operation, and is served for
/// an SFTP connector.
#[tokio::test]
async fn the_remote_sftp_operations_are_refused_for_an_as2_connector() {
    let emulator = Emulator::new();
    let sftp = sftp_connector(&emulator, "the SFTP connector").await;
    let as2 = as2_connector(&emulator, "the AS2 connector").await;

    // The record each kind of connector holds, so the results rows address one that
    // exists on the connector they name.
    let sftp_transfer = accept_transfer(&emulator, "the SFTP connector's transfer", &sftp).await;
    let as2_transfer = accept_transfer(&emulator, "the AS2 connector's transfer", &as2).await;

    // (case, operation, the members other than ConnectorId for the AS2 request, the same
    // for the SFTP request, the output member a served call answers with — `None` for
    // the listing of results, which answers a collection rather than an identifier)
    let rows: Vec<(&str, &str, Value, Value, Option<&str>)> = vec![
        (
            "a remote move",
            "StartRemoteMove",
            json!({"SourcePath": "/outbox/report.csv", "TargetPath": "/archive/report.csv"}),
            json!({"SourcePath": "/outbox/report.csv", "TargetPath": "/archive/report.csv"}),
            Some("MoveId"),
        ),
        (
            "a remote delete",
            "StartRemoteDelete",
            json!({"DeletePath": "/outbox/report.csv"}),
            json!({"DeletePath": "/outbox/report.csv"}),
            Some("DeleteId"),
        ),
        (
            "a directory listing",
            "StartDirectoryListing",
            json!({
                "RemoteDirectoryPath": "/outbox",
                "OutputDirectoryPath": "/bucket/listings",
            }),
            json!({
                "RemoteDirectoryPath": "/outbox",
                "OutputDirectoryPath": "/bucket/listings",
            }),
            Some("ListingId"),
        ),
        (
            "the results of a transfer the connector accepted",
            "ListFileTransferResults",
            json!({"TransferId": &as2_transfer}),
            json!({"TransferId": &sftp_transfer}),
            None,
        ),
        (
            "the results of a transfer the connector never accepted",
            "ListFileTransferResults",
            json!({"TransferId": NEVER_ACCEPTED_TRANSFER}),
            // The SFTP half of this row is the record that DOES exist: what the row
            // varies is the AS2 side, where the gate precedes the lookup.
            json!({"TransferId": &sftp_transfer}),
            None,
        ),
    ];

    let mut refusals: Vec<(String, String)> = Vec::new();

    for (case, op, as2_members, sftp_members, minted) in rows {
        // Refused for the AS2 connector, whatever the request names inside it.
        let mut request = json!({"ConnectorId": &as2});
        let object = request.as_object_mut().expect("the request is an object");
        for (member, value) in as2_members.as_object().expect("the row is an object") {
            object.insert(member.clone(), value.clone());
        }
        let answer = emulator.call(op, request).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?} on the AS2 connector: expected 400 InvalidRequestException, got {} \
             with body {}",
            answer.status,
            answer.body
        );
        let message = message_of(&answer);
        assert!(
            !message.is_empty() && !message.contains(as2.as_str()),
            "case {case:?}: expected a non-empty Message carrying no ConnectorId, got \
             {message:?}"
        );
        if let Some(member) = minted {
            assert!(
                member_of(&answer.body, member).is_null(),
                "case {case:?}: expected the refusal to mint no {member}, got {}",
                answer.body
            );
        }
        refusals.push((format!("{op}: {case}"), message));

        // Served for the SFTP connector: the same operation, the same shape of request.
        let mut request = json!({"ConnectorId": &sftp});
        let object = request.as_object_mut().expect("the request is an object");
        for (member, value) in sftp_members.as_object().expect("the row is an object") {
            object.insert(member.clone(), value.clone());
        }
        let answer = emulator.call(op, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} on the SFTP connector: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        if let Some(member) = minted {
            assert!(
                answer
                    .get(member)
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                "case {case:?}: expected the served call to answer a {member}, got {}",
                answer.body
            );
        }
    }

    // One text per OPERATION: the two rows of the same operation read identically (the
    // refusal reports what is unsupported, not what the request named), and the
    // operations are told apart by the action their text names.
    let results_texts: Vec<&(String, String)> = refusals
        .iter()
        .filter(|(case, _)| case.starts_with("ListFileTransferResults"))
        .collect();
    assert_eq!(
        results_texts.len(),
        2,
        "the table must ask for results twice, got {results_texts:?}"
    );
    assert_eq!(
        results_texts[0].1, results_texts[1].1,
        "case {:?} beside {:?}: expected one text per operation, got {:?} and {:?}",
        results_texts[0].0, results_texts[1].0, results_texts[0].1, results_texts[1].1
    );
    let mut distinct: Vec<&String> = refusals.iter().map(|(_, message)| message).collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        4,
        "expected one distinct text per operation gated here, got {distinct:?}"
    );

    // Nothing the refusals touched: the AS2 connector's own accepted transfer is still
    // the record it was, read through the connector the operation IS supported for.
    let served = emulator
        .call(
            "ListFileTransferResults",
            json!({"ConnectorId": &sftp, "TransferId": &sftp_transfer}),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "case \"the SFTP connector's transfer after the refusals\": expected 200, got {} \
         with body {}",
        served.status, served.body
    );
    assert_eq!(
        member_of(&served.body, "FileTransferResults")
            .as_array()
            .map(Vec::len),
        Some(1),
        "case \"the SFTP connector's transfer after the refusals\": expected its one file \
         still reported, got {}",
        served.body
    );
}

/// RULE (the order of the two refusals): the CONNECTOR is resolved first, so a connector
/// the store does not hold is reported as the missing resource it is — the protocol gate
/// only decides a connector that exists.
#[tokio::test]
async fn a_connector_the_store_does_not_hold_is_still_reported_as_missing() {
    let emulator = Emulator::new();

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a remote move",
            "StartRemoteMove",
            json!({"SourcePath": "/a", "TargetPath": "/b"}),
        ),
        (
            "a remote delete",
            "StartRemoteDelete",
            json!({"DeletePath": "/a"}),
        ),
        (
            "a directory listing",
            "StartDirectoryListing",
            json!({"RemoteDirectoryPath": "/a", "OutputDirectoryPath": "/bucket/out"}),
        ),
        (
            "the results of a transfer",
            "ListFileTransferResults",
            json!({"TransferId": NEVER_ACCEPTED_TRANSFER}),
        ),
    ];

    for (case, op, members) in rows {
        let mut request = json!({"ConnectorId": NEVER_MINTED_CONNECTOR});
        let object = request.as_object_mut().expect("the request is an object");
        for (member, value) in members.as_object().expect("the row is an object") {
            object.insert(member.clone(), value.clone());
        }
        let answer = emulator.call(op, request).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected 400 ResourceNotFoundException, got {} with body {}",
            answer.status,
            answer.body
        );
        assert_eq!(
            member_of(&answer.body, "ResourceType").as_str(),
            Some("Connector"),
            "case {case:?}: expected the missing connector to be reported, got {}",
            answer.body
        );
    }
}
