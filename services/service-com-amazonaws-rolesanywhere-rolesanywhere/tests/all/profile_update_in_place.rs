// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateProfile`.
//!
//! The rule under test: an update of one stored resource is a PARTIAL write — every
//! member the request carries replaces that member of the stored record, every member it
//! omits keeps the value it had, and every member the update does not model (the
//! enablement flag, the identifiers, the creating account, the attribute mappings, the
//! tags, the instant of creation) is left alone. The instant of last update moves,
//! because the call wrote, and the record as it now stands is what the call answers and
//! what every later read reports. An id that resolves to no stored resource is refused
//! rather than answered, and a value outside a published constraint is refused and
//! stores nothing.
//!
//! Each table's rows differ along what the rule generalises over: which members a write
//! carries, their values and sizes (each published bound and one past it), the length of
//! the two lists, the order of the sequence, the enablement the profile is in when it is
//! written to, and the Region the request is signed for.
//!
//! The harness below is local to this suite, as in each sibling suite.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the suite signs for unless a case varies it.
const REGION: &str = "us-west-2";

/// The account every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The published bounds of `UpdateProfile`'s input members
/// (`smithy_query(query="operation", operation="UpdateProfile")`):
/// `name` is `ResourceName` `@length(1, 255)` `@pattern("^[ a-zA-Z0-9-_]*$")`,
/// `sessionPolicy` is `@length(1, 100000)`, `durationSeconds` is `@range(900, 43200)`,
/// `roleArns` is `@length(0, 250)` and `managedPolicyArns` is `@length(0, 50)` with
/// members of `@length(1, 200)`.
const NAME_MIN_LENGTH: usize = 1;
const NAME_MAX_LENGTH: usize = 255;
const SESSION_POLICY_MIN_LENGTH: usize = 1;
const SESSION_POLICY_MAX_LENGTH: usize = 100_000;
const DURATION_MIN_SECONDS: i64 = 900;
const DURATION_MAX_SECONDS: i64 = 43_200;
const ROLE_ARNS_MAX_LENGTH: usize = 250;
const MANAGED_POLICIES_MAX_LENGTH: usize = 50;
const MANAGED_POLICY_MAX_LENGTH: usize = 200;

/// Ids shaped like the modelled `Uuid`
/// (`@length(36, 36)`, `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`) that
/// nothing in the store is ever created under, so the boundary validator accepts them and
/// the handler has to resolve them.
const UNMINTED_IDS: [(&str, &str); 3] = [
    ("all-zero-id", "00000000-0000-0000-0000-000000000000"),
    ("hex-words-id", "deadbeef-dead-beef-dead-beefdeadbeef"),
    ("all-f-id", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
];

/// Ids OUTSIDE the modelled `Uuid` form: too short by one, too long by one, uppercase hex,
/// no separators, and not a uuid at all.
const MALFORMED_IDS: [&str; 5] = [
    "0123abcd-4567-89ef-0123-456789abcde",
    "0123abcd-4567-89ef-0123-456789abcdef0",
    "0123ABCD-4567-89EF-0123-456789ABCDEF",
    "0123abcd456789ef0123456789abcdef0123",
    "not-a-uuid",
];

/// The constraint a `roleArns` entry that is not an IAM role ARN fails: the same one
/// `CreateProfile` refuses such an entry with, so a list one operation stores is a list
/// the other accepts.
const ROLE_ARN_CONSTRAINT: &str =
    "Member must be an IAM role ARN of the form arn:<partition>:iam::<account>:role/<name>";

struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body and the wire error code.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Answer {
    fn message(&self) -> Option<&str> {
        self.body.get("message").and_then(Value::as_str)
    }

    /// The `profile` member of a successful answer.
    fn profile(&self, case: &str) -> Value {
        self.body
            .pointer("/profile")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a profile member, got {self:?}"))
    }
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send_in_region(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        region: &str,
    ) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        self.send_in_region(method, uri, body, REGION).await
    }

    async fn create_profile(&self, body: Value) -> Answer {
        self.send("POST", "/profiles", Some(body)).await
    }

    /// `PATCH /profile/{profileId}`
    /// (`smithy_query(query="operation", operation="UpdateProfile")`).
    async fn update_profile(&self, id: &str, body: Value) -> Answer {
        self.send("PATCH", &format!("/profile/{id}"), Some(body))
            .await
    }

    async fn update_profile_in_region(&self, id: &str, body: Value, region: &str) -> Answer {
        self.send_in_region("PATCH", &format!("/profile/{id}"), Some(body), region)
            .await
    }

    async fn get_profile(&self, id: &str) -> Answer {
        self.send("GET", &format!("/profile/{id}"), None).await
    }

    async fn delete_profile(&self, id: &str) -> Answer {
        self.send("DELETE", &format!("/profile/{id}"), None).await
    }

    async fn list_profiles(&self) -> Answer {
        self.send("GET", "/profiles", None).await
    }

    async fn disable_profile(&self, id: &str) -> Answer {
        self.send("POST", &format!("/profile/{id}/disable"), None)
            .await
    }

    async fn put_mapping(&self, id: &str, certificate_field: &str, specifier: &str) -> Answer {
        self.send(
            "PUT",
            &format!("/profiles/{id}/mappings"),
            Some(json!({
                "certificateField": certificate_field,
                "mappingRules": [{"specifier": specifier}],
            })),
        )
        .await
    }

    async fn list_tags(&self, resource_arn: &str) -> Answer {
        self.send(
            "GET",
            &format!("/ListTagsForResource?resourceArn={resource_arn}"),
            None,
        )
        .await
    }
}

