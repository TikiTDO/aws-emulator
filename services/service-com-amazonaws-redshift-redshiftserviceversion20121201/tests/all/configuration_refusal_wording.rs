// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE the four configuration-resource families refuse an unresolvable
//! or an already-taken identifier with, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The RULE the suite asserts: for a cluster parameter group, an HSM client
//! certificate, a snapshot copy grant and an authentication profile, the
//! refusal's status and error code are what the model fixes for the whole
//! service, while the MESSAGE belongs to the OPERATION — each one answers the
//! fixed text Amazon Redshift words for that operation, with the identifier
//! taken from the REQUEST as its only variable part, and with no variable part
//! at all where the sentence carries none. So two operations asked about the
//! same missing identifier can answer differently, and one operation's sentence
//! does not change with the identifier's shape.
//!
//! The authentication profile family shows the same split one step further: a
//! name that identifies no profile is answered with one fixed sentence naming
//! no profile, while a write whose profile map the service cannot read — no
//! JSON document, no map, or a map with no setting in it — is answered with the
//! profile-map sentence instead.
//!
//! The tables span what the rule generalises over: the operation (a read of a
//! resource, a listing filtered by its identifier, a write whose target it is, a
//! create under a name already taken, and a cluster write that merely
//! dereferences it), and the identifier (a name nothing created, a prefix of a
//! stored one, a stored one with an extra character, a case variant where the
//! family matches exactly, the longest name the documented rule admits, and a
//! resource this account created and then deleted).

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{Answer, Emulator};

/// POST one action with owned parameters, so a row can build its values.
async fn call(emulator: &Emulator, action: &str, params: &[(String, String)]) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call(action, &borrowed).await
}

/// One form parameter, owned so a row can build it from a borrowed identifier.
fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// Assert the status and awsQuery error code of one refusal.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (status, Some(code.to_string())),
        "case {case}: expected {status} {code}, got {} / {:?} with body {}",
        answer.status,
        answer.error_code(),
        answer.body
    );
}

/// Assert the `Message` of one refusal, verbatim.
fn assert_message(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.text("Message").as_deref(),
        Some(expected),
        "case {case}: expected the message {expected:?}, got {:?} with body {}",
        answer.text("Message"),
        answer.body
    );
}

// ---------------------------------------------------------------- parameter groups

/// The form parameters one parameter-group operation sends about a single
/// `ParameterGroupName`: every member it requires, with the name in the member
/// that operation carries it in.
type Request = fn(&str) -> Vec<(String, String)>;

/// The sentence an operation words its `ClusterParameterGroupNotFound` refusal
/// with.
///
/// Spelled out in this suite rather than read off the crate under test, so a row
/// cannot pass by restating whatever the handler happened to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupWording {
    /// `ClusterParameterGroup not found: <ParameterGroupName>` — the operations
    /// that dereference a group they do not own.
    ShapeNameThenName,
    /// `ParameterGroup not found: <ParameterGroupName>` — the operations whose
    /// target is the group itself.
    ShortNameThenName,
    /// `ClusterParameterGroup <ParameterGroupName> not found.` — the group
    /// listing filtered by name.
    NameThenNotFound,
    /// `Parameter group does not exist` — the parameter-group modify, whose
    /// sentence names no group at all.
    DoesNotExist,
}

fn group_sentence(wording: GroupWording, name: &str) -> String {
    match wording {
        GroupWording::ShapeNameThenName => format!("ClusterParameterGroup not found: {name}"),
        GroupWording::ShortNameThenName => format!("ParameterGroup not found: {name}"),
        GroupWording::NameThenNotFound => format!("ClusterParameterGroup {name} not found."),
        GroupWording::DoesNotExist => "Parameter group does not exist".to_string(),
    }
}

fn group_name_only(name: &str) -> Vec<(String, String)> {
    vec![pair("ParameterGroupName", name)]
}

fn modify_group(name: &str) -> Vec<(String, String)> {
    vec![
        pair("ParameterGroupName", name),
        pair("Parameters.Parameter.1.ParameterName", "require_ssl"),
        pair("Parameters.Parameter.1.ParameterValue", "true"),
    ]
}

