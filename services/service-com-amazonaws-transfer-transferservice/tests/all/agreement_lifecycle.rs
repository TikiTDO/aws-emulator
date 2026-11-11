// SPDX-License-Identifier: CC0-1.0

//! The AS2 agreement lifecycle: `CreateAgreement` mints and persists, `DescribeAgreement`
//! reports what was persisted.
//!
//! RULE: `CreateAgreement` resolves the server and the two AS2 profiles the agreement
//! combines, mints a fresh system-assigned `AgreementId` of the modelled shape for EVERY
//! call, and stores the configuration the request carried with the documented defaults
//! applied once (`Status`, `PreserveFilename` and `EnforceMessageSigning`) under the
//! model's own ARN template `agreement/{ServerId}/{AgreementId}` — so `DescribeAgreement`
//! answers that record unchanged, and a `(ServerId, AgreementId)` pair the store does not
//! hold is answered with the modelled not-found error rather than an agreement.
//!
//! The rows differ along what the rule generalises over: whether each optional member is
//! sent at all, each enum member's whole modelled value set, the form and the length of
//! `BaseDirectory` (including both published bounds), `CustomDirectories` as the
//! alternative to it, and the COUNT of `Tags`.

use serde_json::{json, Value};

use crate::harness::{
    assert_members, custom_directories, is_modelled_agreement_id, member_of, Emulator, ACCOUNT,
    REGION, ROLE,
};

/// `AgreementStatusType`'s whole modelled value set
/// (`smithy_query(query="shape",shape="AgreementStatusType")` reports
/// `[ACTIVE, INACTIVE]`).
const AGREEMENT_STATUSES: [&str; 2] = ["ACTIVE", "INACTIVE"];

/// `PreserveFilenameType`'s and `EnforceMessageSigningType`'s whole modelled value set
/// (both report `[ENABLED, DISABLED]`).
const TOGGLE_VALUES: [&str; 2] = ["ENABLED", "DISABLED"];

/// `Status`'s value for an agreement created without one: an agreement the server can
/// use straight away.
const DEFAULT_STATUS: &str = "ACTIVE";

/// `PreserveFilename`'s and `EnforceMessageSigning`'s documented default ("`DISABLED`
/// (default value)" on each member).
const DEFAULT_TOGGLE: &str = "DISABLED";

/// Where an agreement's AS2 files land: the model documents the two members as
/// alternatives ("Specify either `BaseDirectory` or `CustomDirectories`, but not both").
#[derive(Clone)]
enum Directories {
    /// `BaseDirectory`: `HomeDirectory`, `len 0..1024`, pattern `^(|/.*)$`.
    Base(String),
    /// `CustomDirectories`: a `CustomDirectoriesType`, whose five members are all
    /// `@required` `HomeDirectory`s.
    Custom(String),
}

impl Directories {
    /// The request members this configuration sends.
    fn request_members(&self) -> Value {
        match self {
            Self::Base(directory) => json!({"BaseDirectory": directory}),
            Self::Custom(prefix) => json!({"CustomDirectories": custom_directories(prefix)}),
        }
    }

    /// What a reader must report: the configured member, and the other one ABSENT.
    fn expected_members(&self) -> Value {
        match self {
            Self::Base(directory) => json!({
                "BaseDirectory": directory,
                "CustomDirectories": Value::Null,
            }),
            Self::Custom(prefix) => json!({
                "BaseDirectory": Value::Null,
                "CustomDirectories": custom_directories(prefix),
            }),
        }
    }
}

/// One `CreateAgreement` request, as the configuration it is built from. `None` means
/// the member is not sent at all, so the row exercises the documented default.
struct Case {
    /// What this row varies, named in every assertion it makes.
    name: &'static str,
    /// `Description`: modelled `len 1..200`, pattern `^[\u{0021}-\u{007E}]+$`.
    description: Option<String>,
    status: Option<&'static str>,
    preserve_filename: Option<&'static str>,
    enforce_message_signing: Option<&'static str>,
    directories: Directories,
    tags: Vec<(&'static str, &'static str)>,
}

