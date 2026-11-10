// SPDX-License-Identifier: CC0-1.0

//! `DeleteAgreement`: what removing an AS2 agreement leaves behind.
//!
//! RULE under test: `DeleteAgreement` removes the agreement the `(ServerId,
//! AgreementId)` pair addresses — the pair the model makes `AgreementResource`'s
//! identifiers — so afterwards every reader of that agreement answers the service's
//! not-found refusal and the pair is free of it, while every OTHER agreement is left
//! exactly as it was. A pair the store does not hold is refused with that same
//! not-found, which is what a second delete of the same agreement gets.
//!
//! The rows differ along what the rule generalises over: how many agreements the server
//! held when one was deleted, which of them was deleted (the first, a middle one, the
//! last), which reader is asked afterwards, and whether the pair was never created, was
//! just deleted, or names an agreement of ANOTHER server.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::json;

use crate::harness::{is_modelled_agreement_id, member_of, Answer, Emulator, ROLE};

/// An `AgreementId` of the modelled form (`^a-([0-9a-f]{17})$`) that no create minted.
const NEVER_MINTED: [&str; 3] = [
    "a-00000000000000000",
    "a-0000000000000dead",
    "a-fffffffffffffffff",
];

/// Create one agreement on `server_id` between the two profiles and hand back its
/// `AgreementId`.
async fn create_agreement(
    emulator: &Emulator,
    case: &str,
    server_id: &str,
    local_profile: &str,
    partner_profile: &str,
) -> String {
    let answer = emulator
        .call(
            "CreateAgreement",
            json!({
                "ServerId": server_id,
                "LocalProfileId": local_profile,
                "PartnerProfileId": partner_profile,
                "AccessRole": ROLE,
                "BaseDirectory": "/amzn-s3-demo-bucket/as2",
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgreement 200, got {} with body {}",
        answer.status, answer.body
    );
    let agreement_id = answer
        .get("AgreementId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an AgreementId, got {}", answer.body))
        .to_string();
    assert!(
        is_modelled_agreement_id(&agreement_id),
        "case {case:?}: expected an AgreementId matching ^a-([0-9a-f]{{17}}), got \
         {agreement_id:?}"
    );
    agreement_id
}

/// Assert `answer` is this service's not-found refusal for the agreement `agreement_id`.
fn assert_agreement_not_found(case: &str, op: &str, answer: &Answer, agreement_id: &str) {
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (400, "ResourceNotFoundException"),
        "case {case:?}: expected {op} to answer 400 ResourceNotFoundException, got {} with \
         body {}",
        answer.status,
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "ResourceType").as_str(),
        Some("Agreement"),
        "case {case:?}: expected {op} to report the agreement kind, got {}",
        answer.body
    );
    assert_eq!(
        member_of(&answer.body, "Resource").as_str(),
        Some(agreement_id),
        "case {case:?}: expected {op} to report the requested identifier, got {}",
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !message.is_empty() && !message.contains(agreement_id),
        "case {case:?}: expected {op} to answer a non-empty Message carrying no \
         identifier, got {message:?}"
    );
}

/// Describe one agreement, answering the whole response.
async fn describe(emulator: &Emulator, server_id: &str, agreement_id: &str) -> Answer {
    emulator
        .call(
            "DescribeAgreement",
            json!({"ServerId": server_id, "AgreementId": agreement_id}),
        )
        .await
}