/// An IAM role ARN a profile can vend credentials for.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The ARN a profile id carries in `region`, which is the ARN a refusal naming that id
/// words: `ProfileArn` is
/// `@pattern("^arn:aws(-[^:]+)?:rolesanywhere(:.*){2}(:profile.*)$")`
/// (`smithy_query(query="shape", shape="ProfileDetail")`, member `profileArn`).
fn profile_arn(region: &str, profile_id: &str) -> String {
    format!("arn:aws:rolesanywhere:{region}:{ACCOUNT}:profile/{profile_id}")
}

/// The wording an ARN-keyed miss carries.
fn arn_not_found_message(resource_arn: &str) -> String {
    format!("The item doesn't exist. arn: {resource_arn} ; aborting.")
}

/// The wording a refusal of one member against one constraint carries.
fn validation_message(member: &str, constraint: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: {constraint}"
    )
}

/// Assert a call succeeded with the status its operation answers.
fn ok(case: &str, expected_status: u16, answer: &Answer) {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
    );
}

/// One profile detail with its `updatedAt` dropped, so two snapshots can be compared on
/// everything the instant of the last write is allowed to differ on.
fn without_updated_at(case: &str, detail: &Value) -> Value {
    let mut object = detail
        .as_object()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a profile detail must be an object, got {detail:?}")
        })
        .clone();
    object.remove("updatedAt");
    Value::Object(object)
}

/// The instant one `date-time` member of a detail carries.
fn instant(case: &str, detail: &Value, member: &str) -> DateTime {
    let text = detail
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {member} must be an ISO-8601 string, got {detail:?}")
        });
    DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} {text:?} must parse as a date-time, got {error}")
    })
}

/// Create a profile and answer `(id, the detail the create returned)`.
async fn created(client: &Client, case: &str, body: Value) -> (String, Value) {
    let answer = client.create_profile(body).await;
    ok(case, 201, &answer);
    let detail = answer.profile(case);
    let id = detail
        .get("profileId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no profileId in {detail:?}"))
        .to_string();
    (id, detail)
}

/// The element of the `ListProfiles` projection that reports this profile.
async fn listed(client: &Client, case: &str, id: &str) -> Value {
    let answer = client.list_profiles().await;
    ok(case, 200, &answer);
    answer
        .body
        .get("profiles")
        .and_then(Value::as_array)
        .and_then(|elements| {
            elements
                .iter()
                .find(|entry| entry.get("profileId").and_then(Value::as_str) == Some(id))
                .cloned()
        })
        .unwrap_or_else(|| panic!("case {case:?}: {id} must appear in {answer:?}"))
}

