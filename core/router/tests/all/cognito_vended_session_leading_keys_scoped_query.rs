// SPDX-License-Identifier: CC0-1.0

//! A Cognito-vended session acting on a DynamoDB partition: the federated identity the
//! session stands for is what decides the call.
//!
//! RULE: a role session minted by exchanging a web identity token carries the identity that
//! token established, so a request the vended credential signs is decided against the same
//! `<provider>:<claim>` condition keys the exchange was — the subject, the audience and the
//! authentication methods the token names — and a `${<provider>:<claim>}` policy variable in
//! a statement of the role's own policy stands for the value THIS session carries for it.
//! A statement whose `Condition` the request then fails grants nothing, so the call is
//! refused with `AccessDeniedException` (HTTP 400) worded from the request: the session
//! principal, the action and the table's ARN. A key the exchange published no value for
//! cannot be substituted or compared at all, which leaves the statement carrying it
//! undecided, and an undecided statement never grants.
//!
//! Four services have to agree for that to hold — Cognito Identity resolves the pool's role
//! and makes the exchange, IAM holds the role and its policy, STS records the session and
//! names its principal, and DynamoDB makes the decision — so this is the suite that can pin
//! it; a crate-local test reaches only one of them.
//!
//! The rows span what the rule generalises over: WHICH claim of the exchange the statement
//! is written over (the subject, the audience, a key the exchange publishes no value for),
//! whether the claim appears as a policy VARIABLE or as a literal, which set qualifier the
//! clause carries (`ForAllValues:`, `ForAnyValue:`, none at all), and which partition the
//! call addresses (the session's own federated subject, or another identity's). Each row is
//! driven through three data-plane operations — a `Query` that selects its partition through
//! the key condition, and a `GetItem` and a `PutItem` that address theirs through a key
//! map — because the rule is about the partition a call addresses, however it names it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The region every request here signs for.
const REGION: &str = "us-east-1";

/// The table every row acts on, keyed on the identity id that owns each row.
const TABLE: &str = "PerIdentityRows";

/// The partition-key attribute: the identity id a row belongs to, which is what a
/// fine-grained statement scopes a session to.
const PARTITION_ATTRIBUTE: &str = "UserId";

/// The sort-key attribute, so a key map names two attributes and a partition can hold
/// several rows.
const SORT_ATTRIBUTE: &str = "ItemId";

/// The sort-key value of the row seeded in each identity's partition.
const SEEDED_ROW: &str = "row-1";

/// The long-term key every emulator publishes, which is the ambient identity's — what every
/// SETUP call here is signed with, and never what a graded call is.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The role-session name an identity pool's own exchange carries, which is the last segment
/// of the assumed-role ARN a refusal names
/// (`service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::VENDED_ROLE_SESSION_NAME`).
const VENDED_SESSION_NAME: &str = "CognitoIdentityCredentials";

/// The identity-provider principal an identity pool's exchange is made under.
const COGNITO_IDENTITY_PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// How long a poll loop waits for a state the clock owes. Generous: what is under test is
/// the decision, never how fast the host reaches a transition.
const BUDGET: Duration = Duration::from_secs(60);

/// How long a poll loop waits before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The four services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Every suite in this test crate runs with request authentication off: a signed row
    // would otherwise need a real signature. What a graded call pins is the DECISION made
    // for the session's access key id, never signature verification.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key a service reads the caller's identity from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one awsQuery form body to a service as the ambient identity.
async fn form(router: &MultiServiceRouter, signing_name: &str, body: String) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(AMBIENT_KEY, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Send one JSON-RPC call to `signing_name`, targeted at `target`, signed with
/// `access_key_id`, and answer `(status, parsed body)`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    content_type: &str,
    access_key_id: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the test body serializes"),
        ))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One Amazon Cognito Identity call, signed with the ambient key.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        router,
        "cognito-identity",
        "application/x-amz-json-1.1",
        AMBIENT_KEY,
        &format!("AWSCognitoIdentityService.{operation}"),
        body,
    )
    .await
}

