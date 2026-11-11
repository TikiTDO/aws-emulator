// SPDX-License-Identifier: CC0-1.0

//! WHO may read a Keyspaces change stream, driven end to end over the real wire path (an
//! awsJson1_0 request through the multi-service router into the mounted service), so what
//! is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: `AccessDeniedException` is a service-common error of this service
//! (`smithy_query(query="service_info")`), so EVERY operation — the data-plane read
//! included — evaluates the identity-based IAM policies in force on the principal its
//! credentials name, against `cassandra:<OperationName>` on the stream the request
//! addresses, BEFORE it serves anything out of the stream store. A request no statement
//! allows (IAM's default deny), or one an explicit `Deny` covers, is answered HTTP 403
//! `AccessDeniedException` and carries no change record, while a request a matching
//! `Allow` covers reaches the operation's own answer.
//!
//! The user, its policies and its access key live in the IAM store of the SAME shared
//! engine — the store that service's own handlers write — so the documents under test are
//! the ones `iam:PutUserPolicy` stores and the principal a refusal blames is the one the
//! access key belongs to.
//!
//! The tables span what the rule generalises over: the operation (all four of them — the
//! record read, the iterator mint, the stream description and the enumeration), the
//! document in force (none at all, another action of this service, another service's
//! actions, an explicit deny beside an allow, an allow of exactly the action, of the
//! action in another case, of the whole service and of everything), the resource an allow
//! is scoped to (every resource, the table's streams, one stream's own ARN, another
//! table's streams, another Region's) and the principal (an IAM user and the account's own
//! published credential).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::state::{service_state, ChangeRecord};
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and the Region the seeded stream belongs to.
const REGION: &str = "us-east-1";

/// This service's IAM action prefix, which is its `arnNamespace` and its signing name
/// (`smithy_query(query="service_info")`).
const ACTION_PREFIX: &str = "cassandra";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The access key id of the IAM user every row signs with.
const USER_KEY_ID: &str = "AKIAEXAMPLESTREAMKEY1";

/// The IAM user that key belongs to, and the name of its inline policy.
const USER_NAME: &str = "stream-reader";
const POLICY_NAME: &str = "inline";

/// The keyspace and table whose stream the rows read, and a second table of the same
/// keyspace, so a grant scoped to one table can be shown not to cover the other.
const KEYSPACE: &str = "shop";
const TABLE: &str = "orders";
const OTHER_TABLE: &str = "carts";

/// Every operation this service binds, from `smithy_query(query="list_operations")`. The
/// rule covers all of them, so the table is the whole surface rather than a sample.
const OPERATIONS: &[&str] = &["GetRecords", "GetShardIterator", "GetStream", "ListStreams"];

/// A router serving this service beside the IAM service, over ONE shared engine, so the
/// policies the cases seed are the ones that service holds.
fn router_with_identity() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// What one call answered: its HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code an awsJson1_0 error envelope carries.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The refusal's `message`, or the empty string when the answer carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Whether this answer is the modelled authorization refusal.
    fn refused(&self) -> bool {
        self.status == 403 && self.error_code() == "AccessDeniedException"
    }
}

