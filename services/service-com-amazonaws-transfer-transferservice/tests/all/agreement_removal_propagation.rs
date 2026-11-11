// SPDX-License-Identifier: CC0-1.0

//! Where an agreement the store no longer holds has gone: every plane that could still
//! answer for it.
//!
//! RULE under test: an AS2 agreement the store no longer holds — because
//! `DeleteAgreement` removed the `(ServerId, AgreementId)` pair the model makes
//! `AgreementResource`'s identifiers, or because `DeleteServer` took the server's
//! agreements with it — is held by NOTHING afterwards: every operation addressing the
//! pair answers the service's agreement not-found, and the tag plane no longer resolves
//! the agreement's own ARN, while whatever the delete did not name is left exactly as it
//! was. Nothing the removal frees is consumed either: the same server and the same pair
//! of profiles take a new agreement straight afterwards.
//!
//! The rows differ along what the rule generalises over: WHICH removal took the
//! agreement away, how the agreement was configured when it was removed (each directory
//! configuration, each enum member, with and without a tag set), which plane is asked
//! afterwards, and how many create/remove cycles one server has been through.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees rather than what a handler returns.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    assert_agreement_not_found, custom_directories, listed_tags, member_of, Answer, Emulator, ROLE,
};

/// Which removal took the agreement away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    /// `DeleteAgreement` on the pair itself.
    TheAgreement,
    /// `DeleteServer` on the server the agreement is written on, which takes the
    /// server's agreements with it.
    ItsServer,
}

impl Removal {
    fn label(self) -> &'static str {
        match self {
            Removal::TheAgreement => "DeleteAgreement",
            Removal::ItsServer => "DeleteServer",
        }
    }
}

/// How one row's agreement is configured: the members `CreateAgreement` takes beyond the
/// server and the two profiles (`smithy_query(query="operation",
/// operation="CreateAgreement")`), and whether it carries a tag set.
struct Configuration {
    case: &'static str,
    members: Value,
    /// The tags the agreement is created with, as the key -> value pairs they denote —
    /// empty for an agreement created without any.
    tags: &'static [(&'static str, &'static str)],
}

/// The configurations the rule is asserted over: both directory configurations, both
/// `AgreementStatusType` members, both `PreserveFilenameType` /
/// `EnforceMessageSigningType` members, and an agreement with and without tags.
fn configurations() -> Vec<Configuration> {
    vec![
        Configuration {
            case: "a base-directory agreement left at its defaults",
            members: json!({"BaseDirectory": "/amzn-s3-demo-bucket/as2"}),
            tags: &[],
        },
        Configuration {
            case: "a tagged, described, INACTIVE base-directory agreement",
            members: json!({
                "BaseDirectory": "/amzn-s3-demo-bucket/as2/inbound",
                "Description": "removal-propagation",
                "Status": "INACTIVE",
                "Tags": [
                    {"Key": "owner", "Value": "as2-team"},
                    {"Key": "stage", "Value": "test"},
                ],
            }),
            tags: &[("owner", "as2-team"), ("stage", "test")],
        },
        Configuration {
            case: "a tagged custom-directories agreement with both toggles ENABLED",
            members: json!({
                "CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/custom"),
                "Status": "ACTIVE",
                "PreserveFilename": "ENABLED",
                "EnforceMessageSigning": "ENABLED",
                "Tags": [{"Key": "owner", "Value": "as2-team"}],
            }),
            tags: &[("owner", "as2-team")],
        },
        Configuration {
            case: "a custom-directories agreement with both toggles DISABLED",
            members: json!({
                "CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/plain"),
                "PreserveFilename": "DISABLED",
                "EnforceMessageSigning": "DISABLED",
            }),
            tags: &[],
        },
    ]
}

/// The two profiles every agreement in this suite is written between.
struct Parties {
    local: String,
    partner: String,
}

impl Parties {
    async fn seed(emulator: &Emulator) -> Self {
        Self {
            local: emulator
                .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
                .await,
            partner: emulator
                .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
                .await,
        }
    }
}

/// Create one agreement on `server_id` from `members` and hand back its `AgreementId`.
async fn create_agreement(
    emulator: &Emulator,
    case: &str,
    server_id: &str,
    parties: &Parties,
    members: &Value,
) -> String {
    let mut request = json!({
        "ServerId": server_id,
        "LocalProfileId": &parties.local,
        "PartnerProfileId": &parties.partner,
        "AccessRole": ROLE,
    });
    let object = request.as_object_mut().expect("the request is an object");
    for (member, value) in members.as_object().expect("the members are an object") {
        object.insert(member.clone(), value.clone());
    }

    emulator.create_agreement(case, request).await
}

