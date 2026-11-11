// SPDX-License-Identifier: CC0-1.0

//! Three more of this service's refusal `Message` RULES, each asserted over a TABLE
//! whose rows differ along what the rule generalises over. The sibling suite
//! `error_message_rules.rs` pins the duplicate, not-found, state-conflict and
//! `ValidationException` families; these three are the ones it does not cover.
//!
//! 1. A documented either/or member pair ("specify exactly one of A and B") is violated
//!    in TWO ways, and the refusal states the half the request actually broke: naming
//!    NEITHER member is told that one of the two is needed, naming BOTH is told the two
//!    cannot be sent together. One wording per condition, shared by every pair and every
//!    operation that resolves one. The rows differ along the member pair, the operation
//!    (create and update), which half of the rule was broken and which other members the
//!    request carried.
//! 2. An access operation addressed at a server whose `IdentityProviderType` is not the
//!    directory-backed one answers a text fixed for THAT OPERATION and that server's
//!    modelled type, carrying nothing the caller sent — so two requests refused for the
//!    same condition read identically, and the three operations are told apart by the
//!    action the text names. The rows span the whole non-directory half of the
//!    `IdentityProviderType` value set, the three implemented access operations, and two
//!    differently shaped `ExternalId`s.
//! 3. A `NextToken` this service did not issue answers ONE fixed text, whatever was
//!    being listed: the diagnostic belongs to the shared pagination-token codec, not to
//!    a listing. The rows span ten `@paginated` operations and a table of forged tokens
//!    differing in form and length, including both published bounds of `NextToken`'s
//!    modelled `len 1..6144`.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{custom_directories, member_of, Answer, Emulator, ROLE};

/// Satisfies `DirectoryId`'s modelled `len 12..12`, pattern `^d-[0-9a-f]{10}$`.
const DIRECTORY_ID: &str = "d-0123456789";

/// Satisfies `Function`'s modelled `len 1..170`, pattern `^arn:[a-z-]+:lambda:.*$`.
const LAMBDA_IDP: &str = "arn:aws:lambda:us-east-1:000000000000:function:idp";

/// Group SIDs satisfying `ExternalId`'s modelled `len 1..256`, pattern `^S-1-[\d-]+$`:
/// the form `Get-ADGroup` reports, and the shortest form the pattern admits.
const EXTERNAL_IDS: [&str; 2] = ["S-1-5-21-1234567890-123456789-1234567890-1234", "S-1-0"];

/// The `Message` of an answer, or `""` when it carries none.
fn message_of(answer: &Answer) -> String {
    member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert `answer` is HTTP `status` with error `code` and exactly `expected` as its
/// `Message`, and that none of `absent` — the values the request carried — reached the
/// text. Returns the message so a caller can compare it across rows.
fn assert_message(
    case: &str,
    answer: &Answer,
    status: u16,
    code: &str,
    expected: &str,
    absent: &[&str],
) -> String {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected {code}, got body {}",
        answer.body
    );
    let actual = message_of(answer);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the fixed text {expected:?}, got {actual:?}"
    );
    for fragment in absent {
        assert!(
            !actual.contains(fragment),
            "case {case:?}: expected no request data in Message, got {actual:?} carrying \
             {fragment:?}"
        );
    }
    actual
}

// ---------------------------------------------------------------------------
// Rule 1: each half of an either/or requirement is reported as the condition it is
// ---------------------------------------------------------------------------

/// `As2ConnectorConfig.LocalProfileId` / `PartnerProfileId` satisfy `ProfileId`'s
/// modelled `len 19..19`, pattern `^p-([0-9a-f]{17})$`.
const LOCAL_PROFILE: &str = "p-11111111111111111";