fn reset_group(name: &str) -> Vec<(String, String)> {
    vec![
        pair("ParameterGroupName", name),
        pair("ResetAllParameters", "true"),
    ]
}

/// A cluster create configured from a parameter group, which dereferences the
/// group without owning it. The cluster identifier is derived from the name so
/// no two rows collide on it.
fn create_cluster_on_group(name: &str) -> Vec<(String, String)> {
    let suffix: String = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(20)
        .collect();
    vec![
        pair("ClusterIdentifier", &format!("wording-{suffix}")),
        pair("NodeType", "ra3.xlplus"),
        pair("MasterUsername", "admin"),
        pair("ClusterParameterGroupName", name),
    ]
}

/// Every parameter-group operation this suite holds to the rule, with the
/// sentence each one answers.
fn group_operations() -> Vec<(&'static str, Request, GroupWording)> {
    vec![
        (
            "DescribeClusterParameters",
            group_name_only as Request,
            GroupWording::ShapeNameThenName,
        ),
        (
            "CreateCluster",
            create_cluster_on_group as Request,
            GroupWording::ShapeNameThenName,
        ),
        (
            "DeleteClusterParameterGroup",
            group_name_only as Request,
            GroupWording::ShortNameThenName,
        ),
        (
            "ModifyClusterParameterGroup",
            modify_group as Request,
            GroupWording::DoesNotExist,
        ),
        (
            "ResetClusterParameterGroup",
            reset_group as Request,
            GroupWording::ShortNameThenName,
        ),
        (
            "DescribeClusterParameterGroups",
            group_name_only as Request,
            GroupWording::NameThenNotFound,
        ),
    ]
}

/// The family every group this suite creates belongs to, read off the store's
/// own catalog rather than spelled here.
fn group_family() -> String {
    state::parameter_group_family(state::DEFAULT_CLUSTER_VERSION)
}

/// `CreateClusterParameterGroup` for one name.
async fn create_group(emulator: &Emulator, name: &str) -> Answer {
    call(
        emulator,
        "CreateClusterParameterGroup",
        &[
            pair("ParameterGroupName", name),
            ("ParameterGroupFamily".to_string(), group_family()),
            pair("Description", "a refusal-wording probe"),
        ],
    )
    .await
}

#[tokio::test]
async fn every_parameter_group_operation_words_its_not_found_refusal_its_own_way() {
    let emulator = Emulator::new();

    // One group that DOES exist, so a refusal below cannot be an "everything is
    // refused" artefact, and one created-then-deleted name: a group this
    // account held resolves to nothing once it is gone, exactly like one that
    // never existed.
    let present = create_group(&emulator, "pg-present").await;
    assert_eq!(
        present.status, 200,
        "case seed the present group: expected 200, got {} with body {}",
        present.status, present.body
    );
    let doomed = create_group(&emulator, "pg-doomed").await;
    assert_eq!(
        doomed.status, 200,
        "case seed the doomed group: expected 200, got {} with body {}",
        doomed.status, doomed.body
    );
    let removed = call(
        &emulator,
        "DeleteClusterParameterGroup",
        &group_name_only("pg-doomed"),
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "case delete the doomed group: expected 200, got {} with body {}",
        removed.status, removed.body
    );

    // `max_len` is the documented bound on `ParameterGroupName`; see
    // `state::PARAMETER_GROUP_NAME_RULE`. The names are lower case because that
    // is the form a group is stored and matched in.
    let longest: String = std::iter::once('p')
        .chain(std::iter::repeat_n(
            'x',
            state::PARAMETER_GROUP_NAME_RULE.max_len - 1,
        ))
        .collect();
    let names: Vec<(&str, String)> = vec![
        ("a name nothing created", "no-such-group".to_string()),
        (
            "a prefix of a stored name",
            "pg-prese".to_string(),
        ),
        (
            "a stored name with one extra character",
            "pg-presentx".to_string(),
        ),
        ("the longest name the documented rule admits", longest),
        (
            "a group this account created and then deleted",
            "pg-doomed".to_string(),
        ),
    ];

    for (name_case, name) in &names {
        for (action, request, wording) in group_operations() {
            let case = format!("{action} on {name_case}");
            let refused = call(&emulator, action, &request(name)).await;
            assert_refused(&case, &refused, 404, "ClusterParameterGroupNotFound");
            assert_message(&case, &refused, &group_sentence(wording, name));
        }
    }

    // The sentences are DISTINGUISHABLE: the same missing name, asked about
    // through operations of the four wording classes, comes back worded four
    // different ways rather than collapsed onto one template.
    let shared = "pg-collapse-probe";
    let mut answered: Vec<String> = Vec::new();
    for (action, request, _) in group_operations() {
        let refused = call(&emulator, action, &request(shared)).await;
        answered.push(refused.text("Message").unwrap_or_else(|| {
            panic!(
                "case {action} on {shared}: expected a refusal message, got body {}",
                refused.body
            )
        }));
    }
    answered.sort();
    answered.dedup();
    assert_eq!(
        answered.len(),
        4,
        "case the four wording classes: expected four different sentences for one name, got {answered:?}"
    );

    // The group that exists is still there: not one refusal removed it.
    let still_there = call(
        &emulator,
        "DescribeClusterParameterGroups",
        &group_name_only("pg-present"),
    )
    .await;
    assert_eq!(
        still_there.status, 200,
        "case the surviving group: expected 200, got {} with body {}",
        still_there.status, still_there.body
    );
}

