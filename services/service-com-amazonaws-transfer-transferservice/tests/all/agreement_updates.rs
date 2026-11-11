// SPDX-License-Identifier: CC0-1.0

//! What `UpdateAgreement` persists, and what the read that follows it reports.
//!
//! RULE: `UpdateAgreement` applies to the stored agreement every member the request
//! names and leaves every member it does not name as the agreement already had it, so
//! `DescribeAgreement` answers the UPDATED record rather than the one the create wrote.
//! The two directory configurations are alternatives, so the one an update names
//! replaces the other ("if you update an agreement from using base directory to custom
//! directories, the base directory is no longer used", and the reverse), and a request
//! naming both is refused. A refused update — an unresolvable `(ServerId, AgreementId)`
//! pair, a profile id the store cannot resolve, both directory configurations at once —
//! changes nothing the following read sees.
//!
//! The rows differ along what the rule generalises over: WHICH member the update names,
//! each enum member's whole modelled value set, both directions of the status change,
//! the directory configuration the agreement starts with and the one it is moved to, the
//! length and form of the values (including both published `BaseDirectory` bounds and
//! the blank value that names no directory), and how many members one update names at
//! once.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees rather than what a handler returns.

use serde_json::{json, Value};

use crate::harness::{
    assert_members, custom_directories, is_modelled_agreement_id, member_of, Answer, Emulator,
    ROLE,
};

/// `AgreementStatusType`'s whole modelled value set
/// (`smithy_query(query="shape",shape="AgreementStatusType")` reports
/// `[ACTIVE, INACTIVE]`).
const AGREEMENT_STATUSES: [&str; 2] = ["ACTIVE", "INACTIVE"];

/// `PreserveFilenameType`'s and `EnforceMessageSigningType`'s whole modelled value set
/// (both report `[ENABLED, DISABLED]`).
const TOGGLE_VALUES: [&str; 2] = ["ENABLED", "DISABLED"];

/// The `Status` an agreement created without one carries, and the `PreserveFilename` /
/// `EnforceMessageSigning` the model documents as "(default value)".
const DEFAULT_STATUS: &str = "ACTIVE";
const DEFAULT_TOGGLE: &str = "DISABLED";

/// The `Description` every baseline agreement in this suite is created with, so a row
/// that does not name one reads it back unchanged.
const BASELINE_DESCRIPTION: &str = "baseline-agreement";

/// The `BaseDirectory` the baseline agreement of a row that starts with one carries.
const BASELINE_BASE_DIRECTORY: &str = "/amzn-s3-demo-bucket/home/baseline";

/// The `CustomDirectories` prefix the baseline agreement of a row that starts with them
/// carries.
const BASELINE_CUSTOM_PREFIX: &str = "/amzn-s3-demo-bucket/as2/baseline";

/// A SECOND role ARN satisfying `Role`'s modelled pattern `^arn:.*role/\S+$` and
/// `len 20..2048`, so a row that replaces `AccessRole` replaces it with a different
/// value from [`ROLE`].
const OTHER_ROLE: &str = "arn:aws:iam::000000000000:role/transfer-as2-updated";

/// `base` with every member of `overrides` inserted over it.
fn with(base: &Value, overrides: Value) -> Value {
    let mut merged = base.clone();
    let object = merged.as_object_mut().expect("the base is an object");
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        object.insert(member.clone(), value.clone());
    }
    merged
}

/// The server and the four AS2 profiles every row writes its agreements against: the
/// two the baseline agreement combines, and two more an update can move it to.
struct Partnership {
    server_id: String,
    other_server_id: String,
    local_profile: String,
    partner_profile: String,
    other_local_profile: String,
    other_partner_profile: String,
}

