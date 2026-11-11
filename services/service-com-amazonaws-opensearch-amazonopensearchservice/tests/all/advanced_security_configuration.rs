// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rules this service judges a domain's FINE-GRAINED ACCESS
//! CONTROL configuration by — the ones no modelled constraint can state, because they
//! span containers of the request and the record it would leave behind.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness and the per-operation request helpers come from the sibling suites.
//!
//! The rules, each asserted over a SET of inputs rather than one call:
//!
//! * fine-grained access control is served by the domain's encryption and its HTTPS-only
//!   endpoint, so a request that would leave it switched on with encryption at rest,
//!   node-to-node encryption or `EnforceHTTPS` missing is refused with that
//!   prerequisite's own sentence and no domain is created — asserted over a table that
//!   leaves each prerequisite out and, separately, asks for each one DISABLED, with the
//!   complete request accepted as the boundary of the same rule;
//! * the master user is who that boundary grants everything to, so
//!   `MasterUserOptions$MasterUserARN` has to name an IAM identity — asserted over a
//!   table of ARNs of other services, of ARN-shaped values that name no identity, and of
//!   every identity form that IS accepted;
//! * both switches are ONE-WAY: an update asking fine-grained access control or
//!   node-to-node encryption to be turned off on a domain that has it on is refused with
//!   that switch's own sentence, the setting stays enabled, and the refusal is answered
//!   whatever the domain is busy with — in particular straight after a configuration
//!   change the service accepted.

use serde_json::{json, Value};

use super::control_plane_refusal_wording::message;
use super::domain_lifecycle::{error_code, Emulator, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT};

/// The sentences the service states for the three prerequisites of fine-grained access
/// control, as the fine-grained-access-control guide `AdvancedSecurityOptionsInput`'s
/// documentation points at introduces them
/// (`smithy_query(query="shape",shape="AdvancedSecurityOptionsInput")`).
const ENCRYPTION_AT_REST_REQUIRED: &str =
    "You must enable encryption at rest to use advanced security options.";
/// See [`ENCRYPTION_AT_REST_REQUIRED`].
const NODE_TO_NODE_ENCRYPTION_REQUIRED: &str =
    "You must enable node-to-node encryption to use advanced security options.";
/// See [`ENCRYPTION_AT_REST_REQUIRED`].
const ENFORCE_HTTPS_REQUIRED: &str =
    "You must enable EnforceHTTPS in the domain endpoint options to use advanced security.";

/// The sentence a master user ARN that names no IAM identity is refused with.
const MASTER_USER_ARN_NOT_A_PRINCIPAL: &str =
    "The master user ARN is not one of the allowed types: user, root user, or role.";

/// The sentences the two one-way switches are refused with.
const ADVANCED_SECURITY_IS_ONE_WAY: &str =
    "You can't disable advanced security options once you enable it.";
/// See [`ADVANCED_SECURITY_IS_ONE_WAY`].
const NODE_TO_NODE_ENCRYPTION_IS_ONE_WAY: &str =
    "Once enabled, node to node encryption cannot be disabled";

/// An account-owned IAM user ARN — the identity this emulator's own credential speaks
/// for, and so the value a caller naming "my own principal" as the master user sends.
fn iam_user_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The master user options every accepted create below carries: the caller's own IAM
/// user, with the internal user database left off so the ARN is the master.
fn master_user(arn: &str) -> Value {
    json!({
        "Enabled": true,
        "InternalUserDatabaseEnabled": false,
        "MasterUserOptions": { "MasterUserARN": arn },
    })
}

/// A `CreateDomain` body carrying every prerequisite of fine-grained access control, with
/// `overrides` merged over it — the base each row of the prerequisite table removes or
/// disables exactly one container of.
fn complete_create(name: &str, overrides: &[(&str, Option<Value>)]) -> Value {
    let mut body = json!({
        "DomainName": name,
        "EngineVersion": "OpenSearch_2.11",
        "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
        "EncryptionAtRestOptions": { "Enabled": true },
        "NodeToNodeEncryptionOptions": { "Enabled": true },
        "DomainEndpointOptions": { "EnforceHTTPS": true },
        "AdvancedSecurityOptions": master_user(&iam_user_arn()),
    });
    for (member, value) in overrides {
        match value {
            None => {
                body.as_object_mut()
                    .expect("the create body is an object")
                    .remove(*member);
            }
            Some(value) => body[*member] = value.clone(),
        }
    }
    body
}