impl Case {
    /// The `CreateAgreement` body for `server_id` and the two profiles.
    fn request(&self, server_id: &str, local_profile: &str, partner_profile: &str) -> Value {
        let mut body = json!({
            "ServerId": server_id,
            "LocalProfileId": local_profile,
            "PartnerProfileId": partner_profile,
            "AccessRole": ROLE,
        });
        let object = body.as_object_mut().expect("the request body is an object");
        for (member, value) in self
            .directories
            .request_members()
            .as_object()
            .expect("the directory members are an object")
        {
            object.insert(member.clone(), value.clone());
        }
        if let Some(description) = &self.description {
            object.insert("Description".to_string(), json!(description));
        }
        if let Some(status) = self.status {
            object.insert("Status".to_string(), json!(status));
        }
        if let Some(preserve) = self.preserve_filename {
            object.insert("PreserveFilename".to_string(), json!(preserve));
        }
        if let Some(enforce) = self.enforce_message_signing {
            object.insert("EnforceMessageSigning".to_string(), json!(enforce));
        }
        if !self.tags.is_empty() {
            object.insert("Tags".to_string(), self.tags_json());
        }
        body
    }

    /// `Tags` on the wire: a list of `{Key, Value}` structures.
    fn tags_json(&self) -> Value {
        Value::Array(
            self.tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect(),
        )
    }

    /// What `DescribeAgreement` must report for the agreement this row created. A `null`
    /// expectation means the member must be absent.
    fn expected_members(&self, server_id: &str, local_profile: &str, partner_profile: &str) -> Value {
        let mut expected = json!({
            "ServerId": server_id,
            "LocalProfileId": local_profile,
            "PartnerProfileId": partner_profile,
            "AccessRole": ROLE,
            "Description": match &self.description {
                Some(description) => json!(description),
                None => Value::Null,
            },
            "Status": self.status.unwrap_or(DEFAULT_STATUS),
            "PreserveFilename": self.preserve_filename.unwrap_or(DEFAULT_TOGGLE),
            "EnforceMessageSigning": self.enforce_message_signing.unwrap_or(DEFAULT_TOGGLE),
            // The tag set is a collection the agreement always holds, so a row that
            // sent no tags reads back the EMPTY list rather than no member.
            "Tags": self.tags_json(),
        });
        let object = expected
            .as_object_mut()
            .expect("the expectation is an object");
        for (member, value) in self
            .directories
            .expected_members()
            .as_object()
            .expect("the directory expectation is an object")
        {
            object.insert(member.clone(), value.clone());
        }
        expected
    }
}