impl Partnership {
    async fn create(emulator: &Emulator) -> Self {
        Self {
            server_id: emulator.create_server(json!({})).await,
            other_server_id: emulator.create_server(json!({})).await,
            local_profile: emulator
                .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
                .await,
            partner_profile: emulator
                .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
                .await,
            other_local_profile: emulator
                .create_profile(json!({"As2Id": "local-party-2", "ProfileType": "LOCAL"}))
                .await,
            other_partner_profile: emulator
                .create_profile(json!({"As2Id": "partner-party-2", "ProfileType": "PARTNER"}))
                .await,
        }
    }

    /// The `CreateAgreement` body of a baseline agreement carrying `directories`.
    fn create_request(&self, directories: &Value) -> Value {
        with(
            &json!({
                "ServerId": &self.server_id,
                "LocalProfileId": &self.local_profile,
                "PartnerProfileId": &self.partner_profile,
                "AccessRole": ROLE,
                "Description": BASELINE_DESCRIPTION,
            }),
            directories.clone(),
        )
    }

    /// What `DescribeAgreement` reports for a baseline agreement carrying
    /// `directories`, with the documented defaults applied: the expectation every row
    /// states its own changes as overrides on.
    fn baseline_expectation(&self, directories: &Value) -> Value {
        let base = json!({
            "ServerId": &self.server_id,
            "LocalProfileId": &self.local_profile,
            "PartnerProfileId": &self.partner_profile,
            "AccessRole": ROLE,
            "Description": BASELINE_DESCRIPTION,
            "Status": DEFAULT_STATUS,
            "PreserveFilename": DEFAULT_TOGGLE,
            "EnforceMessageSigning": DEFAULT_TOGGLE,
            // The tag set is a collection the agreement always holds, so an agreement
            // created without tags reads the EMPTY list back.
            "Tags": [],
            // The agreement carries exactly one of the two directory configurations, so
            // the other member must be ABSENT; `directories` below sets the one it has.
            "BaseDirectory": Value::Null,
            "CustomDirectories": Value::Null,
        });
        with(&base, directories.clone())
    }
}

/// The two directory configurations, as the request member that names one and the
/// expectation a read answers for it.
fn base_directory(directory: &str) -> (Value, Value) {
    (
        json!({"BaseDirectory": directory}),
        json!({"BaseDirectory": directory, "CustomDirectories": Value::Null}),
    )
}

fn custom_directory(prefix: &str) -> (Value, Value) {
    (
        json!({"CustomDirectories": custom_directories(prefix)}),
        json!({"BaseDirectory": Value::Null, "CustomDirectories": custom_directories(prefix)}),
    )
}

/// One row: the baseline agreement it starts from, the update it sends, and what the
/// read that follows must report over the baseline's own expectation.
struct Case {
    /// What this row varies, named in every assertion it makes.
    case: String,
    /// The directory member the baseline agreement is CREATED with.
    create_directories: Value,
    /// The members the `UpdateAgreement` request names, beyond the two identifiers.
    update: Value,
    /// The members the following read must report differently from the baseline. A
    /// `null` expectation means the member must be absent.
    expected: Value,
}

/// Create a baseline agreement carrying `directories` and return its `AgreementId`.
async fn create_agreement(
    emulator: &Emulator,
    partnership: &Partnership,
    case: &str,
    directories: &Value,
) -> String {
    emulator
        .create_agreement(case, partnership.create_request(directories))
        .await
}

/// `UpdateAgreement` on one pair, carrying `members`.
async fn update(
    emulator: &Emulator,
    server_id: &str,
    agreement_id: &str,
    members: &Value,
) -> Answer {
    let request = with(
        &json!({"ServerId": server_id, "AgreementId": agreement_id}),
        members.clone(),
    );
    emulator.call("UpdateAgreement", request).await
}

/// `DescribeAgreement` on one pair.
async fn describe(emulator: &Emulator, server_id: &str, agreement_id: &str) -> Answer {
    emulator
        .call(
            "DescribeAgreement",
            json!({"ServerId": server_id, "AgreementId": agreement_id}),
        )
        .await
}

/// The `Agreement` member of a successful `DescribeAgreement`.
fn described(case: &str, answer: &Answer) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeAgreement 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.get("Agreement").clone()
}