/// Whether the domain `name` exists at all, read through `DescribeDomain` — the proof a
/// refused create left nothing behind.
async fn absent(emulator: &Emulator, name: &str, case: &str) {
    let read = emulator.describe(name).await;
    assert_eq!(
        (read.status, error_code(&read).as_str()),
        (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case {case}: expected the refused create to have left no domain, got {} with {:?}",
        read.status,
        read.body
    );
    let expected = format!("Domain not found: {name}");
    assert_eq!(
        message(&read),
        expected,
        "case {case}: expected {expected:?}, got {:?}",
        read.body
    );
}

/// `AdvancedSecurityOptions$Enabled` as `DescribeDomainConfig` reports it for `name`.
async fn reported_advanced_security(emulator: &Emulator, name: &str, case: &str) -> Option<bool> {
    let read = emulator.describe_config(name).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected DescribeDomainConfig to be answered, got {} with {:?}",
        read.status, read.body
    );
    read.body["DomainConfig"]["AdvancedSecurityOptions"]["Options"]["Enabled"].as_bool()
}

/// `NodeToNodeEncryptionOptions$Enabled` as `DescribeDomainConfig` reports it for `name`.
async fn reported_node_to_node(emulator: &Emulator, name: &str, case: &str) -> Option<bool> {
    let read = emulator.describe_config(name).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected DescribeDomainConfig to be answered, got {} with {:?}",
        read.status, read.body
    );
    read.body["DomainConfig"]["NodeToNodeEncryptionOptions"]["Options"]["Enabled"].as_bool()
}

/// One row of the prerequisite table: what tells it from the others, which containers of
/// the complete create it overrides (`None` removes the container), and the sentence the
/// service states for it.
type PrerequisiteRow<'a> = (&'a str, Vec<(&'a str, Option<Value>)>, &'a str);

#[tokio::test]
async fn advanced_security_without_one_of_its_prerequisites_is_refused_and_creates_nothing() {
    let emulator = Emulator::new();

    // One row per prerequisite, twice over: the container left OUT of the request, and
    // the container present with the setting switched OFF — the two ways a domain ends up
    // without it.
    let rows: Vec<PrerequisiteRow> = vec![
        (
            "EncryptionAtRestOptions omitted",
            vec![("EncryptionAtRestOptions", None)],
            ENCRYPTION_AT_REST_REQUIRED,
        ),
        (
            "EncryptionAtRestOptions disabled",
            vec![(
                "EncryptionAtRestOptions",
                Some(json!({ "Enabled": false })),
            )],
            ENCRYPTION_AT_REST_REQUIRED,
        ),
        (
            "NodeToNodeEncryptionOptions omitted",
            vec![("NodeToNodeEncryptionOptions", None)],
            NODE_TO_NODE_ENCRYPTION_REQUIRED,
        ),
        (
            "NodeToNodeEncryptionOptions disabled",
            vec![(
                "NodeToNodeEncryptionOptions",
                Some(json!({ "Enabled": false })),
            )],
            NODE_TO_NODE_ENCRYPTION_REQUIRED,
        ),
        (
            "DomainEndpointOptions omitted",
            vec![("DomainEndpointOptions", None)],
            ENFORCE_HTTPS_REQUIRED,
        ),
        (
            "EnforceHTTPS false",
            vec![(
                "DomainEndpointOptions",
                Some(json!({ "EnforceHTTPS": false })),
            )],
            ENFORCE_HTTPS_REQUIRED,
        ),
        (
            // More than one missing: the refusal is the FIRST prerequisite in the order
            // the service states them, so the sentence does not depend on how the
            // request was built.
            "every prerequisite omitted",
            vec![
                ("EncryptionAtRestOptions", None),
                ("NodeToNodeEncryptionOptions", None),
                ("DomainEndpointOptions", None),
            ],
            ENCRYPTION_AT_REST_REQUIRED,
        ),
    ];

    for (index, (case, overrides, expected)) in rows.iter().enumerate() {
        let name = format!("fgac-prereq-{index}");
        let answer = emulator.create(complete_create(&name, overrides)).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the modelled validation refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            message(&answer),
            *expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.body
        );
        absent(&emulator, &name, case).await;
    }

    // The boundary of the same rule, twice over: the request that carries every
    // prerequisite is accepted, and so is one that carries NONE of them while leaving
    // fine-grained access control switched off — the prerequisites are what that boundary
    // needs, so a domain without it needs none of them.
    let complete = "fgac-prereq-complete";
    let accepted = emulator.create(complete_create(complete, &[])).await;
    assert_eq!(
        accepted.status, 200,
        "case every prerequisite present: expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        reported_advanced_security(&emulator, complete, "every prerequisite present").await,
        Some(true),
        "case every prerequisite present: the domain runs behind the boundary it asked for"
    );

    let plain = "fgac-prereq-disabled";
    let disabled = emulator
        .create(complete_create(
            plain,
            &[
                ("EncryptionAtRestOptions", None),
                ("NodeToNodeEncryptionOptions", None),
                ("DomainEndpointOptions", None),
                (
                    "AdvancedSecurityOptions",
                    Some(json!({ "Enabled": false })),
                ),
            ],
        ))
        .await;
    assert_eq!(
        disabled.status, 200,
        "case advanced security disabled: expected 200, got {} with {:?}",
        disabled.status, disabled.body
    );
    assert_eq!(
        reported_advanced_security(&emulator, plain, "advanced security disabled").await,
        Some(false),
        "case advanced security disabled: the domain carries the switch it asked for"
    );
}