/// The table: every row differs from the others in which optional members it sends, in
/// an enum value, in the form or length of the landing directory, or in the number of
/// tags it carries.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "nothing optional beyond the landing directory",
            description: None,
            status: None,
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Base("/amzn-s3-demo-bucket/home/mydirectory".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "an explicitly ACTIVE agreement",
            description: Some("active-agreement".to_string()),
            status: Some(AGREEMENT_STATUSES[0]),
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Base("/bucket/active".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "an INACTIVE agreement",
            description: Some("inactive-agreement".to_string()),
            status: Some(AGREEMENT_STATUSES[1]),
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Base("/bucket/inactive".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "both message toggles ENABLED",
            description: Some("toggles-enabled".to_string()),
            status: None,
            preserve_filename: Some(TOGGLE_VALUES[0]),
            enforce_message_signing: Some(TOGGLE_VALUES[0]),
            directories: Directories::Base("/bucket/toggles-on".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "both message toggles explicitly DISABLED",
            description: None,
            status: Some(AGREEMENT_STATUSES[1]),
            preserve_filename: Some(TOGGLE_VALUES[1]),
            enforce_message_signing: Some(TOGGLE_VALUES[1]),
            directories: Directories::Base("/bucket/toggles-off".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "one toggle ENABLED and the other left to its default",
            description: None,
            status: None,
            preserve_filename: Some(TOGGLE_VALUES[0]),
            enforce_message_signing: None,
            directories: Directories::Base("/bucket/one-toggle".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "the shortest non-empty BaseDirectory the pattern allows",
            description: None,
            status: None,
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Base("/".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "a BaseDirectory at the modelled length bound (len 1024)",
            description: None,
            status: None,
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Base(format!("/{}", "d".repeat(1023))),
            tags: Vec::new(),
        },
        Case {
            name: "CustomDirectories instead of a BaseDirectory",
            description: Some("custom-directories".to_string()),
            status: None,
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Custom("/amzn-s3-demo-bucket/as2".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "CustomDirectories with every toggle and a status set",
            description: Some("custom-and-toggles".to_string()),
            status: Some(AGREEMENT_STATUSES[1]),
            preserve_filename: Some(TOGGLE_VALUES[0]),
            enforce_message_signing: Some(TOGGLE_VALUES[0]),
            directories: Directories::Custom("/bucket/custom".to_string()),
            tags: vec![("env", "prod")],
        },
        Case {
            name: "a Description at the modelled length bound (len 200)",
            description: Some("D".repeat(200)),
            status: None,
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Base("/bucket/long-description".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "a Description of punctuation the modelled pattern allows",
            description: Some("AS2-From_<name>.v2+1/#9".to_string()),
            status: None,
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Base("/bucket/punctuation".to_string()),
            tags: Vec::new(),
        },
        Case {
            name: "three tags, order preserved",
            description: None,
            status: None,
            preserve_filename: None,
            enforce_message_signing: None,
            directories: Directories::Base("/bucket/tags".to_string()),
            tags: vec![("env", "test"), ("owner", "as2-team"), ("tier", "1")],
        },
    ]
}

/// The row named `name`, so a test that needs ONE configuration selects it by what it
/// varies rather than by its position in [`cases`].
fn case_named(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("no case named {name:?} in the table"))
}

/// The row every test that needs just "an agreement" uses: nothing optional beyond the
/// landing directory.
const PLAIN_CASE: &str = "nothing optional beyond the landing directory";

/// The row that configures `CustomDirectories` instead of a `BaseDirectory`.
const CUSTOM_DIRECTORIES_CASE: &str = "CustomDirectories instead of a BaseDirectory";

/// One server and the two profiles every row's agreement is written against.
struct Partnership {
    server_id: String,
    local_profile: String,
    partner_profile: String,
}

impl Partnership {
    async fn create(emulator: &Emulator) -> Self {
        Self {
            server_id: emulator.create_server(json!({})).await,
            local_profile: emulator
                .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
                .await,
            partner_profile: emulator
                .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
                .await,
        }
    }
}

/// Create the agreement `case` describes and return its `AgreementId`.
async fn create_agreement(emulator: &Emulator, partnership: &Partnership, case: &Case) -> String {
    emulator
        .create_agreement(
            case.name,
            case.request(
                &partnership.server_id,
                &partnership.local_profile,
                &partnership.partner_profile,
            ),
        )
        .await
}

/// `DescribeAgreement` for one pair.
async fn describe(
    emulator: &Emulator,
    server_id: &str,
    agreement_id: &str,
) -> crate::harness::Answer {
    emulator.describe_agreement(server_id, agreement_id).await
}

/// RULE: each create mints a modelled `AgreementId` and persists its configuration with
/// the documented defaults applied, and `DescribeAgreement` reports that configuration
/// under the agreement's own ARN.
#[tokio::test]
async fn created_agreements_are_described_from_stored_state() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;
    let cases = cases();

    // Create every row first, so each describe below reads a store holding all of them:
    // an agreement must be reported from its OWN record, not from the last write.
    let mut created: Vec<(&Case, String)> = Vec::new();
    for case in &cases {
        let agreement_id = create_agreement(&emulator, &partnership, case).await;
        created.push((case, agreement_id));
    }

    for (case, agreement_id) in &created {
        let answer = describe(&emulator, &partnership.server_id, agreement_id).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected DescribeAgreement 200, got {} with body {}",
            case.name, answer.status, answer.body
        );

        let agreement = answer.get("Agreement");
        assert_members(
            case.name,
            agreement,
            &case.expected_members(
                &partnership.server_id,
                &partnership.local_profile,
                &partnership.partner_profile,
            ),
        );

        let reported_id = member_of(agreement, "AgreementId")
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            reported_id, agreement_id,
            "case {:?}: expected the described AgreementId to be {agreement_id:?}, got {reported_id:?}",
            case.name
        );

        let expected_arn = format!(
            "arn:aws:transfer:{REGION}:{ACCOUNT}:agreement/{}/{agreement_id}",
            partnership.server_id
        );
        let arn = member_of(agreement, "Arn").as_str().unwrap_or_default();
        assert_eq!(
            arn, expected_arn,
            "case {:?}: expected Arn {expected_arn:?}, got {arn:?}",
            case.name
        );

        // A read may not change what a later caller sees: the same describe answers the
        // same body.
        let again = describe(&emulator, &partnership.server_id, agreement_id).await;
        assert_eq!(
            again.body, answer.body,
            "case {:?}: expected a repeated DescribeAgreement to answer the same body, got {} then {}",
            case.name, answer.body, again.body
        );
    }

    // Every minted id is distinct: an agreement is addressed by its own identifier.
    let mut ids: Vec<&str> = created.iter().map(|(_, id)| id.as_str()).collect();
    ids.sort_unstable();
    let minted = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "expected {minted} distinct AgreementIds, got {ids:?}"
    );
}

/// RULE (minting): two creates carrying the SAME configuration are two independent
/// agreements, each describable on its own.
#[tokio::test]
async fn identical_create_requests_mint_independent_agreements() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;

    for case in cases() {
        let first = create_agreement(&emulator, &partnership, &case).await;
        let second = create_agreement(&emulator, &partnership, &case).await;
        assert_ne!(
            first, second,
            "case {:?}: expected two creates of the same configuration to mint two ids, \
             got {first:?} twice",
            case.name
        );

        for agreement_id in [&first, &second] {
            let answer = describe(&emulator, &partnership.server_id, agreement_id).await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected DescribeAgreement 200 for {agreement_id}, got {} with body {}",
                case.name, answer.status, answer.body
            );
            assert_members(
                case.name,
                answer.get("Agreement"),
                &case.expected_members(
                    &partnership.server_id,
                    &partnership.local_profile,
                    &partnership.partner_profile,
                ),
            );
        }
    }
}