/// Call one operation of this service signed with `access_key_id` — what names the
/// principal the request speaks for.
async fn call_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    input: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://cassandra-streams.{REGION}.amazonaws.com/"
        ))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("KeyspacesStreams.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20261030/{REGION}/{ACTION_PREFIX}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(input.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// The ARN the account holds the IAM user under, which a refusal names as the caller.
fn user_arn() -> String {
    format!(
        "arn:aws:iam::{}:user/{USER_NAME}",
        ::engine::emulator_account_id()
    )
}

/// Record the IAM user [`USER_KEY_ID`] belongs to, as `iam:CreateUser` and
/// `iam:CreateAccessKey` record one: the user, and the key record that maps the access key
/// id to it. NO policy is attached.
fn given_user_key(engine: &Arc<engine::InMemoryEngine>) {
    let minted = ::aws_smithy_types::DateTime::from_secs(0);
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.users.insert(
        iam::name_key(USER_NAME),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::User {
            path: "/".to_string(),
            user_name: USER_NAME.to_string(),
            user_id: "AIDAEXAMPLESTREAMREADER1".to_string(),
            arn: user_arn(),
            create_date: minted,
            password_last_used: None,
            permissions_boundary: None,
            tags: None,
        },
    );
    store.access_keys.insert(
        USER_KEY_ID.to_string(),
        iam::AccessKeyRecord {
            user_key: iam::name_key(USER_NAME),
            status: iam::ACCESS_KEY_ACTIVE_STATUS.to_string(),
            create_date: minted,
        },
    );
}

/// Attach `document` to the user as its inline policy, replacing whatever it carried —
/// what `iam:PutUserPolicy` does. `None` takes every policy off it, as
/// `iam:DeleteUserPolicy` does.
fn given_user_policy(engine: &Arc<engine::InMemoryEngine>, document: Option<&Value>) {
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store.user_inline_policies.remove(&iam::name_key(USER_NAME));
    if let Some(document) = document {
        store.user_inline_policies.insert(
            iam::name_key(USER_NAME),
            BTreeMap::from([(
                iam::name_key(POLICY_NAME),
                iam::InlinePolicy {
                    policy_name: POLICY_NAME.to_string(),
                    policy_document: document.to_string(),
                },
            )]),
        );
    }
}

/// One captured change to a row, as the table's write path records it.
fn change(key: &str) -> ChangeRecord {
    let mut partition_keys = BTreeMap::new();
    partition_keys.insert(
        "id".to_string(),
        ::service_com_amazonaws_keyspacesstreams_keyspacesstreams::ops::types::KeyspacesCellValue::textT(
            key.to_string(),
        ),
    );
    ChangeRecord {
        origin: "USER".to_string(),
        partition_keys,
        clustering_keys: BTreeMap::new(),
        new_image: None,
        old_image: None,
    }
}

/// How many records the seeded stream captured, so a served read can be told from an
/// empty one.
const SEEDED_RECORDS: usize = 2;

/// What a case addresses: a stream with records, its shard, and an iterator at the start
/// of that shard.
///
/// Both the stream and the iterator are seeded through `crate::state`'s write side (the
/// Keyspaces control plane's seam, and the registry `GetShardIterator` mints into), so
/// setting a case up needs no grant of its own and the rows below exercise only the gate.
struct Addressed {
    stream_arn: String,
    shard_id: String,
    iterator: String,
}

/// Enable CDC for one table, capture [`SEEDED_RECORDS`] changes on it and register an
/// iterator at the start of its shard.
fn given_stream(engine: &Arc<engine::InMemoryEngine>, table_name: &str) -> Addressed {
    let state = service_state(&**engine).expect("this service's store");
    let mut state = state.lock();
    let stream_arn = state
        .enable_stream(REGION, KEYSPACE, table_name, "NEW_AND_OLD_IMAGES")
        .expect("a modelled view type is accepted");
    for index in 0..SEEDED_RECORDS {
        assert!(
            state
                .append_change_record(&stream_arn, change(&format!("r{index}")))
                .is_some(),
            "seeding {KEYSPACE}.{table_name}: record {index} must be captured"
        );
    }
    let shard_id = state
        .stream(&stream_arn)
        .expect("the stream just enabled")
        .shards
        .first()
        .expect("a freshly enabled stream has one shard")
        .shard_id
        .clone();
    let iterator = state.mint_shard_iterator(&stream_arn, &shard_id, 0);
    Addressed {
        stream_arn,
        shard_id,
        iterator,
    }
}

/// The request body each operation is called with, so every row sends a request the
/// operation would otherwise serve.
fn request_for(operation: &str, addressed: &Addressed) -> Value {
    match operation {
        "GetRecords" => json!({ "shardIterator": addressed.iterator }),
        "GetShardIterator" => json!({
            "streamArn": addressed.stream_arn,
            "shardId": addressed.shard_id,
            "shardIteratorType": "TRIM_HORIZON",
        }),
        "GetStream" => json!({ "streamArn": addressed.stream_arn }),
        "ListStreams" => json!({ "keyspaceName": KEYSPACE, "tableName": TABLE }),
        other => panic!("case {other:?}: no request is defined for this operation"),
    }
}

/// The resource ARN a request to `operation` is judged against: the stream it addresses,
/// or — for the enumeration, which addresses no single stream — the stream scope its
/// filters select.
fn resource_for(operation: &str, addressed: &Addressed) -> String {
    match operation {
        "ListStreams" => stream_scope_arn(KEYSPACE, TABLE),
        _ => addressed.stream_arn.clone(),
    }
}

/// The ARN of every stream of one table: the resource field Amazon Keyspaces gives a
/// table's streams (`/keyspace/<ks>/table/<tbl>/stream/<label>`) with the label left open,
/// which is how a caller scopes a CDC read policy to a table.
fn stream_scope_arn(keyspace_name: &str, table_name: &str) -> String {
    ::engine::emulator_arn(
        ACTION_PREFIX,
        REGION,
        &format!("/keyspace/{keyspace_name}/table/{table_name}/stream/*"),
    )
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }]
    })
}

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// The documents a row puts in force on the user, and which of IAM's refusals the request
/// then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(
    action: &str,
    other: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!(other))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("dynamodb:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of exactly this action beside an allow of it",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": action, "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": format!("{ACTION_PREFIX}:*"), "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action",
            Some(allow(json!(action))),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow
            // is not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["dynamodb:GetRecords", action]))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!(format!("{ACTION_PREFIX}:*")))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]
}