/// One DynamoDB call, signed with `access_key_id`.
async fn ddb(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    rpc(
        router,
        "dynamodb",
        "application/x-amz-json-1.0",
        access_key_id,
        &format!("DynamoDB_20120810.{operation}"),
        body,
    )
    .await
}

/// The wire error code (`__type`) of a refusal.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message`/`Message` member of a refusal, whichever casing it carries.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The table's ARN, which is the resource every statement here names.
fn table_arn() -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{TABLE}")
}

/// Create the table every row acts on, as the account's own credential.
async fn create_table(router: &MultiServiceRouter) {
    let (status, created) = ddb(
        router,
        AMBIENT_KEY,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": PARTITION_ATTRIBUTE, "KeyType": "HASH"},
                {"AttributeName": SORT_ATTRIBUTE, "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": PARTITION_ATTRIBUTE, "AttributeType": "S"},
                {"AttributeName": SORT_ATTRIBUTE, "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateTable expected 200, got {status} {created}"
    );
}

/// Wait until DynamoDB first reports the table `ACTIVE`: "you can perform read and write
/// operations only on an ACTIVE table".
async fn await_table_active(router: &MultiServiceRouter) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (_, described) = ddb(
            router,
            AMBIENT_KEY,
            "DescribeTable",
            &json!({"TableName": TABLE}),
        )
        .await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: DynamoDB never reported {TABLE} ACTIVE, last read {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Seed one row in `user`'s partition, as the account's own credential.
async fn seed_row(router: &MultiServiceRouter, user: &str) {
    let (status, put) = ddb(
        router,
        AMBIENT_KEY,
        "PutItem",
        &json!({
            "TableName": TABLE,
            "Item": {
                PARTITION_ATTRIBUTE: {"S": user},
                SORT_ATTRIBUTE: {"S": SEEDED_ROW},
                "payload": {"S": "the row that belongs to this identity"},
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: PutItem for {user:?} expected 200, got {status} {put}"
    );
}

/// `cognito-identity:CreateIdentityPool`, answering the pool's id.
async fn create_identity_pool(router: &MultiServiceRouter, pool_name: &str) -> String {
    let (status, pool) = cognito(
        router,
        "CreateIdentityPool",
        &json!({"IdentityPoolName": pool_name, "AllowUnauthenticatedIdentities": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateIdentityPool({pool_name:?}) expected 200, got {status} {pool}"
    );
    pool["IdentityPoolId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: CreateIdentityPool reports no IdentityPoolId: {pool}"))
        .to_string()
}

/// `cognito-identity:GetId`, answering the id of a fresh guest identity of `pool_id`.
async fn get_id(router: &MultiServiceRouter, pool_id: &str) -> String {
    let (status, identity) = cognito(router, "GetId", &json!({"IdentityPoolId": pool_id})).await;
    assert_eq!(
        status, 200,
        "setup: GetId({pool_id:?}) expected 200, got {status} {identity}"
    );
    identity["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: GetId reports no IdentityId: {identity}"))
        .to_string()
}

/// The trust policy of the role an identity pool vends sessions of, written the way the
/// Amazon Cognito documentation writes one: the `Federated` Cognito Identity principal, the
/// web-identity assume, and the `:aud` condition that restricts the assume to the users of
/// that one pool.
///
/// Compact and carrying none of the characters that are significant in an
/// `application/x-www-form-urlencoded` body (`&`, `=`, `+`, `%`, space), so it needs no
/// encoding.
fn pool_trust_policy(pool_id: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Federated\":\"{COGNITO_IDENTITY_PRINCIPAL}\"}},\
         \"Action\":\"sts:AssumeRoleWithWebIdentity\",\"Condition\":{{\"StringEquals\":\
         {{\"{COGNITO_IDENTITY_PRINCIPAL}:aud\":\"{pool_id}\"}}}}}}]}}"
    )
}

/// The role's own identity-based policy: the three data-plane actions each row drives, on
/// the table, under the row's `Condition`.
///
/// Compact and free of the characters a form body treats specially, exactly like
/// [`pool_trust_policy`].
fn role_policy(condition: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":[\"dynamodb:Query\",\"dynamodb:GetItem\",\"dynamodb:PutItem\"],\
         \"Resource\":\"{}\",\"Condition\":{condition}}}]}}",
        table_arn()
    )
}

/// `iam:CreateRole` for a role the pool `pool_id` vends sessions of, with `document` as its
/// inline policy; answers the role's ARN.
async fn create_pool_role(
    router: &MultiServiceRouter,
    role_name: &str,
    pool_id: &str,
    document: &str,
) -> String {
    let (status, created) = form(
        router,
        "iam",
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
             &AssumeRolePolicyDocument={}",
            pool_trust_policy(pool_id)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole({role_name:?}) expected 200, got {status} {created}"
    );
    let (status, attached) = form(
        router,
        "iam",
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={role_name}\
             &PolicyName=per-identity-rows&PolicyDocument={document}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: PutRolePolicy({role_name:?}) expected 200, got {status} {attached}"
    );
    xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup: CreateRole reports no <Arn>: {created}"))
}

/// `cognito-identity:SetIdentityPoolRoles` — the role a guest exchange of `pool_id` vends a
/// session of.
async fn set_unauthenticated_role(router: &MultiServiceRouter, pool_id: &str, role_arn: &str) {
    let (status, configured) = cognito(
        router,
        "SetIdentityPoolRoles",
        &json!({"IdentityPoolId": pool_id, "Roles": {"unauthenticated": role_arn}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: SetIdentityPoolRoles expected 200, got {status} {configured}"
    );
}

/// `cognito-identity:GetCredentialsForIdentity` for `identity_id`, answering the access key
/// id of the session the pool vended — the credential every graded call below signs with.
async fn vend_session(router: &MultiServiceRouter, identity_id: &str) -> String {
    let (status, vended) = cognito(
        router,
        "GetCredentialsForIdentity",
        &json!({"IdentityId": identity_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: GetCredentialsForIdentity({identity_id:?}) expected 200, got {status} {vended}"
    );
    // A client cannot sign with a session that carries no token either, so presence alone is
    // asserted here — no secret reaches an assertion message.
    for member in ["SecretKey", "SessionToken"] {
        assert!(
            !vended["Credentials"][member]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "setup: the vended session must carry {member:?}, got the member absent or empty"
        );
    }
    vended["Credentials"]["AccessKeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the vend reports no AccessKeyId: {vended}"))
        .to_string()
}

/// One operation a row is driven through: the IAM action it performs, and the request body
/// that addresses `user`'s partition with it.
struct Driven {
    action: &'static str,
    request: Value,
}

/// The three calls each row drives against `user`'s partition: a `Query` naming the
/// partition in its key condition, and a `GetItem` and a `PutItem` naming it in a key map.
fn driven(user: &str) -> Vec<Driven> {
    vec![
        Driven {
            action: "dynamodb:Query",
            request: json!({
                "TableName": TABLE,
                "KeyConditionExpression": "#p = :u",
                "ExpressionAttributeNames": {"#p": PARTITION_ATTRIBUTE},
                "ExpressionAttributeValues": {":u": {"S": user}},
            }),
        },
        Driven {
            action: "dynamodb:GetItem",
            request: json!({
                "TableName": TABLE,
                "Key": {
                    PARTITION_ATTRIBUTE: {"S": user},
                    SORT_ATTRIBUTE: {"S": SEEDED_ROW},
                },
            }),
        },
        Driven {
            action: "dynamodb:PutItem",
            request: json!({
                "TableName": TABLE,
                "Item": {
                    PARTITION_ATTRIBUTE: {"S": user},
                    SORT_ATTRIBUTE: {"S": "written-by-the-session"},
                },
            }),
        },
    ]
}

/// The operation name the wire target carries for an IAM action of this service.
fn operation_of(action: &str) -> &str {
    action
        .split_once(':')
        .expect("every action here is prefixed")
        .1
}

/// One row: the `Condition` the role's statement carries, and whether a call addressing the
/// session's own partition and one addressing another identity's are then granted.
struct Row {
    case: &'static str,
    /// Built from the pool the session was vended by and the identity it was vended to, so a
    /// row may pin a claim to a literal as well as to a policy variable.
    condition: fn(&str, &str) -> String,
    own_granted: bool,
    other_granted: bool,
}

/// The table. Each row differs along what the rule generalises over: which claim of the
/// exchange the clause is written over, whether it appears as a variable or a literal, and
/// which set qualifier the clause carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The documented fine-grained pattern: the partitions a call addresses, pinned
            // to the subject of the token the session was minted by exchanging.
            case: "ForAllValues:StringEquals over the leading keys, pinned to the subject \
                   variable",
            condition: |_pool, _identity| {
                format!(
                    "{{\"ForAllValues:StringEquals\":{{\"dynamodb:LeadingKeys\":\
                     [\"${{{COGNITO_IDENTITY_PRINCIPAL}:sub}}\"]}}}}"
                )
            },
            own_granted: true,
            other_granted: false,
        },
        Row {
            case: "the same clause with no set qualifier and a single value",
            condition: |_pool, _identity| {
                format!(
                    "{{\"StringEquals\":{{\"dynamodb:LeadingKeys\":\
                     \"${{{COGNITO_IDENTITY_PRINCIPAL}:sub}}\"}}}}"
                )
            },
            own_granted: true,
            other_granted: false,
        },
        Row {
            case: "ForAnyValue:StringEquals over the leading keys, pinned to the subject \
                   variable",
            condition: |_pool, _identity| {
                format!(
                    "{{\"ForAnyValue:StringEquals\":{{\"dynamodb:LeadingKeys\":\
                     [\"${{{COGNITO_IDENTITY_PRINCIPAL}:sub}}\"]}}}}"
                )
            },
            own_granted: true,
            other_granted: false,
        },
        Row {
            // The claim read directly rather than through a variable: the clause is about
            // WHOSE session this is, not about the partition, so it decides both calls alike.
            case: "the subject claim compared to the identity the session was vended to",
            condition: |_pool, identity| {
                format!("{{\"StringEquals\":{{\"{COGNITO_IDENTITY_PRINCIPAL}:sub\":\"{identity}\"}}}}")
            },
            own_granted: true,
            other_granted: true,
        },
        Row {
            case: "the subject claim compared to another identity than this session's",
            condition: |_pool, identity| {
                // A subject of the same shape the pool mints, and not this session's.
                let other = format!("{identity}x");
                format!("{{\"StringEquals\":{{\"{COGNITO_IDENTITY_PRINCIPAL}:sub\":\"{other}\"}}}}")
            },
            own_granted: false,
            other_granted: false,
        },
        Row {
            // Another claim of the same exchange: the audience is the pool the session was
            // vended by, so the whole claim set travels with the session, not the subject
            // alone.
            case: "the audience claim compared to the pool the session was vended by",
            condition: |pool, _identity| {
                format!("{{\"StringEquals\":{{\"{COGNITO_IDENTITY_PRINCIPAL}:aud\":\"{pool}\"}}}}")
            },
            own_granted: true,
            other_granted: true,
        },
        Row {
            // A key the exchange publishes no value for: nothing substitutes it and nothing
            // compares it, so the statement stays undecided and grants nothing.
            case: "a claim of the same provider the exchange publishes no value for",
            condition: |_pool, _identity| {
                format!(
                    "{{\"ForAllValues:StringEquals\":{{\"dynamodb:LeadingKeys\":\
                     [\"${{{COGNITO_IDENTITY_PRINCIPAL}:nosuchclaim}}\"]}}}}"
                )
            },
            own_granted: false,
            other_granted: false,
        },
    ]
}

#[tokio::test]
async fn a_vended_session_acts_only_where_its_federated_claims_satisfy_the_roles_policy() {
    let router = build_router();
    create_table(&router).await;
    await_table_active(&router).await;

    for (position, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            condition,
            own_granted,
            other_granted,
        } = row;
        // A pool, two guest identities of it, and a row in each identity's partition. Fresh
        // per row, so the only thing that differs between rows is the role's own statement.
        let pool_name = format!("per-identity-pool-{position}");
        let pool_id = create_identity_pool(&router, &pool_name).await;
        let own = get_id(&router, &pool_id).await;
        let other = get_id(&router, &pool_id).await;
        assert_ne!(
            own, other,
            "case {case:?}: two GetId calls must mint two distinct identities, got {own:?} \
             twice"
        );
        seed_row(&router, &own).await;
        seed_row(&router, &other).await;

        let role_name = format!("per-identity-role-{position}");
        let role_arn = create_pool_role(
            &router,
            &role_name,
            &pool_id,
            &role_policy(&condition(&pool_id, &own)),
        )
        .await;
        set_unauthenticated_role(&router, &pool_id, &role_arn).await;
        let access_key_id = vend_session(&router, &own).await;
        let session_arn =
            format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/{VENDED_SESSION_NAME}");

        // (which partition, its key, whether the statement grants a call addressing it)
        let addressed = [
            ("its own partition", own.clone(), own_granted),
            ("another identity's partition", other.clone(), other_granted),
        ];
        for (whose, user, granted) in addressed {
            for call in driven(&user) {
                let operation = operation_of(call.action);
                let (status, answer) = ddb(&router, &access_key_id, operation, &call.request).await;
                let expected = if granted { 200 } else { 400 };
                assert_eq!(
                    status, expected,
                    "case {case:?}: {operation} on {whose} expected HTTP {expected}, got \
                     {status} {answer}"
                );
                if granted {
                    continue;
                }
                assert_eq!(
                    error_code(&answer),
                    "AccessDeniedException",
                    "case {case:?}: {operation} on {whose} expected AccessDeniedException, got \
                     {answer}"
                );
                let message = message_of(&answer);
                let wanted = format!(
                    "User: {session_arn} is not authorized to perform: {} on resource: {} \
                     because no identity-based policy allows the {} action",
                    call.action,
                    table_arn(),
                    call.action
                );
                assert_eq!(
                    message, wanted,
                    "case {case:?}: {operation} on {whose} expected the refusal {wanted:?}, got \
                     {message:?}"
                );
            }
        }

        // Whatever the row says, the session reads its OWN partition's seeded row when the
        // statement grants the read, and no row of the other identity's partition reaches it
        // when the statement does not.
        if own_granted {
            let (status, queried) = ddb(
                &router,
                &access_key_id,
                "Query",
                &driven(&own)[0].request.clone(),
            )
            .await;
            assert_eq!(
                (status, queried["Items"].as_array().map(Vec::len)),
                (200, Some(2)),
                "case {case:?}: the session's own partition holds the seeded row and the one \
                 its PutItem added, got {status} {queried}"
            );
            for item in queried["Items"].as_array().into_iter().flatten() {
                assert_eq!(
                    item[PARTITION_ATTRIBUTE],
                    json!({"S": &own}),
                    "case {case:?}: every row the session reads belongs to its own identity, \
                     got {item}"
                );
            }
        }
        if !other_granted {
            // The other identity's partition still holds exactly what the account seeded: a
            // refused write left nothing behind.
            let (status, queried) =
                ddb(&router, AMBIENT_KEY, "Query", &driven(&other)[0].request).await;
            assert_eq!(
                (status, queried["Items"].as_array().map(Vec::len)),
                (200, Some(1)),
                "case {case:?}: the refused calls must leave the other identity's partition \
                 as the account seeded it, got {status} {queried}"
            );
        }
    }
}