#[tokio::test]
async fn a_parameter_group_name_already_taken_is_refused_naming_the_requested_name() {
    let emulator = Emulator::new();

    // Rows differing along the shape of the name and along HOW the second
    // create collides: the same spelling, and another case of it (a name is
    // stored and matched folded to lower case).
    for (case, stored, requested) in [
        ("the same spelling", "pg-dup", "pg-dup"),
        (
            "a long hyphenated name",
            "pg-dup-with-several-hyphens",
            "pg-dup-with-several-hyphens",
        ),
        (
            "an upper-case spelling of a stored name",
            "pg-folded",
            "PG-FOLDED",
        ),
        (
            "a mixed-case spelling of a stored name",
            "pg-mixed",
            "Pg-Mixed",
        ),
    ] {
        let created = create_group(&emulator, stored).await;
        assert_eq!(
            created.status, 200,
            "case {case}: the first create must be served, got {} with body {}",
            created.status, created.body
        );

        let duplicate = create_group(&emulator, requested).await;
        assert_refused(
            case,
            &duplicate,
            400,
            "ClusterParameterGroupAlreadyExists",
        );
        // The fixed text, with the name the REQUEST carried as its only
        // variable part — not the folded name the store holds.
        assert_message(
            case,
            &duplicate,
            &format!("Parameter group {requested} already exists"),
        );
    }
}

// ------------------------------------------------------- HSM client certificates

/// `CreateHsmClientCertificate` for one identifier.
async fn create_certificate(emulator: &Emulator, identifier: &str) -> Answer {
    emulator
        .call(
            "CreateHsmClientCertificate",
            &[("HsmClientCertificateIdentifier", identifier)],
        )
        .await
}