/// THE RULE: every operation is judged against its OWN `cassandra:<OperationName>` action
/// on the stream the request addresses, before it serves anything; a refusal is HTTP 403
/// `AccessDeniedException` naming the principal, the action, the resource and which of
/// IAM's rules refused it, and an allowed call reaches the operation's own answer.
#[tokio::test]
async fn every_operation_is_authorized_against_its_own_action() {
    for operation in OPERATIONS {
        let action = format!("{ACTION_PREFIX}:{operation}");
        // Another action of this service, which a row grants instead of the one under
        // test.
        let other = OPERATIONS
            .iter()
            .map(|row| format!("{ACTION_PREFIX}:{row}"))
            .find(|candidate| *candidate != action)
            .expect("this service binds more than one operation");

        for (row, document, refused) in documents_in_force(&action, &other) {
            let case = format!("{operation} with {row}");
            // A fresh store, user and stream per row: no row can pass on another's write.
            let (router, engine) = router_with_identity();
            given_user_key(&engine);
            given_user_policy(&engine, document.as_ref());
            let addressed = given_stream(&engine, TABLE);

            let answer = call_as(
                &router,
                USER_KEY_ID,
                operation,
                &request_for(operation, &addressed),
            )
            .await;
            match refused {
                Some(refusal) => {
                    assert!(
                        answer.refused(),
                        "case {case:?}: expected HTTP 403 AccessDeniedException, got {} {:?}",
                        answer.status,
                        answer.body
                    );
                    assert_eq!(
                        answer.message(),
                        format!(
                            "User: {} is not authorized to perform: {action} on resource: {} {}",
                            user_arn(),
                            resource_for(operation, &addressed),
                            refusal.reason(&action)
                        ),
                        "case {case:?}: expected IAM's wording, got {:?}",
                        answer.body
                    );
                    assert!(
                        answer.body.get("changeRecords").is_none()
                            && answer.body.get("shardIterator").is_none()
                            && answer.body.get("streams").is_none(),
                        "case {case:?}: a refused call reports nothing of the stream, got {:?}",
                        answer.body
                    );
                }
                None => {
                    // Authorization answers BEFORE anything is read, so an allowed call
                    // falls through to the operation's own verdict — and never to another
                    // authorization decision.
                    assert!(
                        !answer.refused(),
                        "case {case:?}: an allowed request must not be refused, got {} {:?}",
                        answer.status,
                        answer.body
                    );
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: an allowed request reaches the operation's own answer, \
                         got {} {:?}",
                        answer.status, answer.body
                    );
                }
            }
        }
    }
}