/// RULE (either/or): the model documents `BaseDirectory` and `CustomDirectories` as
/// alternatives — "Specify either BaseDirectory or CustomDirectories, but not both" — so
/// a request naming both, or neither, is refused with the modelled
/// `InvalidRequestException` and nothing is persisted.
///
/// The rows differ in HOW the configuration misses that requirement, including the blank
/// `BaseDirectory` the member's pattern `^(|/.*)$` admits, which names no directory.
#[tokio::test]
async fn an_agreement_names_exactly_one_directory_configuration() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;

    // A `BaseDirectory` that is neither empty nor rooted is already refused at the
    // boundary by the member's pattern `^(|/.*)$`, so the only value that reaches this
    // handler and names no directory is the empty string.
    let refused: [(&str, Value); 3] = [
        (
            "both a BaseDirectory and CustomDirectories",
            json!({
                "BaseDirectory": "/bucket/base",
                "CustomDirectories": custom_directories("/bucket/custom"),
            }),
        ),
        ("neither of the two", json!({})),
        (
            "a blank BaseDirectory, which names no directory",
            json!({"BaseDirectory": ""}),
        ),
    ];

    for (case, directories) in &refused {
        let mut body = json!({
            "ServerId": &partnership.server_id,
            "LocalProfileId": &partnership.local_profile,
            "PartnerProfileId": &partnership.partner_profile,
            "AccessRole": ROLE,
        });
        let object = body.as_object_mut().expect("the request body is an object");
        for (member, value) in directories.as_object().expect("an object of members") {
            object.insert(member.clone(), value.clone());
        }

        let answer = emulator.call("CreateAgreement", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default();
        for member in ["BaseDirectory", "CustomDirectories"] {
            assert!(
                message.contains(member),
                "case {case:?}: expected the refusal to name {member}, got {message:?}"
            );
        }
    }

    // Blank BaseDirectory plus CustomDirectories names exactly ONE configuration, so it
    // is accepted and the blank member is reported absent.
    let mut body = json!({
        "ServerId": &partnership.server_id,
        "LocalProfileId": &partnership.local_profile,
        "PartnerProfileId": &partnership.partner_profile,
        "AccessRole": ROLE,
        "BaseDirectory": "",
    });
    body.as_object_mut()
        .expect("the request body is an object")
        .insert(
            "CustomDirectories".to_string(),
            custom_directories("/bucket/only-custom"),
        );
    let created = emulator.call("CreateAgreement", body).await;
    assert_eq!(
        created.status, 200,
        "case \"a blank BaseDirectory beside CustomDirectories\": expected 200, got {} with body {}",
        created.status, created.body
    );
    let agreement_id = created
        .get("AgreementId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let answer = describe(&emulator, &partnership.server_id, &agreement_id).await;
    assert_members(
        "a blank BaseDirectory beside CustomDirectories",
        answer.get("Agreement"),
        &json!({
            "BaseDirectory": Value::Null,
            "CustomDirectories": custom_directories("/bucket/only-custom"),
        }),
    );
}

/// RULE (references): the server and the two profiles an agreement combines must resolve,
/// so a well-formed id the store does not hold is refused with the modelled not-found and
/// no agreement is written.
///
/// The rows differ in WHICH reference is unresolvable and in where in the id space it
/// sits.
#[tokio::test]
async fn an_agreement_is_refused_when_a_reference_does_not_resolve() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;

    // Syntactically valid identifiers (`^s-([0-9a-f]{17})$`, `^p-([0-9a-f]{17})$`) that
    // no create minted: the low end of the id space and the high end.
    const ABSENT_SERVERS: [&str; 2] = ["s-00000000000000000", "s-fffffffffffffffff"];
    const ABSENT_PROFILES: [&str; 2] = ["p-00000000000000000", "p-fffffffffffffffff"];

    let mut rows: Vec<(String, Value, &str, String)> = Vec::new();
    for absent in ABSENT_SERVERS {
        rows.push((
            format!("an unknown ServerId {absent}"),
            json!({
                "ServerId": absent,
                "LocalProfileId": &partnership.local_profile,
                "PartnerProfileId": &partnership.partner_profile,
            }),
            "Server",
            absent.to_string(),
        ));
    }
    for absent in ABSENT_PROFILES {
        rows.push((
            format!("an unknown LocalProfileId {absent}"),
            json!({
                "ServerId": &partnership.server_id,
                "LocalProfileId": absent,
                "PartnerProfileId": &partnership.partner_profile,
            }),
            "Profile",
            absent.to_string(),
        ));
        rows.push((
            format!("an unknown PartnerProfileId {absent}"),
            json!({
                "ServerId": &partnership.server_id,
                "LocalProfileId": &partnership.local_profile,
                "PartnerProfileId": absent,
            }),
            "Profile",
            absent.to_string(),
        ));
    }

    for (case, references, resource_type, resource) in &rows {
        let mut body = json!({"AccessRole": ROLE, "BaseDirectory": "/bucket/unresolved"});
        let object = body.as_object_mut().expect("the request body is an object");
        for (member, value) in references.as_object().expect("an object of members") {
            object.insert(member.clone(), value.clone());
        }

        let answer = emulator.call("CreateAgreement", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        let reported_type = member_of(&answer.body, "ResourceType")
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            reported_type, *resource_type,
            "case {case:?}: expected ResourceType {resource_type:?}, got {reported_type:?}"
        );
        let reported_resource = member_of(&answer.body, "Resource")
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            reported_resource, *resource,
            "case {case:?}: expected the unresolved id in Resource, got {reported_resource:?}"
        );
    }

    // Nothing was persisted by any refused create: the server holds no agreement, so the
    // id the next create mints is the FIRST one and every probe above is still not found.
    let case = case_named(PLAIN_CASE);
    let agreement_id = create_agreement(&emulator, &partnership, &case).await;
    let answer = describe(&emulator, &partnership.server_id, &agreement_id).await;
    assert_eq!(
        answer.status, 200,
        "expected the first agreement after the refusals to describe, got {} with body {}",
        answer.status, answer.body
    );
}