/// A body of every member an update can carry, so a case that must leave the unmodelled
/// members alone writes as much as the operation allows.
fn every_member_body() -> Value {
    json!({
        "name": "written by the update",
        "roleArns": [role_arn("Updated"), role_arn("team/Nested")],
        "managedPolicyArns": ["arn:aws:iam::aws:policy/AdministratorAccess"],
        "sessionPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Deny\"}]}",
        "durationSeconds": 7200,
        "acceptRoleSessionName": true,
    })
}

/// A SEQUENCE of updates on ONE profile composes in place: each write replaces the
/// members it carries on top of what the write before it left, every member it omits
/// survives, and the result is what the operation's own answer, `GetProfile` and the
/// `ListProfiles` projection all report.
///
/// The rows span which members a write carries, their values at each published bound, the
/// length of the two lists, a re-sent body and a body carrying nothing at all.
#[tokio::test]
async fn update_profile_composes_in_place_across_a_sequence_of_writes() {
    let client = Client::new();
    // A sibling the sequence never addresses, so a write that reached the wrong record
    // shows up at the end.
    let (sibling_id, sibling_detail) = created(
        &client,
        "sibling",
        json!({"name": "untouched sibling", "roleArns": [role_arn("Sibling")]}),
    )
    .await;
    let (id, create_detail) = created(
        &client,
        "create",
        json!({
            "name": "as created",
            "roleArns": [role_arn("Created")],
            "managedPolicyArns": ["arn:aws:iam::aws:policy/ReadOnlyAccess"],
            "sessionPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            "durationSeconds": 3600,
            "requireInstanceProperties": true,
            "acceptRoleSessionName": false,
        }),
    )
    .await;

    let every_member = every_member_body();
    let steps: Vec<(&str, Value)> = vec![
        ("name-only", json!({"name": "renamed once"})),
        (
            "name-at-the-length-floor",
            json!({"name": "n".repeat(NAME_MIN_LENGTH)}),
        ),
        (
            "name-at-the-length-ceiling",
            json!({"name": "n".repeat(NAME_MAX_LENGTH)}),
        ),
        (
            "name-with-every-allowed-character-class",
            json!({"name": "a Z 0-9_-"}),
        ),
        (
            "duration-at-the-published-floor",
            json!({"durationSeconds": DURATION_MIN_SECONDS}),
        ),
        (
            "duration-at-the-published-ceiling",
            json!({"durationSeconds": DURATION_MAX_SECONDS}),
        ),
        ("role-arns-emptied", json!({"roleArns": []})),
        (
            "role-arns-replaced-with-several",
            json!({
                "roleArns": (0..7).map(|index| role_arn(&format!("Bulk{index}")))
                    .collect::<Vec<String>>(),
            }),
        ),
        (
            "role-arns-at-the-list-ceiling",
            json!({
                "roleArns": (0..ROLE_ARNS_MAX_LENGTH)
                    .map(|index| role_arn(&format!("Max{index}")))
                    .collect::<Vec<String>>(),
            }),
        ),
        (
            "managed-policies-replaced",
            json!({
                "managedPolicyArns": [
                    "arn:aws:iam::aws:policy/ReadOnlyAccess",
                    format!("arn:aws:iam::{ACCOUNT}:policy/Custom"),
                ],
            }),
        ),
        (
            "managed-policies-at-the-list-ceiling",
            json!({
                "managedPolicyArns": (0..MANAGED_POLICIES_MAX_LENGTH)
                    .map(|index| format!("arn:aws:iam::{ACCOUNT}:policy/Many{index}"))
                    .collect::<Vec<String>>(),
            }),
        ),
        (
            "managed-policy-at-the-member-length-ceiling",
            json!({
                "managedPolicyArns": [
                    format!(
                        "arn:aws:iam::{ACCOUNT}:policy/{}",
                        "p".repeat(
                            MANAGED_POLICY_MAX_LENGTH
                                - format!("arn:aws:iam::{ACCOUNT}:policy/").len()
                        )
                    ),
                ],
            }),
        ),
        ("managed-policies-emptied", json!({"managedPolicyArns": []})),
        (
            "session-policy-at-the-length-floor",
            json!({"sessionPolicy": "s".repeat(SESSION_POLICY_MIN_LENGTH)}),
        ),
        (
            "session-policy-at-the-length-ceiling",
            json!({"sessionPolicy": "s".repeat(SESSION_POLICY_MAX_LENGTH)}),
        ),
        (
            "accept-role-session-name-set",
            json!({"acceptRoleSessionName": true}),
        ),
        (
            "accept-role-session-name-cleared",
            json!({"acceptRoleSessionName": false}),
        ),
        ("every-member-at-once", every_member.clone()),
        // The same body again: a re-sent write leaves the same values, so only the
        // instant of the last write may differ.
        ("the-same-body-again", every_member),
        // A body carrying nothing still writes the record, so nothing but that instant
        // may move.
        ("no-members", json!({})),
    ];

    // Every member the update can write, tracked as the sequence writes it. A member a
    // step does not carry keeps whatever the step before it left, so the whole expected
    // detail is the create's detail with the carried members overwritten.
    let mut expected: Map<String, Value> = create_detail
        .as_object()
        .expect("the create reports an object")
        .clone();
    let mut previous_updated = instant("create", &create_detail, "updatedAt");

    for (case, body) in &steps {
        for (member, value) in body.as_object().expect("a step body is an object") {
            expected.insert(member.clone(), value.clone());
        }

        let answer = client.update_profile(&id, body.clone()).await;
        ok(case, 200, &answer);
        let detail = answer.profile(case);
        assert_eq!(
            without_updated_at(case, &detail),
            without_updated_at(case, &Value::Object(expected.clone())),
            "case {case:?}: the update must write the members it carries and leave every \
             other one alone, got {detail:?}"
        );

        // The instant of the last write moves forward and never behind the creation.
        let updated_at = instant(case, &detail, "updatedAt");
        assert!(
            updated_at.as_nanos() >= previous_updated.as_nanos(),
            "case {case:?}: updatedAt must not go backwards, got {updated_at:?} after \
             {previous_updated:?}"
        );
        assert!(
            updated_at.as_nanos() >= instant(case, &detail, "createdAt").as_nanos(),
            "case {case:?}: updatedAt must not precede createdAt, got {detail:?}"
        );
        previous_updated = updated_at;

        // Persisted: both read paths answer exactly what the write answered.
        let read = client.get_profile(&id).await;
        ok(case, 200, &read);
        assert_eq!(
            read.profile(case),
            detail,
            "case {case:?}: GetProfile must report what the update wrote, got {read:?}"
        );
        assert_eq!(
            listed(&client, case, &id).await,
            detail,
            "case {case:?}: the ListProfiles projection must report what the update wrote"
        );
    }

    // The sibling is untouched, members and timestamps alike.
    let sibling = client.get_profile(&sibling_id).await;
    ok("sibling", 200, &sibling);
    assert_eq!(
        sibling.profile("sibling"),
        sibling_detail,
        "case \"sibling\": no other profile may change, got {sibling:?}"
    );
}