/// The table: every row names a different member (or a different number of members), a
/// different value out of an enum's modelled set, or a different pair of directory
/// configurations to move between.
fn cases() -> Vec<Case> {
    let (base_member, base_expected) = base_directory(BASELINE_BASE_DIRECTORY);
    let (custom_member, custom_expected) = custom_directory(BASELINE_CUSTOM_PREFIX);
    let mut rows = vec![
        Case {
            case: "an update naming no member beyond the two identifiers".to_string(),
            create_directories: base_member.clone(),
            update: json!({}),
            expected: base_expected.clone(),
        },
        Case {
            case: format!("Status {} on an agreement that is ACTIVE", AGREEMENT_STATUSES[1]),
            create_directories: base_member.clone(),
            update: json!({"Status": AGREEMENT_STATUSES[1]}),
            expected: with(&base_expected, json!({"Status": AGREEMENT_STATUSES[1]})),
        },
        Case {
            case: format!("Status {} named again on an ACTIVE agreement", AGREEMENT_STATUSES[0]),
            create_directories: base_member.clone(),
            update: json!({"Status": AGREEMENT_STATUSES[0]}),
            expected: with(&base_expected, json!({"Status": AGREEMENT_STATUSES[0]})),
        },
        Case {
            case: "Description replaced".to_string(),
            create_directories: base_member.clone(),
            update: json!({"Description": "updated-agreement"}),
            expected: with(&base_expected, json!({"Description": "updated-agreement"})),
        },
        Case {
            case: "a Description at the modelled length bound (len 200)".to_string(),
            create_directories: base_member.clone(),
            update: json!({"Description": "U".repeat(200)}),
            expected: with(&base_expected, json!({"Description": "U".repeat(200)})),
        },
        Case {
            case: "a Description of the punctuation the modelled pattern allows".to_string(),
            create_directories: base_member.clone(),
            update: json!({"Description": "AS2-From_<name>.v3+1/#7"}),
            expected: with(&base_expected, json!({"Description": "AS2-From_<name>.v3+1/#7"})),
        },
        Case {
            case: "AccessRole replaced".to_string(),
            create_directories: base_member.clone(),
            update: json!({"AccessRole": OTHER_ROLE}),
            expected: with(&base_expected, json!({"AccessRole": OTHER_ROLE})),
        },
        Case {
            case: "a new BaseDirectory replacing the stored one".to_string(),
            create_directories: base_member.clone(),
            update: json!({"BaseDirectory": "/amzn-s3-demo-bucket/home/moved"}),
            expected: base_directory("/amzn-s3-demo-bucket/home/moved").1,
        },
        Case {
            case: "the shortest non-empty BaseDirectory the pattern allows".to_string(),
            create_directories: base_member.clone(),
            update: json!({"BaseDirectory": "/"}),
            expected: base_directory("/").1,
        },
        Case {
            case: "a BaseDirectory at the modelled length bound (len 1024)".to_string(),
            create_directories: base_member.clone(),
            update: json!({"BaseDirectory": format!("/{}", "d".repeat(1023))}),
            expected: base_directory(&format!("/{}", "d".repeat(1023))).1,
        },
        Case {
            case: "a blank BaseDirectory, which names no directory, so it replaces nothing"
                .to_string(),
            create_directories: base_member.clone(),
            update: json!({"BaseDirectory": ""}),
            expected: base_expected.clone(),
        },
        Case {
            case: "CustomDirectories replacing a BaseDirectory".to_string(),
            create_directories: base_member.clone(),
            update: custom_member.clone(),
            expected: custom_expected.clone(),
        },
        Case {
            case: "a blank BaseDirectory beside CustomDirectories names only the custom \
                   directories"
                .to_string(),
            create_directories: base_member.clone(),
            update: with(&custom_member, json!({"BaseDirectory": ""})),
            expected: custom_expected.clone(),
        },
        Case {
            case: "a BaseDirectory replacing CustomDirectories".to_string(),
            create_directories: custom_member.clone(),
            update: json!({"BaseDirectory": "/amzn-s3-demo-bucket/home/from-custom"}),
            expected: base_directory("/amzn-s3-demo-bucket/home/from-custom").1,
        },
        Case {
            case: "CustomDirectories replacing CustomDirectories".to_string(),
            create_directories: custom_member.clone(),
            update: custom_directory("/amzn-s3-demo-bucket/as2/moved").0,
            expected: custom_directory("/amzn-s3-demo-bucket/as2/moved").1,
        },
    ];
    // Each message toggle, over its whole modelled value set, named on its own: the
    // value that differs from the default and the value that IS the default.
    for toggle in TOGGLE_VALUES {
        rows.push(Case {
            case: format!("PreserveFilename {toggle}"),
            create_directories: base_member.clone(),
            update: json!({"PreserveFilename": toggle}),
            expected: with(&base_expected, json!({"PreserveFilename": toggle})),
        });
        rows.push(Case {
            case: format!("EnforceMessageSigning {toggle}"),
            create_directories: base_member.clone(),
            update: json!({"EnforceMessageSigning": toggle}),
            expected: with(&base_expected, json!({"EnforceMessageSigning": toggle})),
        });
    }
    rows
}