#[tokio::test]
async fn the_hsm_client_certificate_read_and_delete_word_their_refusals_differently() {
    let emulator = Emulator::new();

    let present = create_certificate(&emulator, "hsm-cert-present").await;
    assert_eq!(
        present.status, 200,
        "case seed the present certificate: expected 200, got {} with body {}",
        present.status, present.body
    );
    let doomed = create_certificate(&emulator, "hsm-cert-doomed").await;
    assert_eq!(
        doomed.status, 200,
        "case seed the doomed certificate: expected 200, got {} with body {}",
        doomed.status, doomed.body
    );
    let removed = emulator
        .call(
            "DeleteHsmClientCertificate",
            &[("HsmClientCertificateIdentifier", "hsm-cert-doomed")],
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case delete the doomed certificate: expected 200, got {} with body {}",
        removed.status, removed.body
    );

    // Rows differing along HOW the identifier misses: nothing resembles it, a
    // case variant of a stored one (the identifier is matched exactly as it was
    // stored), a prefix, one extra character, and one this account created and
    // then deleted.
    for (case, identifier) in [
        ("an identifier nothing resembles", "hsm-cert-absent"),
        ("an upper-case variant of a stored one", "HSM-CERT-PRESENT"),
        ("a prefix of a stored one", "hsm-cert-prese"),
        ("a stored one with one extra character", "hsm-cert-presentx"),
        (
            "a certificate this account created and then deleted",
            "hsm-cert-doomed",
        ),
    ] {
        let read = emulator
            .call(
                "DescribeHsmClientCertificates",
                &[("HsmClientCertificateIdentifier", identifier)],
            )
            .await;
        let read_case = format!("DescribeHsmClientCertificates on {case}");
        assert_refused(&read_case, &read, 400, "HsmClientCertificateNotFoundFault");
        assert_message(
            &read_case,
            &read,
            &format!("HSM client certificate '{identifier}' not found."),
        );

        let deleted = emulator
            .call(
                "DeleteHsmClientCertificate",
                &[("HsmClientCertificateIdentifier", identifier)],
            )
            .await;
        let delete_case = format!("DeleteHsmClientCertificate on {case}");
        assert_refused(
            &delete_case,
            &deleted,
            400,
            "HsmClientCertificateNotFoundFault",
        );
        assert_message(
            &delete_case,
            &deleted,
            &format!("HSM client certificate not found: {identifier}"),
        );
    }

    // The certificate that exists is still served, so the refusals above were
    // about the identifier rather than about the account.
    let served = emulator
        .call(
            "DescribeHsmClientCertificates",
            &[("HsmClientCertificateIdentifier", "hsm-cert-present")],
        )
        .await;
    assert_eq!(
        served.status, 200,
        "case the surviving certificate: expected 200, got {} with body {}",
        served.status, served.body
    );
}

// ----------------------------------------------------------- snapshot copy grants

/// `CreateSnapshotCopyGrant` for one name.
async fn create_grant(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call("CreateSnapshotCopyGrant", &[("SnapshotCopyGrantName", name)])
        .await
}

#[tokio::test]
async fn the_snapshot_copy_grant_read_and_delete_word_their_refusals_differently() {
    let emulator = Emulator::new();

    let present = create_grant(&emulator, "scg-present").await;
    assert_eq!(
        present.status, 200,
        "case seed the present grant: expected 200, got {} with body {}",
        present.status, present.body
    );
    let doomed = create_grant(&emulator, "scg-doomed").await;
    assert_eq!(
        doomed.status, 200,
        "case seed the doomed grant: expected 200, got {} with body {}",
        doomed.status, doomed.body
    );
    let removed = emulator
        .call(
            "DeleteSnapshotCopyGrant",
            &[("SnapshotCopyGrantName", "scg-doomed")],
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case delete the doomed grant: expected 200, got {} with body {}",
        removed.status, removed.body
    );

    for (case, name) in [
        ("a name nothing resembles", "scg-absent"),
        ("an upper-case variant of a stored one", "SCG-PRESENT"),
        ("a prefix of a stored one", "scg-prese"),
        ("a stored one with one extra character", "scg-presentx"),
        (
            "a grant this account created and then deleted",
            "scg-doomed",
        ),
    ] {
        let read = emulator
            .call(
                "DescribeSnapshotCopyGrants",
                &[("SnapshotCopyGrantName", name)],
            )
            .await;
        let read_case = format!("DescribeSnapshotCopyGrants on {case}");
        assert_refused(&read_case, &read, 400, "SnapshotCopyGrantNotFoundFault");
        assert_message(
            &read_case,
            &read,
            &format!("Snapshot copy grant '{name}' not found."),
        );

        let deleted = emulator
            .call(
                "DeleteSnapshotCopyGrant",
                &[("SnapshotCopyGrantName", name)],
            )
            .await;
        let delete_case = format!("DeleteSnapshotCopyGrant on {case}");
        assert_refused(&delete_case, &deleted, 400, "SnapshotCopyGrantNotFoundFault");
        assert_message(
            &delete_case,
            &deleted,
            &format!("Snapshot copy grant not found: '{name}'."),
        );
    }
}