/// An update writes only the members it models: the enablement flag, the identifiers, the
/// creating account, the instant of creation, the unused instance-properties flag, the
/// attribute mappings and the tags all survive a write carrying every member the
/// operation has.
///
/// The rows span the enablement the profile is in when it is written to, and how it got
/// there.
#[tokio::test]
async fn update_profile_leaves_the_members_it_does_not_model_alone() {
    let cases: [(&str, Value, bool, bool); 4] = [
        (
            "created-enabled-by-default",
            json!({"name": "default enablement", "roleArns": [role_arn("Default")]}),
            false,
            true,
        ),
        (
            "created-explicitly-enabled",
            json!({"name": "explicitly enabled", "roleArns": [role_arn("Enabled")], "enabled": true}),
            false,
            true,
        ),
        (
            "created-disabled",
            json!({"name": "created disabled", "roleArns": [role_arn("Disabled")], "enabled": false}),
            false,
            false,
        ),
        (
            "disabled-after-creation",
            json!({"name": "disabled later", "roleArns": [role_arn("Later")], "enabled": true}),
            true,
            false,
        ),
    ];

    for (case, mut create_body, disable_first, expected_enabled) in cases {
        let client = Client::new();
        create_body["tags"] = json!([{"key": "owner", "value": "platform"}]);
        create_body["requireInstanceProperties"] = Value::Bool(true);
        let (id, _) = created(&client, case, create_body).await;

        // A mapping on the profile, so a write that dropped the mappings shows up.
        let mapped = client.put_mapping(&id, "x509Subject", "CN").await;
        ok(&format!("{case}/put-mapping"), 200, &mapped);

        if disable_first {
            let disabled = client.disable_profile(&id).await;
            ok(&format!("{case}/disable"), 200, &disabled);
        }

        let before = client.get_profile(&id).await;
        ok(&format!("{case}/before"), 200, &before);
        let before_detail = before.profile(case);
        let arn = before_detail
            .get("profileArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no profileArn in {before_detail:?}"))
            .to_string();
        let tags_before = client.list_tags(&arn).await;
        ok(&format!("{case}/tags-before"), 200, &tags_before);

        let answer = client.update_profile(&id, every_member_body()).await;
        ok(case, 200, &answer);
        let detail = answer.profile(case);

        for member in [
            "enabled",
            "profileId",
            "profileArn",
            "createdBy",
            "createdAt",
            "requireInstanceProperties",
            "attributeMappings",
        ] {
            assert_eq!(
                detail.get(member),
                before_detail.get(member),
                "case {case:?}: {member} is not a member of this request, so the update must \
                 leave it alone, got {detail:?}"
            );
        }
        assert_eq!(
            detail.get("enabled"),
            Some(&Value::Bool(expected_enabled)),
            "case {case:?}: expected enabled {expected_enabled}, got {detail:?}"
        );

        // Every member the request DID carry was written, so the write is not a no-op.
        for (member, value) in every_member_body()
            .as_object()
            .expect("the body is an object")
        {
            assert_eq!(
                detail.get(member),
                Some(value),
                "case {case:?}: expected {member} {value:?}, got {detail:?}"
            );
        }

        let tags_after = client.list_tags(&arn).await;
        ok(&format!("{case}/tags-after"), 200, &tags_after);
        assert_eq!(
            tags_after.body, tags_before.body,
            "case {case:?}: the update must leave the resource's tags alone, got {tags_after:?}"
        );

        let read = client.get_profile(&id).await;
        ok(&format!("{case}/read"), 200, &read);
        assert_eq!(
            read.profile(case),
            detail,
            "case {case:?}: GetProfile must report what the update wrote, got {read:?}"
        );
    }
}