/// The rows that move an agreement onto ANOTHER profile. They are built beside the
/// table because the profile ids are minted per test run.
fn profile_cases(partnership: &Partnership) -> Vec<Case> {
    let (base_member, base_expected) = base_directory(BASELINE_BASE_DIRECTORY);
    vec![
        Case {
            case: "LocalProfileId moved to another local profile".to_string(),
            create_directories: base_member.clone(),
            update: json!({"LocalProfileId": &partnership.other_local_profile}),
            expected: with(
                &base_expected,
                json!({"LocalProfileId": &partnership.other_local_profile}),
            ),
        },
        Case {
            case: "PartnerProfileId moved to another partner profile".to_string(),
            create_directories: base_member.clone(),
            update: json!({"PartnerProfileId": &partnership.other_partner_profile}),
            expected: with(
                &base_expected,
                json!({"PartnerProfileId": &partnership.other_partner_profile}),
            ),
        },
        Case {
            case: "every updatable member at once".to_string(),
            create_directories: base_member,
            update: json!({
                "Status": AGREEMENT_STATUSES[1],
                "Description": "everything-at-once",
                "AccessRole": OTHER_ROLE,
                "PreserveFilename": TOGGLE_VALUES[0],
                "EnforceMessageSigning": TOGGLE_VALUES[0],
                "LocalProfileId": &partnership.other_local_profile,
                "PartnerProfileId": &partnership.other_partner_profile,
                "CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/all"),
            }),
            expected: with(
                &custom_directory("/amzn-s3-demo-bucket/as2/all").1,
                json!({
                    "Status": AGREEMENT_STATUSES[1],
                    "Description": "everything-at-once",
                    "AccessRole": OTHER_ROLE,
                    "PreserveFilename": TOGGLE_VALUES[0],
                    "EnforceMessageSigning": TOGGLE_VALUES[0],
                    "LocalProfileId": &partnership.other_local_profile,
                    "PartnerProfileId": &partnership.other_partner_profile,
                }),
            ),
        },
    ]
}

/// One refused update: the request that must be refused, and the refusal the service
/// answers it with. A row that asserts no resource members leaves `resource_type` empty.
struct Refusal {
    /// What this row varies, named in every assertion it makes.
    case: String,
    server_id: String,
    agreement_id: String,
    /// The members the request names beyond the two identifiers.
    members: Value,
    /// The awsJson error code.
    code: &'static str,
    message: String,
    resource_type: &'static str,
    resource: String,
}