/// RULE: the delete removes the agreement it names and only that one, and every reader
/// of it answers not found afterwards.
///
/// The rows are (how many agreements the server held, which index was deleted).
#[tokio::test]
async fn deleting_an_agreement_removes_it_from_every_reader_and_leaves_the_others() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let local_profile = emulator
        .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
        .await;
    let partner_profile = emulator
        .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
        .await;

    // (how many the server holds, the index this row deletes)
    let rows: [(usize, usize); 4] = [(1, 0), (2, 0), (3, 1), (3, 2)];

    for (held, doomed_index) in rows {
        let case = format!("deleting agreement {doomed_index} of {held}");
        let mut agreements: Vec<String> = Vec::with_capacity(held);
        for index in 0..held {
            agreements.push(
                create_agreement(
                    &emulator,
                    &format!("{case}: agreement {index}"),
                    &server_id,
                    &local_profile,
                    &partner_profile,
                )
                .await,
            );
        }
        let doomed = agreements.remove(doomed_index);

        // Every agreement the row created is readable before the delete.
        for agreement_id in agreements.iter().chain([&doomed]) {
            let read = describe(&emulator, &server_id, agreement_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected {agreement_id} to be readable before the delete, \
                 got {} with body {}",
                read.status, read.body
            );
        }

        let deleted = emulator
            .call(
                "DeleteAgreement",
                json!({"ServerId": &server_id, "AgreementId": &doomed}),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteAgreement 200, got {} with body {}",
            deleted.status, deleted.body
        );

        // The deleted agreement is gone from every reader of it, including a second
        // delete and the tag plane (whose ARN resolution reads the same records).
        assert_agreement_not_found(
            &case,
            "DescribeAgreement",
            &describe(&emulator, &server_id, &doomed).await,
            &doomed,
        );
        assert_agreement_not_found(
            &case,
            "DeleteAgreement",
            &emulator
                .call(
                    "DeleteAgreement",
                    json!({"ServerId": &server_id, "AgreementId": &doomed}),
                )
                .await,
            &doomed,
        );
        assert_agreement_not_found(
            &case,
            "UpdateAgreement",
            &emulator
                .call(
                    "UpdateAgreement",
                    json!({
                        "ServerId": &server_id,
                        "AgreementId": &doomed,
                        "Status": "INACTIVE",
                    }),
                )
                .await,
            &doomed,
        );

        // Every other agreement of the server is untouched, and the delete took the
        // whole record with it — not only the reader's view of it.
        for agreement_id in &agreements {
            let read = describe(&emulator, &server_id, agreement_id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected {agreement_id} to survive the delete, got {} with \
                 body {}",
                read.status, read.body
            );
            assert_eq!(
                member_of(read.get("Agreement"), "AgreementId").as_str(),
                Some(agreement_id.as_str()),
                "case {case:?}: expected {agreement_id} to read back as itself, got {}",
                read.body
            );
        }

        // Clean the server for the next row.
        for agreement_id in &agreements {
            let answer = emulator
                .call(
                    "DeleteAgreement",
                    json!({"ServerId": &server_id, "AgreementId": agreement_id}),
                )
                .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the teardown delete of {agreement_id} to succeed, \
                 got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}

/// RULE: a `(ServerId, AgreementId)` pair the store does not hold is refused with the
/// agreement not-found, whether the id was never minted or belongs to ANOTHER server.
#[tokio::test]
async fn deleting_an_agreement_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let other_server = emulator.create_server(json!({})).await;
    let local_profile = emulator
        .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
        .await;
    let partner_profile = emulator
        .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
        .await;
    let held = create_agreement(
        &emulator,
        "the agreement of the other server",
        &other_server,
        &local_profile,
        &partner_profile,
    )
    .await;

    let mut rows: Vec<(String, String)> = NEVER_MINTED
        .iter()
        .map(|agreement_id| {
            (
                format!("the never-minted {agreement_id}"),
                (*agreement_id).to_string(),
            )
        })
        .collect();
    rows.push((
        "an agreement of another server".to_string(),
        held.clone(),
    ));

    for (case, agreement_id) in &rows {
        let answer = emulator
            .call(
                "DeleteAgreement",
                json!({"ServerId": &server_id, "AgreementId": agreement_id}),
            )
            .await;
        assert_agreement_not_found(case, "DeleteAgreement", &answer, agreement_id);
    }

    // The other server's agreement is still its own: a refused delete removed nothing.
    let read = describe(&emulator, &other_server, &held).await;
    assert_eq!(
        read.status, 200,
        "case \"the other server's agreement after the refusals\": expected 200, got {} \
         with body {}",
        read.status, read.body
    );
}