#[tokio::test]
async fn a_grant_name_already_taken_is_refused_naming_the_requested_identifier() {
    let emulator = Emulator::new();

    // Rows differing along the shape of the name the second create reuses.
    for (case, name) in [
        ("a plain name", "scg-dup"),
        ("a hyphenated name", "scg-dup-with-hyphens"),
        ("a name carrying digits", "scg-dup-2026"),
    ] {
        let created = create_grant(&emulator, name).await;
        assert_eq!(
            created.status, 200,
            "case {case}: the first create must be served, got {} with body {}",
            created.status, created.body
        );

        let duplicate = create_grant(&emulator, name).await;
        assert_refused(case, &duplicate, 400, "SnapshotCopyGrantAlreadyExistsFault");
        assert_message(
            case,
            &duplicate,
            &format!(
                "Cannot create the snapshot copy grant because a grant with the identifier '{name}' already exists."
            ),
        );
    }
}

// ------------------------------------------------------- authentication profiles

/// The one sentence every authentication profile LOOKUP answers when the name
/// identifies no profile. It carries NO variable part: the profile the request
/// asked about is not named.
const PROFILE_NOT_FOUND: &str =
    "The authentication profile does not exist. Check the name and try the operation again.";

/// The one sentence the service answers when the profile map a write carries is
/// not one it can read: the content is no JSON document, no map, or a map with
/// no setting in it. It too carries no variable part.
const PROFILE_MAP_NOT_JSON: &str = "The profile map file doesn't contain valid content in JSON format. \
Edit the map file to add valid content in JSON format or specify another map file that contains valid content in JSON format. \
You can include a path to the file. Instead of a file, you can specify a JSON string.";

/// `CreateAuthenticationProfile` with one name and content.
async fn create_profile(emulator: &Emulator, name: &str, content: &str) -> Answer {
    emulator
        .call(
            "CreateAuthenticationProfile",
            &[
                ("AuthenticationProfileName", name),
                ("AuthenticationProfileContent", content),
            ],
        )
        .await
}

/// `ModifyAuthenticationProfile` with one name and content.
async fn modify_profile(emulator: &Emulator, name: &str, content: &str) -> Answer {
    emulator
        .call(
            "ModifyAuthenticationProfile",
            &[
                ("AuthenticationProfileName", name),
                ("AuthenticationProfileContent", content),
            ],
        )
        .await
}