/// RULE (not found): a well-formed `(ServerId, AgreementId)` pair the store does not hold
/// is answered with the service's generic not-found error, whose `Message` carries no
/// identifier.
///
/// The rows differ in WHERE in the id space the probe sits and in whether the pair misses
/// on the agreement or on the server half of the key.
#[tokio::test]
async fn describing_an_agreement_the_store_does_not_hold_answers_not_found() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;
    let other_server = emulator.create_server(json!({})).await;

    // Syntactically valid `AgreementId`s (`^a-([0-9a-f]{17})$`) that no create minted.
    const NEVER_MINTED: [&str; 3] = [
        "a-00000000000000000",
        "a-0000000000000dead",
        "a-fffffffffffffffff",
    ];

    let agreement_id = create_agreement(&emulator, &partnership, &case_named(PLAIN_CASE)).await;

    let mut probes: Vec<(String, String, String)> = Vec::new();
    for absent in NEVER_MINTED {
        assert!(
            is_modelled_agreement_id(absent),
            "the probe id {absent:?} must satisfy AgreementId's modelled pattern"
        );
        probes.push((
            format!("an AgreementId {absent} nothing minted"),
            partnership.server_id.clone(),
            absent.to_string(),
        ));
    }
    // The agreement EXISTS, but on another server: the key is the pair, so the lookup
    // misses.
    probes.push((
        "an existing agreement read under another ServerId".to_string(),
        other_server.clone(),
        agreement_id.clone(),
    ));

    let mut messages: Vec<(String, String)> = Vec::new();
    for (case, server_id, probe) in &probes {
        let answer = describe(&emulator, server_id, probe).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        let resource_type = member_of(&answer.body, "ResourceType")
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            resource_type, "Agreement",
            "case {case:?}: expected ResourceType \"Agreement\", got {resource_type:?}"
        );
        let resource = member_of(&answer.body, "Resource")
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            resource, probe,
            "case {case:?}: expected the requested id in Resource, got {resource:?}"
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.is_empty(),
            "case {case:?}: Message is modelled required, got body {}",
            answer.body
        );
        assert!(
            !message.contains(probe),
            "case {case:?}: expected no identifier in Message, got {message:?}"
        );
        messages.push((case.clone(), message));
    }

    // One fixed message for the resource type, whatever the probe.
    let (first_case, first) = messages.first().expect("every row produced a message");
    for (case, message) in &messages {
        assert_eq!(
            message, first,
            "case {case:?}: expected the same fixed Agreement message as case {first_case:?} \
             ({first:?}), got {message:?}"
        );
    }
}