/// RULE: a request that names NEITHER member of a documented either/or pair is told that
/// one of the two is needed; one that names BOTH is told the two cannot be sent
/// together. The text follows the condition reported, not the whole rule, and names only
/// the two members.
///
/// The rows differ along the member pair (`As2Config`/`SftpConfig` on a connector,
/// `BaseDirectory`/`CustomDirectories` on an agreement), the operation that resolves it
/// (`CreateConnector`, `UpdateConnector`, `CreateAgreement`), which half of the rule was
/// broken, and the other members the request carried.
#[tokio::test]
async fn an_either_or_pair_reports_the_half_of_the_rule_the_request_broke() {
    let emulator = Emulator::new();

    // A stored SFTP connector, so an UPDATE can break the same pair a create does.
    let created = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the stored connector must be created, got {} with body {}",
        created.status, created.body
    );
    let connector_id = created
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("CreateConnector answered no ConnectorId: {}", created.body))
        .to_string();

    // One server and two profiles, so the agreement rows reference resolvable resources
    // and are refused for their directory configuration alone.
    let server_id = emulator.create_server(json!({})).await;
    let local_profile = emulator
        .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
        .await;
    let partner_profile = emulator
        .create_profile(json!({"As2Id": "partner-party", "ProfileType": "PARTNER"}))
        .await;
    let agreement_base = json!({
        "ServerId": &server_id,
        "LocalProfileId": &local_profile,
        "PartnerProfileId": &partner_profile,
        "AccessRole": ROLE,
    });

    /// The members a row adds to its operation's base request.
    fn with(base: &Value, extra: Value) -> Value {
        let mut body = base.clone();
        let object = body.as_object_mut().expect("the request body is an object");
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            object.insert(member.clone(), value.clone());
        }
        body
    }

    let connector_base = json!({"AccessRole": ROLE});
    let update_base = json!({"ConnectorId": &connector_id});

    // (case, operation, request, the expected Message)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "CreateConnector naming neither configuration, with a Url",
            "CreateConnector",
            with(
                &connector_base,
                json!({"Url": "sftp://partner.example.com"}),
            ),
            "One of As2Config or SftpConfig must be specified".to_string(),
        ),
        (
            "CreateConnector naming neither configuration and no Url either",
            "CreateConnector",
            connector_base.clone(),
            "One of As2Config or SftpConfig must be specified".to_string(),
        ),
        (
            "CreateConnector naming both configurations",
            "CreateConnector",
            with(
                &connector_base,
                json!({
                    "Url": "https://partner.example.com/as2",
                    "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                    "SftpConfig": {"UserSecretId": "sftp"},
                }),
            ),
            "As2Config and SftpConfig cannot both be specified".to_string(),
        ),
        (
            "UpdateConnector adding an As2Config to a stored SFTP connector",
            "UpdateConnector",
            with(
                &update_base,
                json!({"As2Config": {"LocalProfileId": LOCAL_PROFILE}}),
            ),
            "As2Config and SftpConfig cannot both be specified".to_string(),
        ),
        (
            "CreateAgreement naming neither directory configuration",
            "CreateAgreement",
            agreement_base.clone(),
            "One of BaseDirectory or CustomDirectories must be specified".to_string(),
        ),
        (
            "CreateAgreement whose BaseDirectory is the blank value its pattern admits, \
             which names no directory",
            "CreateAgreement",
            with(&agreement_base, json!({"BaseDirectory": ""})),
            "One of BaseDirectory or CustomDirectories must be specified".to_string(),
        ),
        (
            "CreateAgreement naming both directory configurations",
            "CreateAgreement",
            with(
                &agreement_base,
                json!({
                    "BaseDirectory": "/bucket/base",
                    "CustomDirectories": custom_directories("/bucket/custom"),
                }),
            ),
            "BaseDirectory and CustomDirectories cannot both be specified".to_string(),
        ),
    ];

    for (case, op, request, expected) in cases {
        let answer = emulator.call(op, request).await;
        // 400 is `InvalidRequestException`'s modelled `@httpError`.
        assert_message(
            case,
            &answer,
            400,
            "InvalidRequestException",
            &expected,
            &[
                ROLE,
                connector_id.as_str(),
                server_id.as_str(),
                local_profile.as_str(),
                partner_profile.as_str(),
                LOCAL_PROFILE,
            ],
        );
    }

    // The two halves are DIFFERENT texts: the condition reported is what the caller has
    // to act on.
    let neither = emulator
        .call("CreateConnector", json!({"AccessRole": ROLE}))
        .await;
    let both = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {"LocalProfileId": LOCAL_PROFILE},
                "SftpConfig": {"UserSecretId": "sftp"},
            }),
        )
        .await;
    assert_ne!(
        message_of(&neither),
        message_of(&both),
        "case \"the two halves of one either/or rule\": expected the two conditions to be \
         worded differently, both are {:?}",
        message_of(&neither)
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the access directory-provider refusal is fixed per operation and type
// ---------------------------------------------------------------------------

/// Every `IdentityProviderType` an access cannot be addressed on, with the
/// `IdentityProviderDetails` that type requires. The value set is
/// `[SERVICE_MANAGED, API_GATEWAY, AWS_DIRECTORY_SERVICE, AWS_LAMBDA]` per
/// `smithy_query(query="shape",shape="IdentityProviderType")`; this is all of it except
/// the directory-backed one, plus the request that names no type at all and so resolves
/// to the documented default.
fn non_directory_servers() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "no IdentityProviderType at all",
            json!({}),
            "SERVICE_MANAGED",
        ),
        (
            "SERVICE_MANAGED",
            json!({"IdentityProviderType": "SERVICE_MANAGED"}),
            "SERVICE_MANAGED",
        ),
        (
            "API_GATEWAY",
            json!({
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {"Url": "https://example.invalid/auth"},
            }),
            "API_GATEWAY",
        ),
        (
            "AWS_LAMBDA",
            json!({
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": {"Function": LAMBDA_IDP},
            }),
            "AWS_LAMBDA",
        ),
    ]
}