/// RULE: the refusal is about AUTHORIZATION and nothing else — the same principal, the
/// same request and the same stream state, once the action is granted, is served the
/// records the stream captured.
#[tokio::test]
async fn granting_the_action_serves_the_very_request_that_was_refused() {
    let (router, engine) = router_with_identity();
    given_user_key(&engine);
    let addressed = given_stream(&engine, TABLE);
    let request = request_for("GetRecords", &addressed);

    // Allowed everything of this service EXCEPT the record read, which is the
    // least-privilege boundary a consumer of this stream is held to.
    given_user_policy(
        &engine,
        Some(&allow(json!([
            format!("{ACTION_PREFIX}:GetStream"),
            format!("{ACTION_PREFIX}:GetShardIterator"),
        ]))),
    );
    let refused = call_as(&router, USER_KEY_ID, "GetRecords", &request).await;
    assert!(
        refused.refused(),
        "case \"GetRecords not granted\": expected HTTP 403 AccessDeniedException, got {} {:?}",
        refused.status,
        refused.body
    );
    assert!(
        refused.body.get("changeRecords").is_none(),
        "case \"GetRecords not granted\": a refused read carries no change record, got {:?}",
        refused.body
    );
    // The other two actions the same document grants are served, so the refusal above is
    // about the action and not about the credential.
    for granted in ["GetStream", "GetShardIterator"] {
        let served = call_as(
            &router,
            USER_KEY_ID,
            granted,
            &request_for(granted, &addressed),
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {granted:?}: the action this document grants is served, got {} {:?}",
            served.status, served.body
        );
    }

    given_user_policy(
        &engine,
        Some(&allow(json!(format!("{ACTION_PREFIX}:GetRecords")))),
    );
    let served = call_as(&router, USER_KEY_ID, "GetRecords", &request).await;
    assert_eq!(
        served.status, 200,
        "case \"GetRecords granted\": the identical call must be served, got {} {:?}",
        served.status, served.body
    );
    assert_eq!(
        served.body["changeRecords"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        SEEDED_RECORDS,
        "case \"GetRecords granted\": the read serves what the stream captured, got {:?}",
        served.body
    );
}

/// The `Resource` element a row scopes its allow to, as a caller writes it in a policy.
#[derive(Debug, Clone, Copy)]
enum Scope {
    /// `"*"`: every resource.
    Everything,
    /// Every stream of one table of [`KEYSPACE`], in `region`.
    TableStreams {
        table_name: &'static str,
        region: &'static str,
    },
    /// Exactly the stream the request addresses, by its own ARN.
    OneStream,
}

impl Scope {
    /// The `Resource` value this scope renders to for the stream a case addresses.
    fn resource(self, addressed: &Addressed) -> String {
        match self {
            Self::Everything => "*".to_string(),
            Self::TableStreams { table_name, region } => ::engine::emulator_arn(
                ACTION_PREFIX,
                region,
                &format!("/keyspace/{KEYSPACE}/table/{table_name}/stream/*"),
            ),
            Self::OneStream => addressed.stream_arn.clone(),
        }
    }
}

/// RULE: a request is judged against the STREAM it addresses, so an allow scoped to one
/// table's streams covers reads of that table's stream and no other — not another table of
/// the same keyspace, and not the same table in another Region.
#[tokio::test]
async fn an_action_is_judged_against_the_stream_the_request_addresses() {
    // (case, the Resource an allow is scoped to, which operations it then covers)
    let rows: Vec<(&str, Scope, Vec<&str>)> = vec![
        ("every resource", Scope::Everything, OPERATIONS.to_vec()),
        (
            "this table's streams",
            Scope::TableStreams {
                table_name: TABLE,
                region: REGION,
            },
            OPERATIONS.to_vec(),
        ),
        (
            // The enumeration addresses the table's whole stream scope rather than one
            // stream, so a grant naming a single stream does not cover it.
            "exactly one stream's own ARN",
            Scope::OneStream,
            vec!["GetRecords", "GetShardIterator", "GetStream"],
        ),
        (
            "another table of the same keyspace",
            Scope::TableStreams {
                table_name: OTHER_TABLE,
                region: REGION,
            },
            Vec::new(),
        ),
        (
            "this table's streams in another Region",
            Scope::TableStreams {
                table_name: TABLE,
                region: "eu-west-1",
            },
            Vec::new(),
        ),
    ];

    for (case, scope, covered) in rows {
        for operation in OPERATIONS {
            let (router, engine) = router_with_identity();
            given_user_key(&engine);
            let addressed = given_stream(&engine, TABLE);
            given_user_policy(
                &engine,
                Some(&json!({
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": format!("{ACTION_PREFIX}:*"),
                        "Resource": scope.resource(&addressed),
                    }],
                })),
            );

            let answer = call_as(
                &router,
                USER_KEY_ID,
                operation,
                &request_for(operation, &addressed),
            )
            .await;
            let allowed = covered.contains(operation);
            assert_eq!(
                answer.refused(),
                !allowed,
                "case {case:?}/{operation}: an allow scoped to that resource must {}cover this \
                 operation, got {} {:?}",
                if allowed { "" } else { "not " },
                answer.status,
                answer.body
            );
        }
    }
}

/// RULE: the account's own published credential — a key neither STS nor IAM holds a record
/// for — is not a principal the IAM store holds policies for, so it is served whatever the
/// user's policies say.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    for operation in OPERATIONS {
        let (router, engine) = router_with_identity();
        given_user_key(&engine);
        // The user carries nothing at all, which is what refuses it above.
        given_user_policy(&engine, None);
        let addressed = given_stream(&engine, TABLE);

        let answer = call_as(
            &router,
            ACCOUNT_KEY_ID,
            operation,
            &request_for(operation, &addressed),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {operation:?}: the account's own credential must be served, got {} {:?}",
            answer.status, answer.body
        );
    }
}