/// RULE (ARN): the agreement's ARN carries the region the caller signed the create for,
/// and the describe reports that stored ARN whatever region it is read from.
#[tokio::test]
async fn an_agreements_arn_carries_the_region_it_was_created_in() {
    let emulator = Emulator::new();

    // Rows: regions in three partitions' naming styles, none of them the handler's
    // fallback default.
    for region in ["eu-west-2", "ap-southeast-1", "us-west-2"] {
        let partnership = Partnership::create(&emulator).await;
        let case = case_named(PLAIN_CASE);
        let answer = emulator
            .call_in(
                region,
                "CreateAgreement",
                case.request(
                    &partnership.server_id,
                    &partnership.local_profile,
                    &partnership.partner_profile,
                ),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "region {region}: expected CreateAgreement 200, got {} with body {}",
            answer.status, answer.body
        );
        let agreement_id = answer
            .get("AgreementId")
            .as_str()
            .unwrap_or_else(|| panic!("region {region}: no AgreementId in {}", answer.body))
            .to_string();

        let expected_arn = format!(
            "arn:aws:transfer:{region}:{ACCOUNT}:agreement/{}/{agreement_id}",
            partnership.server_id
        );
        // Read from a DIFFERENT region than the create: the ARN is the stored one.
        for reader in [region, REGION] {
            let described = emulator
                .call_in(
                    reader,
                    "DescribeAgreement",
                    json!({
                        "ServerId": &partnership.server_id,
                        "AgreementId": &agreement_id,
                    }),
                )
                .await;
            assert_eq!(
                described.status, 200,
                "region {region} read from {reader}: expected 200, got {} with body {}",
                described.status, described.body
            );
            let arn = member_of(described.get("Agreement"), "Arn")
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                arn, expected_arn,
                "region {region} read from {reader}: expected Arn {expected_arn:?}, got {arn:?}"
            );
        }
    }
}