async fn describe_agreement(emulator: &Emulator, server_id: &str, agreement_id: &str) -> Answer {
    emulator.describe_agreement(server_id, agreement_id).await
}

/// The stored agreement's own ARN, read back through `DescribeAgreement` — the value the
/// tag plane addresses it by, taken from the service rather than built by the test.
async fn agreement_arn(
    emulator: &Emulator,
    case: &str,
    server_id: &str,
    agreement_id: &str,
) -> String {
    let read = describe_agreement(emulator, server_id, agreement_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected {agreement_id} to be readable before the removal, got {} \
         with body {}",
        read.status, read.body
    );
    member_of(read.get("Agreement"), "Arn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the agreement's Arn, got {}", read.body))
        .to_string()
}

/// Ask every plane that could still answer for the removed agreement, and assert each
/// one reports it gone.
async fn assert_held_by_nothing(
    emulator: &Emulator,
    case: &str,
    server_id: &str,
    agreement_id: &str,
    arn: &str,
) {
    assert_agreement_not_found(
        case,
        "DescribeAgreement",
        &describe_agreement(emulator, server_id, agreement_id).await,
        agreement_id,
    );
    assert_agreement_not_found(
        case,
        "DeleteAgreement",
        &emulator
            .call(
                "DeleteAgreement",
                json!({"ServerId": server_id, "AgreementId": agreement_id}),
            )
            .await,
        agreement_id,
    );
    assert_agreement_not_found(
        case,
        "UpdateAgreement",
        &emulator
            .call(
                "UpdateAgreement",
                json!({
                    "ServerId": server_id,
                    "AgreementId": agreement_id,
                    "Status": "INACTIVE",
                }),
            )
            .await,
        agreement_id,
    );

    // The tag plane addresses the agreement by its OWN ARN, so the ARN the create
    // minted resolves to nothing once the agreement is gone: a write through it is
    // refused with the same not-found, reported for the agreement kind.
    assert_agreement_not_found(
        case,
        "TagResource",
        &emulator
            .call(
                "TagResource",
                json!({"Arn": arn, "Tags": [{"Key": "owner", "Value": "as2-team"}]}),
            )
            .await,
        agreement_id,
    );
    assert_agreement_not_found(
        case,
        "UntagResource",
        &emulator
            .call("UntagResource", json!({"Arn": arn, "TagKeys": ["owner"]}))
            .await,
        agreement_id,
    );

    // And the tag set the agreement carried went with it, so the read of its ARN
    // reports no tags at all rather than the removed agreement's.
    let listed = listed_tags(emulator, case, arn).await;
    assert!(
        listed.is_empty(),
        "case {case:?}: expected the removed agreement's ARN to carry no tags, got {listed:?}"
    );
}