/// An id that resolves to no stored profile is the modelled 404 carrying the ARN-keyed
/// wording — never a canned success — an id outside the modelled `Uuid` form is a 400, and
/// neither writes anything.
///
/// The rows span why the id does not resolve (never minted, deleted a moment ago) and the
/// Region the request is signed for, which is the only variable part of the message.
#[tokio::test]
async fn update_profile_refuses_an_id_that_resolves_to_nothing() {
    let client = Client::new();

    let (sibling_id, sibling_detail) = created(
        &client,
        "sibling",
        json!({"name": "sibling", "roleArns": [role_arn("Sibling")]}),
    )
    .await;
    let (deleted_id, _) = created(
        &client,
        "deleted",
        json!({"name": "deleted", "roleArns": [role_arn("Deleted")]}),
    )
    .await;
    let removed = client.delete_profile(&deleted_id).await;
    ok("delete-the-deleted-fixture", 200, &removed);

    let mut cases: Vec<(String, String, &str)> = Vec::new();
    for (label, id) in UNMINTED_IDS {
        cases.push((format!("unminted-{label}"), id.to_string(), REGION));
    }
    cases.push(("deleted-profile-id".to_string(), deleted_id.clone(), REGION));
    for region in ["eu-central-1", "ap-southeast-2", "us-east-1"] {
        cases.push((
            format!("unminted-in-{region}"),
            UNMINTED_IDS[1].1.to_string(),
            region,
        ));
    }

    for (case, id, region) in cases {
        let expected = arn_not_found_message(&profile_arn(region, &id));
        let answer = client
            .update_profile_in_region(&id, json!({"name": "renamed"}), region)
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an unresolvable profileId must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            Some(expected.as_str()),
            "case {case:?}: expected message {expected:?}, got {:?}",
            answer.message()
        );
    }

    for id in MALFORMED_IDS {
        let answer = client.update_profile(id, json!({"name": "renamed"})).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {id:?}: an id outside the modelled Uuid form must be refused, got {answer:?}"
        );
    }

    // Not one refusal wrote anything: the stored profile is still exactly as created.
    let still_there = client.get_profile(&sibling_id).await;
    ok("sibling-survives-the-refusals", 200, &still_there);
    assert_eq!(
        still_there.profile("sibling"),
        sibling_detail,
        "case \"sibling-survives-the-refusals\": a refused call must change nothing, got \
         {still_there:?}"
    );
}