#[tokio::test]
async fn a_master_user_arn_that_names_no_iam_identity_is_refused() {
    let emulator = Emulator::new();
    let account = ::engine::emulator_account_id();

    // One row per way a well-formed ARN names something that is not an identity — an S3
    // bucket (the recorded case), a KMS key, an IAM POLICY (the right service, the wrong
    // resource type), an identity-shaped ARN carrying a wildcard rather than naming one
    // identity, and a value that is no ARN at all but long enough to pass `ARN`'s
    // `@length(min: 20)`.
    let refused = [
        "arn:aws:s3:::fgac-master-bucket".to_string(),
        format!("arn:aws:kms:us-east-1:{account}:key/00000000-0000-0000-0000-000000000000"),
        format!("arn:aws:iam::{account}:policy/fgac-master-policy"),
        format!("arn:aws:iam::{account}:user/*"),
        "this-is-not-an-arn-at-all".to_string(),
    ];
    for (index, arn) in refused.iter().enumerate() {
        let name = format!("fgac-master-bad-{index}");
        let answer = emulator
            .create(complete_create(
                &name,
                &[("AdvancedSecurityOptions", Some(master_user(arn)))],
            ))
            .await;
        let case = format!("MasterUserARN {arn:?}");
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the modelled validation refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            message(&answer),
            MASTER_USER_ARN_NOT_A_PRINCIPAL,
            "case {case}: expected {MASTER_USER_ARN_NOT_A_PRINCIPAL:?}, got {:?}",
            answer.body
        );
        absent(&emulator, &name, &case).await;
    }

    // The boundary of the same rule: every identity form IS accepted — an IAM user (with
    // and without a path), an IAM role, the account's root user and a role SESSION, which
    // speaks for the role it is a session of.
    let accepted = [
        iam_user_arn(),
        format!("arn:aws:iam::{account}:user/engineering/analyst"),
        format!("arn:aws:iam::{account}:role/opensearch-master"),
        format!("arn:aws:iam::{account}:root"),
        format!("arn:aws:sts::{account}:assumed-role/opensearch-master/session-1"),
    ];
    for (index, arn) in accepted.iter().enumerate() {
        let name = format!("fgac-master-ok-{index}");
        let answer = emulator
            .create(complete_create(
                &name,
                &[("AdvancedSecurityOptions", Some(master_user(arn)))],
            ))
            .await;
        let case = format!("MasterUserARN {arn:?}");
        assert_eq!(
            answer.status, 200,
            "case {case}: expected the identity to be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        // Read back through the config reader: the domain the create answered with is the
        // domain a later read reports.
        assert_eq!(
            reported_advanced_security(&emulator, &name, &case).await,
            Some(true),
            "case {case}: expected the stored domain to carry the boundary it was created with"
        );
    }
}