/// RULE: whichever removal took the agreement away, and however it was configured, every
/// plane answers it gone — while the agreements the removal did not take are untouched.
#[tokio::test]
async fn a_removed_agreement_is_held_by_no_plane_and_leaves_the_others() {
    let emulator = Emulator::new();
    let parties = Parties::seed(&emulator).await;

    // A server of its own per row, so the `DeleteServer` rows can take one without
    // disturbing the next row, plus one server that no row ever removes.
    let bystander_server = emulator.create_server(json!({})).await;
    let bystander_agreement = create_agreement(
        &emulator,
        "the agreement of the untouched server",
        &bystander_server,
        &parties,
        &json!({"BaseDirectory": "/amzn-s3-demo-bucket/as2/bystander"}),
    )
    .await;

    for removal in [Removal::TheAgreement, Removal::ItsServer] {
        for configuration in configurations() {
            let case = format!("{} removing {}", removal.label(), configuration.case);
            let server_id = emulator.create_server(json!({})).await;

            let agreement_id = create_agreement(
                &emulator,
                &case,
                &server_id,
                &parties,
                &configuration.members,
            )
            .await;
            // A SECOND agreement on the same server: a row's removal either leaves it
            // (the pair was not its own) or takes it with the server.
            let sibling = create_agreement(
                &emulator,
                &format!("{case}: the sibling agreement"),
                &server_id,
                &parties,
                &json!({"BaseDirectory": "/amzn-s3-demo-bucket/as2/sibling"}),
            )
            .await;

            let arn = agreement_arn(&emulator, &case, &server_id, &agreement_id).await;
            // Whatever the create was given is on the agreement's ARN before the
            // removal, so the empty read afterwards is the removal's doing.
            let before = listed_tags(&emulator, &case, &arn).await;
            let expected: BTreeMap<String, String> = configuration
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                before, expected,
                "case {case:?}: expected the created agreement's ARN to carry the tags it \
                 was created with, got {before:?}"
            );

            let removed = match removal {
                Removal::TheAgreement => {
                    emulator
                        .call(
                            "DeleteAgreement",
                            json!({"ServerId": &server_id, "AgreementId": &agreement_id}),
                        )
                        .await
                }
                Removal::ItsServer => {
                    emulator
                        .call("DeleteServer", json!({"ServerId": &server_id}))
                        .await
                }
            };
            assert_eq!(
                removed.status,
                200,
                "case {case:?}: expected {} 200, got {} with body {}",
                removal.label(),
                removed.status,
                removed.body
            );

            assert_held_by_nothing(&emulator, &case, &server_id, &agreement_id, &arn).await;

            // The sibling follows the removal's own reach: the pair-addressed delete took
            // only the pair it named, while the server's delete took every agreement
            // written on it.
            let sibling_read = describe_agreement(&emulator, &server_id, &sibling).await;
            match removal {
                Removal::TheAgreement => assert_eq!(
                    sibling_read.status, 200,
                    "case {case:?}: expected the sibling {sibling} to survive, got {} with \
                     body {}",
                    sibling_read.status, sibling_read.body
                ),
                Removal::ItsServer => assert_agreement_not_found(
                    &format!("{case}: the sibling agreement"),
                    "DescribeAgreement",
                    &sibling_read,
                    &sibling,
                ),
            }

            // The untouched server's agreement is still its own, whatever any row removed.
            let bystander_read =
                describe_agreement(&emulator, &bystander_server, &bystander_agreement).await;
            assert_eq!(
                bystander_read.status, 200,
                "case {case:?}: expected the untouched server's agreement to be readable, \
                 got {} with body {}",
                bystander_read.status, bystander_read.body
            );
        }
    }
}

/// RULE: a removal frees the agreement and consumes nothing else — the server and the two
/// profiles it was written between still take a NEW agreement, with an identifier of its
/// own, however many cycles the server has been through.
#[tokio::test]
async fn removing_an_agreement_frees_it_and_consumes_nothing_else() {
    let emulator = Emulator::new();
    let parties = Parties::seed(&emulator).await;
    let server_id = emulator.create_server(json!({})).await;

    let mut seen: Vec<String> = Vec::new();
    // The rows are how many create/delete cycles the same server has been through.
    for cycle in 1..=4u32 {
        let case = format!("create/delete cycle {cycle}");
        let agreement_id = create_agreement(
            &emulator,
            &case,
            &server_id,
            &parties,
            &json!({"BaseDirectory": format!("/amzn-s3-demo-bucket/as2/cycle-{cycle}")}),
        )
        .await;
        assert!(
            !seen.contains(&agreement_id),
            "case {case:?}: expected a fresh AgreementId, got the already-minted \
             {agreement_id} among {seen:?}"
        );
        seen.push(agreement_id.clone());

        let read = describe_agreement(&emulator, &server_id, &agreement_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the new agreement to be readable, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            member_of(read.get("Agreement"), "ServerId").as_str(),
            Some(server_id.as_str()),
            "case {case:?}: expected the new agreement to be written on the same server, \
             got {}",
            read.body
        );

        let deleted = emulator
            .call(
                "DeleteAgreement",
                json!({"ServerId": &server_id, "AgreementId": &agreement_id}),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteAgreement 200, got {} with body {}",
            deleted.status, deleted.body
        );

        // The parties and the server outlive every cycle: only the agreement went.
        for (member, profile_id) in [
            ("LocalProfileId", &parties.local),
            ("PartnerProfileId", &parties.partner),
        ] {
            let profile = emulator
                .call("DescribeProfile", json!({"ProfileId": profile_id}))
                .await;
            assert_eq!(
                profile.status, 200,
                "case {case:?}: expected the {member} profile {profile_id} to survive the \
                 delete, got {} with body {}",
                profile.status, profile.body
            );
        }
        let server = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            server.status, 200,
            "case {case:?}: expected the server to survive the delete, got {} with body {}",
            server.status, server.body
        );
    }
}