/// A value outside a published constraint is refused with the modelled 400 and stores
/// nothing: the profile afterwards is byte for byte the one the update was called on.
///
/// The rows span one step past each published bound — the two length bounds of `name` and
/// its character class, both ends of `durationSeconds`, the length bounds of
/// `sessionPolicy` and the ceilings of the two lists — plus a `roleArns` entry that is not
/// an IAM role ARN, which is refused with the same wording `CreateProfile` refuses one
/// with.
#[tokio::test]
async fn update_profile_refuses_values_outside_the_published_constraints() {
    let client = Client::new();
    let (id, _) = created(
        &client,
        "create",
        json!({
            "name": "guarded",
            "roleArns": [role_arn("Guarded")],
            "sessionPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            "durationSeconds": 3600,
        }),
    )
    .await;
    let before = client.get_profile(&id).await;
    ok("before", 200, &before);
    let before_detail = before.profile("before");

    let cases: Vec<(String, Value, Option<String>)> = vec![
        ("name-empty".to_string(), json!({"name": ""}), None),
        (
            "name-one-past-the-length-ceiling".to_string(),
            json!({"name": "n".repeat(NAME_MAX_LENGTH + 1)}),
            None,
        ),
        (
            "name-outside-the-character-class".to_string(),
            json!({"name": "not/allowed!"}),
            None,
        ),
        (
            "duration-one-below-the-floor".to_string(),
            json!({"durationSeconds": DURATION_MIN_SECONDS - 1}),
            None,
        ),
        (
            "duration-one-above-the-ceiling".to_string(),
            json!({"durationSeconds": DURATION_MAX_SECONDS + 1}),
            None,
        ),
        (
            "session-policy-empty".to_string(),
            json!({"sessionPolicy": ""}),
            None,
        ),
        (
            "session-policy-one-past-the-length-ceiling".to_string(),
            json!({"sessionPolicy": "s".repeat(SESSION_POLICY_MAX_LENGTH + 1)}),
            None,
        ),
        (
            "role-arns-one-past-the-list-ceiling".to_string(),
            json!({
                "roleArns": (0..=ROLE_ARNS_MAX_LENGTH)
                    .map(|index| role_arn(&format!("Over{index}")))
                    .collect::<Vec<String>>(),
            }),
            None,
        ),
        (
            "managed-policies-one-past-the-list-ceiling".to_string(),
            json!({
                "managedPolicyArns": (0..=MANAGED_POLICIES_MAX_LENGTH)
                    .map(|index| format!("arn:aws:iam::{ACCOUNT}:policy/Over{index}"))
                    .collect::<Vec<String>>(),
            }),
            None,
        ),
        // Shaped like an ARN the model's pattern admits, but naming no IAM role: the
        // handler is what judges it, so the wording is this service's own.
        (
            "role-arn-with-a-non-numeric-account".to_string(),
            json!({"roleArns": ["arn:aws:iam::ACCOUNT:role/Name"]}),
            Some(validation_message("roleArns.1.member", ROLE_ARN_CONSTRAINT)),
        ),
        (
            "role-arn-naming-another-service".to_string(),
            json!({"roleArns": [role_arn("Fine"), "arn:aws:s3:::bucket".to_string()]}),
            None,
        ),
        (
            "role-arn-that-is-not-an-arn".to_string(),
            json!({"roleArns": ["role/NotAnArn"]}),
            None,
        ),
    ];

    for (case, body, expected_message) in cases {
        let answer = client.update_profile(&id, body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a value outside a published constraint must be refused, got \
             {answer:?}"
        );
        if let Some(expected) = expected_message {
            assert_eq!(
                answer.message(),
                Some(expected.as_str()),
                "case {case:?}: expected message {expected:?}, got {:?}",
                answer.message()
            );
        }

        // Nothing was written: not even the instant of the last update moved.
        let read = client.get_profile(&id).await;
        ok(&case, 200, &read);
        assert_eq!(
            read.profile(&case),
            before_detail,
            "case {case:?}: a refused update must store nothing, got {read:?}"
        );
    }
}