/// RULE: an update persists every member it names and leaves every member it does not
/// name as the agreement already had it, so the following read answers the updated
/// record — under the agreement's own unchanged identifier and ARN.
#[tokio::test]
async fn an_update_persists_the_members_it_names_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;

    let mut rows = cases();
    rows.extend(profile_cases(&partnership));

    for row in &rows {
        let case = row.case.as_str();
        let agreement_id =
            create_agreement(&emulator, &partnership, case, &row.create_directories).await;
        // The ARN the create minted, which no update rewrites: an update changes the
        // agreement's configuration, never its identity.
        let created = described(case, &describe(&emulator, &partnership.server_id, &agreement_id).await);
        let arn = member_of(&created, "Arn").clone();

        let answer = update(&emulator, &partnership.server_id, &agreement_id, &row.update).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateAgreement 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("AgreementId"),
            &json!(&agreement_id),
            "case {case:?}: expected the updated AgreementId back, got {}",
            answer.body
        );

        let agreement = described(case, &describe(&emulator, &partnership.server_id, &agreement_id).await);
        assert_members(
            case,
            &agreement,
            &with(
                &partnership.baseline_expectation(&row.expected),
                json!({"AgreementId": &agreement_id, "Arn": arn}),
            ),
        );
    }
}

/// RULE (the status, in both directions): "you can update the status for the agreement,
/// either activating an inactive agreement or the reverse" — so each write moves the
/// stored status and the read that follows reports the value last written, however many
/// times it is written.
///
/// The steps differ along the direction of the change and along whether the value is
/// already the stored one.
#[tokio::test]
async fn the_status_moves_in_both_directions_and_the_read_follows_each_write() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;
    let (base_member, base_expected) = base_directory(BASELINE_BASE_DIRECTORY);
    let agreement_id = create_agreement(
        &emulator,
        &partnership,
        "status round trip",
        &base_member,
    )
    .await;

    // The created agreement is ACTIVE, then: deactivated, deactivated again (the same
    // value), reactivated, and reactivated again.
    let walk = [
        ("deactivated", AGREEMENT_STATUSES[1]),
        ("deactivated a second time", AGREEMENT_STATUSES[1]),
        ("reactivated", AGREEMENT_STATUSES[0]),
        ("reactivated a second time", AGREEMENT_STATUSES[0]),
    ];
    for (case, status) in walk {
        let answer = update(
            &emulator,
            &partnership.server_id,
            &agreement_id,
            &json!({"Status": status}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateAgreement 200, got {} with body {}",
            answer.status, answer.body
        );
        let agreement =
            described(case, &describe(&emulator, &partnership.server_id, &agreement_id).await);
        // The status is the value last written, and nothing else about the agreement
        // moved with it.
        assert_members(
            case,
            &agreement,
            &with(
                &partnership.baseline_expectation(&base_expected),
                json!({"Status": status}),
            ),
        );
    }
}