#[tokio::test]
async fn every_authentication_profile_lookup_answers_one_sentence_naming_no_profile() {
    let emulator = Emulator::new();

    let present = create_profile(&emulator, "ap-present", r#"{"Present":"1"}"#).await;
    assert_eq!(
        present.status, 200,
        "case seed the present profile: expected 200, got {} with body {}",
        present.status, present.body
    );
    let doomed = create_profile(&emulator, "ap-doomed", r#"{"Who":"doomed"}"#).await;
    assert_eq!(
        doomed.status, 200,
        "case seed the doomed profile: expected 200, got {} with body {}",
        doomed.status, doomed.body
    );
    let removed = emulator
        .call(
            "DeleteAuthenticationProfile",
            &[("AuthenticationProfileName", "ap-doomed")],
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case delete the doomed profile: expected 200, got {} with body {}",
        removed.status, removed.body
    );

    for (case, name) in [
        ("a name nothing resembles", "ap-absent"),
        ("an upper-case variant of a stored one", "AP-PRESENT"),
        ("a prefix of a stored one", "ap-prese"),
        ("a stored one with one extra character", "ap-presentx"),
        (
            "a profile this account created and then deleted",
            "ap-doomed",
        ),
    ] {
        // The read, the modify and the delete all answer the not-found fault,
        // worded with one fixed sentence that names no profile — whatever
        // profile map the modify carried.
        for (action, answer) in [
            (
                "DescribeAuthenticationProfiles",
                emulator
                    .call(
                        "DescribeAuthenticationProfiles",
                        &[("AuthenticationProfileName", name)],
                    )
                    .await,
            ),
            (
                "ModifyAuthenticationProfile",
                modify_profile(&emulator, name, r#"{"AllowDBUserOverride":"1"}"#).await,
            ),
            (
                "ModifyAuthenticationProfile with a nested document",
                modify_profile(&emulator, name, r#"{"Profile":{"Region":"eu-west-1"}}"#).await,
            ),
            (
                "DeleteAuthenticationProfile",
                emulator
                    .call(
                        "DeleteAuthenticationProfile",
                        &[("AuthenticationProfileName", name)],
                    )
                    .await,
            ),
        ] {
            let lookup_case = format!("{action} on {case}");
            assert_refused(
                &lookup_case,
                &answer,
                404,
                "AuthenticationProfileNotFoundFault",
            );
            assert_message(&lookup_case, &answer, PROFILE_NOT_FOUND);
            let message = answer.text("Message").unwrap_or_default();
            assert!(
                !message.contains(name),
                "case {lookup_case}: expected the sentence to name no profile, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn profile_content_outside_the_documented_profile_map_is_refused() {
    let emulator = Emulator::new();

    let kept = r#"{"Kept":"1"}"#;
    let created = create_profile(&emulator, "ap-existing", kept).await;
    assert_eq!(
        created.status, 200,
        "case seed the existing profile: expected 200, got {} with body {}",
        created.status, created.body
    );

    // Rows differing along HOW the content fails to be one profile map carrying
    // settings: text that is no JSON document at all, a truncated document, a
    // member with no value, nothing at all, two documents, text quoted the way
    // JSON does not admit, documents that are not maps (an array, a string, a
    // number, a boolean, null) and a map with no setting in it. The profile the
    // modify names EXISTS, so the refusal is about the document the request
    // carried.
    for (case, content) in [
        ("plain text", "not json at all"),
        ("a truncated object", "{"),
        ("a member with no value", r#"{"a":}"#),
        ("an empty string", ""),
        ("two documents", "{} {}"),
        ("single-quoted text", "'quoted'"),
        ("a JSON array rather than a map", r#"[{"Key":"a"}]"#),
        ("a JSON string rather than a map", r#""not-a-map""#),
        ("a JSON number rather than a map", "12345"),
        ("a JSON boolean rather than a map", "true"),
        ("a JSON null rather than a map", "null"),
        ("a map carrying no setting", "{}"),
    ] {
        let modified = modify_profile(&emulator, "ap-existing", content).await;
        let modify_case = format!("ModifyAuthenticationProfile with {case}");
        assert_refused(
            &modify_case,
            &modified,
            400,
            "InvalidAuthenticationProfileRequestFault",
        );
        assert_message(&modify_case, &modified, PROFILE_MAP_NOT_JSON);

        // The create words the same condition the same way.
        let rejected = create_profile(&emulator, "ap-rejected", content).await;
        let create_case = format!("CreateAuthenticationProfile with {case}");
        assert_refused(
            &create_case,
            &rejected,
            400,
            "InvalidAuthenticationProfileRequestFault",
        );
        assert_message(&create_case, &rejected, PROFILE_MAP_NOT_JSON);
    }

    // Nothing was written: the stored profile still holds the content it held,
    // and the refused create left no record behind.
    let read = emulator
        .call(
            "DescribeAuthenticationProfiles",
            &[("AuthenticationProfileName", "ap-existing")],
        )
        .await;
    assert_eq!(
        read.text("AuthenticationProfileContent").as_deref(),
        Some(kept),
        "case the surviving profile: expected its content untouched, got {:?}",
        read.text("AuthenticationProfileContent")
    );
    let absent = emulator
        .call(
            "DescribeAuthenticationProfiles",
            &[("AuthenticationProfileName", "ap-rejected")],
        )
        .await;
    assert_refused(
        "the refused create stored nothing",
        &absent,
        404,
        "AuthenticationProfileNotFoundFault",
    );

    // The rule refuses content that is not one profile map, not every profile
    // map: a map carrying settings is still served, whatever its values are.
    for (case, content) in [
        ("one setting", r#"{"AllowDBUserOverride":"1"}"#),
        (
            "several settings",
            r#"{"AllowDBUserOverride":"1","Client_ID":"probe","App_ID":"example"}"#,
        ),
        (
            "settings whose values are a nested map and an array",
            r#"{"Profile":{"Region":"eu-central-1"},"Ports":[5439,5440]}"#,
        ),
    ] {
        let served = modify_profile(&emulator, "ap-existing", content).await;
        assert_eq!(
            served.status, 200,
            "case {case}: expected the document to be served with 200, got {} with body {}",
            served.status, served.body
        );
        assert_eq!(
            served.text("AuthenticationProfileContent").as_deref(),
            Some(content),
            "case {case}: expected the modify to report the new content, got {:?}",
            served.text("AuthenticationProfileContent")
        );
    }
}