#[tokio::test]
async fn the_one_way_security_switches_cannot_be_turned_off_by_an_update() {
    let emulator = Emulator::new();
    let name = "fgac-one-way";
    let created = emulator.create(complete_create(name, &[])).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    // A configuration change the service ACCEPTS, so every refusal below is answered on a
    // domain that is still applying one: the one-way rule is about the configuration the
    // request asks for, not about what the domain is busy with.
    let policies = emulator
        .update_config(
            name,
            json!({ "AccessPolicies": "{\"Version\":\"2012-10-17\",\"Statement\":[]}" }),
        )
        .await;
    assert_eq!(
        policies.status, 200,
        "case UpdateDomainConfig(AccessPolicies): expected 200, got {} with {:?}",
        policies.status, policies.body
    );

    // (the case, the request, the sentence that switch is refused with). Each is driven
    // immediately after the accepted change above, and again once the domain has settled,
    // so the answer is asserted to be the same either way.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "AdvancedSecurityOptions disabled",
            json!({ "AdvancedSecurityOptions": { "Enabled": false } }),
            ADVANCED_SECURITY_IS_ONE_WAY,
        ),
        (
            "NodeToNodeEncryptionOptions disabled",
            json!({ "NodeToNodeEncryptionOptions": { "Enabled": false } }),
            NODE_TO_NODE_ENCRYPTION_IS_ONE_WAY,
        ),
        (
            // The same switch asked for off beside a change the service would otherwise
            // apply: the request is refused as a whole.
            "AdvancedSecurityOptions disabled beside an accepted member",
            json!({
                "AdvancedSecurityOptions": { "Enabled": false },
                "SnapshotOptions": { "AutomatedSnapshotStartHour": 3 },
            }),
            ADVANCED_SECURITY_IS_ONE_WAY,
        ),
    ];
    for (case, body, expected) in &rows {
        for (when, settled) in [("while a change is landing", false), ("once settled", true)] {
            if settled {
                tokio::time::sleep(SETTLE_WAIT).await;
            }
            let case = format!("{case} ({when})");
            let answer = emulator.update_config(name, body.clone()).await;
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {case}: expected the modelled validation refusal, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                message(&answer),
                *expected,
                "case {case}: expected {expected:?}, got {:?}",
                answer.body
            );
        }
    }

    // Both switches are exactly where the create left them: a refused update stored
    // nothing.
    assert_eq!(
        reported_advanced_security(&emulator, name, "after the refusals").await,
        Some(true),
        "case after the refusals: fine-grained access control is still enabled"
    );
    assert_eq!(
        reported_node_to_node(&emulator, name, "after the refusals").await,
        Some(true),
        "case after the refusals: node-to-node encryption is still enabled"
    );
}

#[tokio::test]
async fn an_update_that_switches_advanced_security_on_is_judged_by_the_same_prerequisites() {
    let emulator = Emulator::new();

    // A domain created with NONE of the prerequisites, so each row below asks for the
    // boundary on a record that cannot carry it, and one created with all of them but the
    // boundary off, which the same update then switches on.
    let plain = "fgac-update-plain";
    let ready = "fgac-update-ready";
    for (name, overrides) in [
        (
            plain,
            vec![
                ("EncryptionAtRestOptions", None),
                ("NodeToNodeEncryptionOptions", None),
                ("DomainEndpointOptions", None),
                (
                    "AdvancedSecurityOptions",
                    Some(json!({ "Enabled": false })),
                ),
            ],
        ),
        (
            ready,
            vec![(
                "AdvancedSecurityOptions",
                Some(json!({ "Enabled": false })),
            )],
        ),
    ] {
        let created = emulator.create(complete_create(name, &overrides)).await;
        assert_eq!(
            created.status, 200,
            "case CreateDomain({name:?}): expected 200, got {} with {:?}",
            created.status, created.body
        );
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    // The update is judged on the MERGED record, so the rows differ in which
    // prerequisites the request brings with it: none, one, two — each refused with the
    // first one still missing.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "nothing but the switch",
            json!({ "AdvancedSecurityOptions": master_user(&iam_user_arn()) }),
            ENCRYPTION_AT_REST_REQUIRED,
        ),
        (
            "the switch with encryption at rest",
            json!({
                "AdvancedSecurityOptions": master_user(&iam_user_arn()),
                "EncryptionAtRestOptions": { "Enabled": true },
            }),
            NODE_TO_NODE_ENCRYPTION_REQUIRED,
        ),
        (
            "the switch with both encryptions",
            json!({
                "AdvancedSecurityOptions": master_user(&iam_user_arn()),
                "EncryptionAtRestOptions": { "Enabled": true },
                "NodeToNodeEncryptionOptions": { "Enabled": true },
            }),
            ENFORCE_HTTPS_REQUIRED,
        ),
    ];
    for (case, body, expected) in &rows {
        let answer = emulator.update_config(plain, body.clone()).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the modelled validation refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            message(&answer),
            *expected,
            "case {case}: expected {expected:?}, got {:?}",
            answer.body
        );
        assert_eq!(
            reported_advanced_security(&emulator, plain, case).await,
            Some(false),
            "case {case}: a refused update leaves the boundary off"
        );
    }

    // The boundary of the same rule: the domain that already has the three prerequisites
    // takes the switch.
    let answer = emulator
        .update_config(
            ready,
            json!({ "AdvancedSecurityOptions": master_user(&iam_user_arn()) }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case the prepared domain: expected the switch to be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        reported_advanced_security(&emulator, ready, "the prepared domain").await,
        Some(true),
        "case the prepared domain: the boundary the update asked for is stored"
    );
}