/// RULE (refusals): an update the service refuses persists NOTHING — the agreement the
/// following read answers is the one the create wrote.
///
/// The rows differ along what makes the request unacceptable: which half of the
/// `(ServerId, AgreementId)` pair misses, where in the id space the unresolvable
/// reference sits, which profile member carries it, and the documented either/or over
/// the two directory configurations.
#[tokio::test]
async fn a_refused_update_persists_nothing() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;
    let (base_member, base_expected) = base_directory(BASELINE_BASE_DIRECTORY);
    let agreement_id =
        create_agreement(&emulator, &partnership, "refusals", &base_member).await;

    // Syntactically valid identifiers (`^a-([0-9a-f]{17})$`, `^p-([0-9a-f]{17})$`) that
    // no create minted: the low end of the id space, a mid value and the high end.
    const NEVER_MINTED_AGREEMENTS: [&str; 3] = [
        "a-00000000000000000",
        "a-0000000000000dead",
        "a-fffffffffffffffff",
    ];
    const NEVER_MINTED_PROFILES: [&str; 2] = ["p-00000000000000000", "p-fffffffffffffffff"];

    // A change every refused row carries, so a row that was silently applied would show
    // up in the read below rather than passing unnoticed.
    let visible_change = json!({"Status": AGREEMENT_STATUSES[1], "Description": "must-not-land"});

    let mut rows: Vec<Refusal> = Vec::new();
    for absent in NEVER_MINTED_AGREEMENTS {
        assert!(
            is_modelled_agreement_id(absent),
            "the probe id {absent:?} must satisfy AgreementId's modelled pattern"
        );
        rows.push(Refusal {
            case: format!("an AgreementId {absent} nothing minted"),
            server_id: partnership.server_id.clone(),
            agreement_id: absent.to_string(),
            members: visible_change.clone(),
            code: "ResourceNotFoundException",
            message: "Unknown agreement".to_string(),
            resource_type: "Agreement",
            resource: absent.to_string(),
        });
    }
    // The agreement EXISTS, but on another server: the key is the pair, so the lookup
    // misses and the agreement is not updated under a server that does not hold it.
    rows.push(Refusal {
        case: "an existing agreement addressed under another ServerId".to_string(),
        server_id: partnership.other_server_id.clone(),
        agreement_id: agreement_id.clone(),
        members: visible_change.clone(),
        code: "ResourceNotFoundException",
        message: "Unknown agreement".to_string(),
        resource_type: "Agreement",
        resource: agreement_id.clone(),
    });
    for absent in NEVER_MINTED_PROFILES {
        for member in ["LocalProfileId", "PartnerProfileId"] {
            rows.push(Refusal {
                case: format!("an unknown {member} {absent}"),
                server_id: partnership.server_id.clone(),
                agreement_id: agreement_id.clone(),
                members: with(&visible_change, json!({member: absent})),
                code: "ResourceNotFoundException",
                message: "Unknown profile".to_string(),
                resource_type: "Profile",
                resource: absent.to_string(),
            });
        }
    }
    // The documented either/or: "Specify either BaseDirectory or CustomDirectories, but
    // not both. Specifying both causes the command to fail."
    rows.push(Refusal {
        case: "both directory configurations at once".to_string(),
        server_id: partnership.server_id.clone(),
        agreement_id: agreement_id.clone(),
        members: with(
            &visible_change,
            json!({
                "BaseDirectory": "/amzn-s3-demo-bucket/home/both",
                "CustomDirectories": custom_directories("/amzn-s3-demo-bucket/as2/both"),
            }),
        ),
        code: "InvalidRequestException",
        message: "BaseDirectory and CustomDirectories cannot both be specified".to_string(),
        // `InvalidRequestException` carries `Message` alone, so this row asserts no
        // resource members.
        resource_type: "",
        resource: String::new(),
    });

    for row in &rows {
        let case = row.case.as_str();
        let answer = update(&emulator, &row.server_id, &row.agreement_id, &row.members).await;
        // 400 is `InvalidRequestException`'s modelled `@httpError`, and the status this
        // service's recorded wire answers `ResourceNotFoundException` with.
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            row.code,
            "case {case:?}: expected {}, got body {}",
            row.code,
            answer.body
        );
        let reported_message = member_of(&answer.body, "Message").as_str().unwrap_or_default();
        assert_eq!(
            reported_message, row.message,
            "case {case:?}: expected Message {:?}, got {reported_message:?}",
            row.message
        );
        if !row.resource_type.is_empty() {
            let reported_type = member_of(&answer.body, "ResourceType")
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                reported_type, row.resource_type,
                "case {case:?}: expected ResourceType {:?}, got {reported_type:?}",
                row.resource_type
            );
            let reported_resource = member_of(&answer.body, "Resource")
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                reported_resource, row.resource,
                "case {case:?}: expected the unresolved id in Resource, \
                 got {reported_resource:?}"
            );
        }

        // Nothing the refused request named was applied: the agreement still reports
        // exactly what the create wrote.
        let agreement =
            described(case, &describe(&emulator, &partnership.server_id, &agreement_id).await);
        assert_members(
            case,
            &agreement,
            &partnership.baseline_expectation(&base_expected),
        );
    }
}