/// RULE: deleting a server succeeds even while it still has ACTIVE agreements — there is
/// no referential-integrity refusal — and the agreements go WITH it: an agreement is
/// addressed by `(ServerId, AgreementId)`, so once that server is gone the pair resolves
/// to nothing and `DescribeAgreement` answers the modelled `ResourceNotFoundException`
/// rather than an ACTIVE agreement whose own `ServerId` names a server that does not
/// exist.
///
/// The rows differ in the agreement's directory configuration and in how many agreements
/// the deleted server held; a SECOND server's agreement, created before the delete, is
/// asserted to survive it, so the delete is shown to take the agreements of the server it
/// named and of no other.
#[tokio::test]
async fn an_agreement_goes_with_the_server_it_references() {
    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;

    // Another server of the same two profiles, whose agreement the delete must not touch.
    let other_server = emulator.create_server(json!({})).await;
    let other_partnership = Partnership {
        server_id: other_server.clone(),
        local_profile: partnership.local_profile.clone(),
        partner_profile: partnership.partner_profile.clone(),
    };
    let survivor_case = case_named(PLAIN_CASE);
    let survivor = create_agreement(&emulator, &other_partnership, &survivor_case).await;

    let rows = vec![case_named(PLAIN_CASE), case_named(CUSTOM_DIRECTORIES_CASE)];
    let mut created: Vec<(Case, String)> = Vec::new();
    for case in rows {
        let agreement_id = create_agreement(&emulator, &partnership, &case).await;
        created.push((case, agreement_id));
    }

    let deleted = emulator
        .call("DeleteServer", json!({"ServerId": &partnership.server_id}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteServer of a referenced server to succeed, got {} with body {}",
        deleted.status, deleted.body
    );

    for (case, agreement_id) in &created {
        let answer = describe(&emulator, &partnership.server_id, agreement_id).await;
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {:?}: expected the deleted server's agreement to be gone, got {} with body {}",
            case.name,
            answer.status,
            answer.body
        );
        assert!(
            answer.body.get("Agreement").is_none(),
            "case {:?}: a refused read must answer no agreement, got {}",
            case.name,
            answer.body
        );
    }

    // The other server's agreement was never addressed by the delete, so it still reports
    // the configuration it was created with.
    let answer = describe(&emulator, &other_server, &survivor).await;
    assert_eq!(
        answer.status, 200,
        "expected another server's agreement to survive, got {} with body {}",
        answer.status, answer.body
    );
    assert_members(
        "another server's agreement",
        answer.get("Agreement"),
        &survivor_case.expected_members(
            &other_server,
            &other_partnership.local_profile,
            &other_partnership.partner_profile,
        ),
    );
}