/// RULE: an access operation addressed at a server with no directory is refused 400
/// `InvalidRequestException` whose `Message` is fixed for that OPERATION and the server's
/// modelled `IdentityProviderType` — carrying neither the `ServerId` nor the `ExternalId`
/// the request sent, so the same condition always reads the same.
///
/// The rows differ along the operation (the three implemented access operations), the
/// server's identity-provider type (the whole non-directory half of the value set,
/// including the absent member that resolves to the default), the shape of the
/// `ExternalId`, and whether that `ExternalId` was ever granted anywhere.
#[tokio::test]
async fn the_access_directory_refusal_is_fixed_per_operation_and_server_type() {
    let emulator = Emulator::new();

    // (operation, the action its text names)
    let operations: [(&str, &str); 3] = [
        ("CreateAccess", "create"),
        ("DescribeAccess", "describe"),
        ("DeleteAccess", "delete"),
    ];

    for (configuration, server_request, idp_type) in non_directory_servers() {
        let server_id = emulator.create_server(server_request).await;
        for (op, action) in operations {
            let mut seen: Vec<String> = Vec::new();
            for external_id in EXTERNAL_IDS {
                let mut request = json!({"ServerId": &server_id, "ExternalId": external_id});
                if op == "CreateAccess" {
                    request["Role"] = json!(ROLE);
                }
                let case = format!("{op} on {configuration} for {external_id}");
                let answer = emulator.call(op, request).await;
                seen.push(assert_message(
                    &case,
                    &answer,
                    400,
                    "InvalidRequestException",
                    &format!("Cannot {action} access on server with IdP type: {idp_type}"),
                    &[server_id.as_str(), external_id, ROLE],
                ));
            }
            assert!(
                seen.windows(2).all(|pair| pair[0] == pair[1]),
                "case {op:?} on {configuration:?}: expected one text for the condition, got \
                 {seen:?}"
            );
        }

        // Nothing was written by any of the refusals.
        assert!(
            emulator.store().lock().accesses.is_empty(),
            "case {configuration:?}: a refused access operation must persist nothing"
        );
    }

    // The three operations are told APART by the action their text names.
    let server_id = emulator.create_server(json!({})).await;
    let mut texts: Vec<String> = Vec::new();
    for (op, _) in operations {
        let mut request = json!({"ServerId": &server_id, "ExternalId": EXTERNAL_IDS[0]});
        if op == "CreateAccess" {
            request["Role"] = json!(ROLE);
        }
        texts.push(message_of(&emulator.call(op, request).await));
    }
    for (index, text) in texts.iter().enumerate() {
        for other in texts.iter().skip(index + 1) {
            assert_ne!(
                text, other,
                "case \"one refusal per access operation\": expected each operation's text to \
                 differ, got {texts:?}"
            );
        }
    }

    // And the gate refuses only the servers that have no directory: on the
    // directory-backed one all three operations are served, so the grant round-trips.
    let directory_server = emulator
        .create_server(json!({
            "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
            "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
        }))
        .await;
    for external_id in EXTERNAL_IDS {
        for (op, request) in [
            (
                "CreateAccess",
                json!({"ServerId": &directory_server, "ExternalId": external_id, "Role": ROLE}),
            ),
            (
                "DescribeAccess",
                json!({"ServerId": &directory_server, "ExternalId": external_id}),
            ),
            (
                "DeleteAccess",
                json!({"ServerId": &directory_server, "ExternalId": external_id}),
            ),
        ] {
            let answer = emulator.call(op, request).await;
            assert_eq!(
                answer.status, 200,
                "case \"{op} on the directory-backed server for {external_id}\": expected 200, \
                 got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: one fixed text for a `NextToken` the service did not issue
// ---------------------------------------------------------------------------

/// The longest `NextToken` the model admits (`len 1..6144`), in a form the token codec
/// cannot have produced.
fn longest_forged_token() -> String {
    "z".repeat(6144)
}

/// RULE: a `NextToken` this service did not issue is refused 400
/// `InvalidNextTokenException` with ONE fixed text, the same for every listing and every
/// forged token, and the rejected token is not echoed back.
///
/// The rows differ along the operation (ten of this service's `@paginated` listings,
/// including the two scoped to a server and the one scoped to a resource ARN) and the
/// form and length of the forged token: not hex at all, odd-length hex, hex that decodes
/// to bytes that are not text, and both published bounds of the modelled `len 1..6144`.
#[tokio::test]
async fn a_forged_next_token_is_refused_with_one_fixed_text_for_every_listing() {
    let emulator = Emulator::new();

    // A server, a user and a connector, so the listings have something to page and the
    // server-scoped rows address a server that exists.
    let server_id = emulator.create_server(json!({})).await;
    emulator.create_user(&server_id, "alice").await;
    emulator
        .create_profile(json!({"As2Id": "local-party", "ProfileType": "LOCAL"}))
        .await;
    let described = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await;
    let server_arn = member_of(described.get("Server"), "Arn")
        .as_str()
        .unwrap_or_else(|| panic!("DescribeServer answered no Arn: {}", described.body))
        .to_string();

    // (operation, the request members other than the paging ones)
    let listings: Vec<(&str, Value)> = vec![
        ("ListProfiles", json!({})),
        ("ListServers", json!({})),
        ("ListConnectors", json!({})),
        ("ListWorkflows", json!({})),
        ("ListWebApps", json!({})),
        ("ListCertificates", json!({})),
        ("ListSecurityPolicies", json!({})),
        ("ListUsers", json!({"ServerId": &server_id})),
        ("ListHostKeys", json!({"ServerId": &server_id})),
        ("ListTagsForResource", json!({"Arn": &server_arn})),
    ];

    let longest = longest_forged_token();
    // (what the token is, the token). None of them is a value the codec issues: it emits
    // the hex of a stored key, which is even-length hex decoding to text.
    let tokens: Vec<(&str, &str)> = vec![
        ("not hex at all", "not-a-token"),
        ("one character, the modelled minimum length", "z"),
        ("odd-length hex", "abc"),
        ("hex that decodes to bytes that are not text", "c0ffee00"),
        ("the shortest such hex", "ff"),
        ("the modelled maximum length", longest.as_str()),
    ];

    let mut seen: Vec<(String, String)> = Vec::new();
    for (op, base) in &listings {
        for (shape, token) in &tokens {
            let mut request = (*base).clone();
            request["NextToken"] = json!(token);
            let case = format!("{op} with a NextToken that is {shape}");
            let answer = emulator.call(op, request).await;
            let message = assert_message(
                &case,
                &answer,
                400,
                "InvalidNextTokenException",
                "Invalid nextToken",
                &[token, server_id.as_str(), server_arn.as_str()],
            );
            seen.push((case, message));
        }
    }

    for (case, message) in &seen {
        assert_eq!(
            *message, seen[0].1,
            "case {case:?}: expected the one token diagnostic {:?}, got {message:?}",
            seen[0].1
        );
    }

    // A listing whose token the service DID issue still pages, so the refusal is about
    // the token and not about sending one.
    emulator.create_user(&server_id, "bob").await;
    let first = emulator
        .call("ListUsers", json!({"ServerId": &server_id, "MaxResults": 1}))
        .await;
    let issued = first
        .get("NextToken")
        .as_str()
        .unwrap_or_else(|| panic!("ListUsers answered no NextToken: {}", first.body))
        .to_string();
    let second = emulator
        .call(
            "ListUsers",
            json!({"ServerId": &server_id, "MaxResults": 1, "NextToken": issued}),
        )
        .await;
    assert_eq!(
        second.status, 200,
        "case \"a NextToken the service issued\": expected 200, got {} with body {}",
        second.status, second.body
    );
}
