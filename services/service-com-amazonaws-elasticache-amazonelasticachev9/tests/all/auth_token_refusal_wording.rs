// SPDX-License-Identifier: CC0-1.0

//! The two sentences an `AuthToken` is refused with, and which one each request gets.
//!
//! `AuthToken` is the password a client authenticates to the engine with, and the API states
//! two rules about it: it "can be specified only on replication groups where
//! TransitEncryptionEnabled is true", and the password itself "must be only printable ASCII
//! characters", "at least 16 characters and no more than 128 characters in length", with
//! "the only permitted printable special characters" being `!`, `&`, `#`, `$`, `^`, `<`, `>`
//! and `-`.
//!
//! ElastiCache answers each rule with ONE sentence, both as the modelled
//! `InvalidParameterValueException` (`awsQueryError` code `InvalidParameterValue`, HTTP 400):
//!
//! * a token sent for a group that does not encrypt in transit —
//!   `The AUTH token is only supported when encryption-in-transit is enabled`;
//! * a token that is not a password the engine accepts — `Invalid AuthToken provided.`,
//!   whichever of the password constraints it breaks.
//!
//! The rows span the constraints a token can break (too short, too long, a character outside
//! the permitted alphabet) and both writers of a replication group's password
//! (`CreateReplicationGroup` and `ModifyReplicationGroup`), with a VALID token on an
//! encrypted group as the control: the refusals are about the token and nothing else.

use crate::harness::{params, Emulator, Reply, PAST_TRANSITION};

/// The `awsQuery` error code both refusals carry, from the model:
/// `InvalidParameterValueException` has `awsQueryError` code `InvalidParameterValue` with
/// HTTP 400 (`smithy_query(query="operation",operation="CreateReplicationGroup")`).
const REFUSAL_CODE: &str = "InvalidParameterValue";

/// A password that satisfies every documented constraint: 18 characters out of the permitted
/// alphabet.
const GOOD_TOKEN: &str = "aVeryLongPassword1";

/// Tokens that break one documented constraint each, with the case name.
///
/// The length bounds are the documented 16 and 128, so one row is a character short of the
/// minimum and one a character past the maximum; the last carries a special character the
/// permitted set excludes (`*`).
const BAD_TOKENS: [(&str, &str); 4] = [
    ("one short of the minimum", "aPassword123456"),
    ("far shorter than the minimum", "short"),
    ("one past the maximum", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    ("a character outside the permitted set", "has*a*star*in*it*"),
];

/// Assert `reply` is the modelled refusal carrying `message`.
fn assert_refused(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case}: expected HTTP 400, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        REFUSAL_CODE,
        "case {case}: expected {REFUSAL_CODE}, got {:?} ({})",
        reply.error_code(),
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case}: expected {message:?}, got {:?}",
        reply.error_message()
    );
}

/// One `CreateReplicationGroup` with the pairs a row sends.
async fn create(emulator: &Emulator, id: &str, extra: &[(&str, &str)]) -> Reply {
    let mut request = params(&[
        ("ReplicationGroupId", id),
        ("ReplicationGroupDescription", "the AuthToken rules"),
    ]);
    request.extend(params(extra));
    emulator.call("CreateReplicationGroup", &request).await
}

#[tokio::test]
async fn a_token_the_engine_would_not_accept_is_refused_with_one_sentence() {
    // The sentence AWS answers for a password that breaks any of the documented
    // constraints, at both writers of one.
    let expected = "Invalid AuthToken provided.";
    let emulator = Emulator::new();
    // The control: the same request with a valid token is accepted, so each refusal below
    // is about the token.
    let accepted = create(
        &emulator,
        "token-rg",
        &[
            ("TransitEncryptionEnabled", "true"),
            ("AuthToken", GOOD_TOKEN),
        ],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a valid token on an encrypted group must be accepted, got {} {}",
        accepted.status, accepted.body
    );
    std::thread::sleep(PAST_TRANSITION);

    for (index, (case, token)) in BAD_TOKENS.iter().enumerate() {
        let refused = create(
            &emulator,
            &format!("bad-token-rg-{index}"),
            &[("TransitEncryptionEnabled", "true"), ("AuthToken", token)],
        )
        .await;
        assert_refused(&format!("CreateReplicationGroup {case}"), &refused, expected);
        // A refused create leaves no group behind.
        let described = emulator
            .call(
                "DescribeReplicationGroups",
                &params(&[("ReplicationGroupId", &format!("bad-token-rg-{index}"))]),
            )
            .await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "ReplicationGroupNotFoundFault"),
            "case {case}: a refused create must create nothing, got {} {}",
            described.status,
            described.body
        );

        let refused = emulator
            .call(
                "ModifyReplicationGroup",
                &params(&[
                    ("ReplicationGroupId", "token-rg"),
                    ("AuthToken", token),
                    ("AuthTokenUpdateStrategy", "ROTATE"),
                    ("ApplyImmediately", "true"),
                ]),
            )
            .await;
        assert_refused(&format!("ModifyReplicationGroup {case}"), &refused, expected);
    }
}

#[tokio::test]
async fn a_token_without_encryption_in_transit_is_refused_with_the_other_sentence() {
    // The password is what the connection cannot carry unencrypted, so the refusal names the
    // AUTH token rather than the encryption member — and it is the same sentence at both
    // writers.
    let expected = "The AUTH token is only supported when encryption-in-transit is enabled";
    let emulator = Emulator::new();
    for (index, (case, token)) in [("a valid token", GOOD_TOKEN), BAD_TOKENS[0]]
        .into_iter()
        .enumerate()
    {
        // Whether the token itself is a password the engine would accept makes no
        // difference: without encryption in transit it cannot be set at all.
        let refused = create(
            &emulator,
            &format!("plain-token-rg-{index}"),
            &[("AuthToken", token)],
        )
        .await;
        assert_refused(&format!("CreateReplicationGroup {case}"), &refused, expected);
        let refused = create(
            &emulator,
            &format!("plain-token-off-rg-{index}"),
            &[("TransitEncryptionEnabled", "false"), ("AuthToken", token)],
        )
        .await;
        assert_refused(
            &format!("CreateReplicationGroup {case} with encryption off"),
            &refused,
            expected,
        );
    }

    // The same at the modify: a group created without encryption in transit cannot take a
    // password either.
    let created = create(&emulator, "plain-rg", &[]).await;
    assert_eq!(
        created.status, 200,
        "the modify row needs its group, got {} {}",
        created.status, created.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let refused = emulator
        .call(
            "ModifyReplicationGroup",
            &params(&[
                ("ReplicationGroupId", "plain-rg"),
                ("AuthToken", GOOD_TOKEN),
                ("AuthTokenUpdateStrategy", "ROTATE"),
                ("ApplyImmediately", "true"),
            ]),
        )
        .await;
    assert_refused("ModifyReplicationGroup a valid token", &refused, expected);
}