/// RULE (the bounds on the store): the agreement store is bounded BOTH per server and in
/// total — past either bound a create is refused with the modelled
/// `ServiceUnavailableException` instead of growing, and the refusal disturbs no stored
/// record.
///
/// The total bound is reached across servers rather than on one: each server has its own
/// per-server allowance, so the rows below drive the per-server bound on one server and
/// then the total bound from a FRESH server, which has its whole per-server allowance.
#[tokio::test]
async fn the_agreement_store_is_bounded_per_server_and_in_total() {
    use service_com_amazonaws_transfer_transferservice::state as st;

    let emulator = Emulator::new();
    let partnership = Partnership::create(&emulator).await;
    let case = case_named(PLAIN_CASE);
    let request = |server_id: &str| {
        case.request(
            server_id,
            &partnership.local_profile,
            &partnership.partner_profile,
        )
    };

    // Row 1: one server fills its own allowance, and the next create on it is refused.
    for n in 0..st::MAX_AGREEMENTS_PER_SERVER {
        let answer = emulator
            .call("CreateAgreement", request(&partnership.server_id))
            .await;
        assert_eq!(
            answer.status, 200,
            "case \"agreement {n} of {}\": expected 200 up to the bound, got {} with body {}",
            st::MAX_AGREEMENTS_PER_SERVER, answer.status, answer.body
        );
    }
    let answer = emulator
        .call("CreateAgreement", request(&partnership.server_id))
        .await;
    assert_eq!(
        answer.status, 503,
        "case \"one agreement past the per-server bound\": expected HTTP 503, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ServiceUnavailableException",
        "case \"one agreement past the per-server bound\": expected ServiceUnavailableException, \
         got {}",
        answer.body
    );

    // Row 2: the store is at its TOTAL bound, reached the way a caller reaches it — new
    // servers, each with a fresh per-server allowance, none of them deleted. The records
    // are seeded directly so the row asserts the bound rather than spending a wire call
    // per record.
    {
        let store = emulator.store();
        let mut guard = store.lock();
        let held = guard.agreements.len();
        for n in held..st::MAX_STORED_AGREEMENTS {
            let server_id = format!("s-{:017x}", n + 1);
            let agreement_id = format!("a-{:017x}", n + 1);
            guard.agreements.insert(
                (server_id.clone(), agreement_id.clone()),
                st::Agreement {
                    arn: st::agreement_arn(REGION, &server_id, &agreement_id),
                    agreement_id,
                    server_id,
                    status: "ACTIVE".to_string(),
                    local_profile_id: partnership.local_profile.clone(),
                    partner_profile_id: partnership.partner_profile.clone(),
                    base_directory: Some("/bucket/seeded".to_string()),
                    access_role: ROLE.to_string(),
                    preserve_filename: "DISABLED".to_string(),
                    enforce_message_signing: "DISABLED".to_string(),
                    ..st::Agreement::default()
                },
            );
        }
        assert_eq!(
            guard.agreements.len(),
            st::MAX_STORED_AGREEMENTS,
            "expected the store to hold exactly {} agreements before the total bound is \
             probed, got {}",
            st::MAX_STORED_AGREEMENTS,
            guard.agreements.len()
        );
        assert!(
            !guard.has_agreement_capacity(),
            "expected the store at {} agreements to report no capacity",
            st::MAX_STORED_AGREEMENTS
        );
    }

    let fresh_server = emulator.create_server(json!({})).await;
    let answer = emulator.call("CreateAgreement", request(&fresh_server)).await;
    assert_eq!(
        answer.status, 503,
        "case \"a fresh server at the total bound\": expected HTTP 503, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ServiceUnavailableException",
        "case \"a fresh server at the total bound\": expected ServiceUnavailableException, got {}",
        answer.body
    );
    assert_eq!(
        emulator.store().lock().agreements.len(),
        st::MAX_STORED_AGREEMENTS,
        "case \"a fresh server at the total bound\": expected the refusal to store nothing, \
         got {} agreements",
        emulator.store().lock().agreements.len()
    );

    // Releasing one record makes room for exactly one more create: the bound is a live
    // count, not a high-water mark.
    let released = emulator
        .store()
        .lock()
        .agreements
        .keys()
        .next_back()
        .cloned()
        .expect("the seeded store holds a key");
    emulator.store().lock().agreements.remove(&released);
    let answer = emulator.call("CreateAgreement", request(&fresh_server)).await;
    assert_eq!(
        answer.status, 200,
        "case \"one release below the total bound\": expected 200, got {} with body {}",
        answer.status, answer.body
    );
}