/// The create and the update judge a `roleArns` list identically: a list one of them
/// refuses the other refuses with the same wording, and a list one of them stores the
/// other accepts.
///
/// The rows span the form of the offending entry and its position in the list. An entry
/// the modelled `RoleArn` `@pattern` admits is judged by the handler, so the expected
/// wording is this service's own; an entry outside that pattern is refused by the
/// boundary validator before any handler runs, and both operations are held only to
/// wording it identically.
#[tokio::test]
async fn the_create_and_the_update_judge_role_arns_identically() {
    let client = Client::new();
    let (id, _) = created(
        &client,
        "create",
        json!({"name": "judged", "roleArns": [role_arn("Judged")]}),
    )
    .await;

    // `(case, the list, the expected wording when the handler is what judges it)`. List
    // elements are numbered from one, as every refusal of a list element here numbers
    // them.
    let cases: [(&str, Vec<String>, Option<String>); 5] = [
        (
            "non-numeric-account",
            vec!["arn:aws:iam::ACCOUNT:role/Name".to_string()],
            Some(validation_message("roleArns.1.member", ROLE_ARN_CONSTRAINT)),
        ),
        (
            "another-service",
            vec![role_arn("Fine"), "arn:aws:s3:::bucket".to_string()],
            None,
        ),
        ("not-an-arn", vec!["role/NotAnArn".to_string()], None),
        (
            "role-with-no-name",
            vec![format!("arn:aws:iam::{ACCOUNT}:role/")],
            Some(validation_message("roleArns.1.member", ROLE_ARN_CONSTRAINT)),
        ),
        (
            "third-entry-offends",
            vec![
                role_arn("One"),
                role_arn("Two"),
                "arn:aws:iam::ACCOUNT:role/Third".to_string(),
            ],
            Some(validation_message("roleArns.3.member", ROLE_ARN_CONSTRAINT)),
        ),
    ];

    for (case, role_arns, expected) in cases {
        let created_answer = client
            .create_profile(json!({"name": "rejected", "roleArns": role_arns.clone()}))
            .await;
        let updated_answer = client
            .update_profile(&id, json!({"roleArns": role_arns.clone()}))
            .await;
        for (operation, answer) in [
            ("CreateProfile", &created_answer),
            ("UpdateProfile", &updated_answer),
        ] {
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: {operation} must refuse {role_arns:?}, got {answer:?}"
            );
        }
        assert_eq!(
            updated_answer.message(),
            created_answer.message(),
            "case {case:?}: both operations must word the refusal identically, got {:?} and \
             {:?}",
            created_answer.message(),
            updated_answer.message()
        );
        if let Some(expected) = expected {
            assert_eq!(
                updated_answer.message(),
                Some(expected.as_str()),
                "case {case:?}: expected message {expected:?}, got {:?}",
                updated_answer.message()
            );
        }
    }

    // A list the create stores is a list the update accepts, and the other way round.
    let accepted: Vec<String> = vec![
        role_arn("Plain"),
        role_arn("with/a/path/Nested"),
        "arn:aws-us-gov:iam::000000000001:role/Partitioned".to_string(),
    ];
    let (other_id, other_detail) = created(
        &client,
        "accepted-by-the-create",
        json!({"name": "accepted", "roleArns": accepted.clone()}),
    )
    .await;
    assert_eq!(
        other_detail.get("roleArns"),
        Some(&json!(accepted)),
        "case \"accepted-by-the-create\": the create must store the list it accepted, got \
         {other_detail:?}"
    );
    let answer = client
        .update_profile(&other_id, json!({"roleArns": accepted.clone()}))
        .await;
    ok("accepted-by-the-update", 200, &answer);
    assert_eq!(
        answer.profile("accepted-by-the-update").get("roleArns"),
        Some(&json!(accepted)),
        "case \"accepted-by-the-update\": the update must store the list the create stored, \
         got {answer:?}"
    );
}
