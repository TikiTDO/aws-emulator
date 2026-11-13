// SPDX-License-Identifier: CC0-1.0

//! WHICH sentence a client-secret lookup that resolves nothing answers, over the wire
//! (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.DeleteUserPoolClientSecret`).
//!
//! The rule under test: a `ClientSecretId` the named app client holds no secret for is
//! refused with `ResourceNotFoundException`, at HTTP 400 and naming that shape in
//! `x-amzn-errortype`, carrying ONE fixed sentence chosen by WHAT the lookup failed to
//! resolve — an app client that holds no secret at all has no secret of its own to name,
//! and the refusal reports the app client's secret as absent, while an app client that
//! does hold secrets has the ONE the request named reported absent. Both sentences are
//! fixed text: neither quotes the id asked for, the app client (which the model marks
//! `@sensitive`) nor the pool, so one app client's refusals read the same whatever id a
//! caller asks for.
//!
//! The rows differ along what the rule generalises over: whether the named app client
//! holds a secret at all, and where the secret it holds came from (`GenerateSecret`, a
//! value the create supplied, an add after a delete) — and, for each of those clients, the
//! FORM of the unresolvable id (hex, UUID, the `<client-id>--<epoch-millis>` form an add
//! mints, each published bound of `ClientSecretIdType`'s length 1..128) and WHY it resolves
//! nothing (never minted, minted for an app client of another pool, minted then deleted).
//! Every assertion names its case and reports the value it saw.
//!
//! The harness is `super::error_wire`'s, so this suite drives the same router and the same
//! one call as the sibling suites asserting this surface's wire rules.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::error_wire::{call, error_code, ok, router, ERRORTYPE, REFUSED};

/// The sentence a lookup answers when the app client holds no secret at all, so the app
/// client's secret itself is what does not exist.
const NO_SECRET_OF_THE_CLIENT: &str = "User pool client secret does not exist.";

/// The sentence a lookup answers when the app client does hold secrets, so the ONE the
/// request named is what does not exist.
const NO_SUCH_SECRET: &str = "Client secret does not exist";

/// The error class every one of those lookups is refused with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The length bounds of `ClientSecretIdType`
/// (`smithy_query(query="shape",shape="ClientSecretIdType")`: length 1..128), so an
/// unresolvable id is asked for at each published bound.
const SECRET_ID_LENGTH_BOUNDS: [usize; 2] = [1, 128];

/// A client secret value within `ClientSecretType`'s length 24..64
/// (`smithy_query(query="operation",operation="CreateUserPoolClient")`), for the row whose
/// app client was created holding a secret the request supplied.
const SUPPLIED_SECRET: &str = "suppliedclientsecret00000001";

/// The `message` member of an error body.
fn message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// What secret an app client's create gave it.
#[derive(Debug, Clone, Copy)]
enum Created {
    /// A PUBLIC app client: no `GenerateSecret`, no `ClientSecret`, so it holds none.
    Public,
    /// `GenerateSecret: true`, so the create minted the client's first secret.
    Generated,
    /// The `ClientSecret` value the create supplied.
    Supplied(&'static str),
}

/// Create an app client of `user_pool_id` as `created` says, and return its minted
/// `ClientId`.
async fn client(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    name: &str,
    created: Created,
) -> String {
    let mut body = json!({
        "UserPoolId": user_pool_id,
        "ClientName": name,
        // A create that SUPPLIES the value must not also ask for one to be generated.
        "GenerateSecret": matches!(created, Created::Generated),
    });
    if let Created::Supplied(supplied) = created {
        body["ClientSecret"] = json!(supplied);
    }
    let response = ok(router, "CreateUserPoolClient", &body).await;
    response["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// Store one more generated secret on `client_id` and return its minted `ClientSecretId`.
async fn add_secret(router: &MultiServiceRouter, user_pool_id: &str, client_id: &str) -> String {
    let added = ok(
        router,
        "AddUserPoolClientSecret",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    added["ClientSecretDescriptor"]["ClientSecretId"]
        .as_str()
        .expect("a minted ClientSecretId")
        .to_string()
}

/// The `ClientSecretId`s `client_id` holds, in the order they are reported.
async fn held_ids(router: &MultiServiceRouter, user_pool_id: &str, client_id: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListUserPoolClientSecrets",
        &json!({ "UserPoolId": user_pool_id, "ClientId": client_id }),
    )
    .await;
    listed["ClientSecrets"]
        .as_array()
        .unwrap_or_else(|| panic!("ListUserPoolClientSecrets must answer a list, got {listed}"))
        .iter()
        .map(|entry| {
            entry["ClientSecretId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Delete `client_secret_id` from `client_id`, requiring it to succeed.
async fn delete_secret(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    client_secret_id: &str,
) {
    ok(
        router,
        "DeleteUserPoolClientSecret",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "ClientSecretId": client_secret_id,
        }),
    )
    .await;
}

/// One app client under test: how it was created, how many secrets it ends up holding, and
/// the sentence its unresolvable lookups must answer.
struct ClientUnderTest {
    case: &'static str,
    client_id: String,
    held: Vec<String>,
    expected: &'static str,
}

/// One row: the app client the delete names, the id it asks for, and why that id resolves
/// to no secret of it.
struct Row {
    case: String,
    client_id: String,
    client_secret_id: String,
    expected: &'static str,
}

#[tokio::test]
async fn a_client_secret_lookup_that_resolves_nothing_names_what_is_absent() {
    let router = router();
    let user_pool_id = pool(&router, "client-secret-absence-pool").await;

    // An app client of ANOTHER pool, so an id that IS a minted secret id resolves to no
    // secret of any app client under test.
    let other_pool = pool(&router, "client-secret-absence-other-pool").await;
    let elsewhere = client(
        &router,
        &other_pool,
        "another-pools-client",
        Created::Generated,
    )
    .await;
    let elsewhere_secret = held_ids(&router, &other_pool, &elsewhere)
        .await
        .first()
        .expect("the other pool's app client holds its generated secret")
        .clone();

    // A public app client holds no secret at all.
    let public = client(&router, &user_pool_id, "holds-no-secret", Created::Public).await;
    // One created with `GenerateSecret`, holding the one secret the create minted.
    let generated = client(
        &router,
        &user_pool_id,
        "holds-a-generated-secret",
        Created::Generated,
    )
    .await;
    // One created holding the secret the create supplied.
    let supplied = client(
        &router,
        &user_pool_id,
        "holds-a-supplied-secret",
        Created::Supplied(SUPPLIED_SECRET),
    )
    .await;
    // One that rotated a secret in and the create's one out, so what it holds now came
    // from an add rather than from its create.
    let rotating = client(
        &router,
        &user_pool_id,
        "holds-a-rotated-secret",
        Created::Generated,
    )
    .await;
    let rotated_in = add_secret(&router, &user_pool_id, &rotating).await;

    // And an id that WAS a secret of `rotating`, until this delete took it away: the id of
    // a secret that no longer exists resolves nothing, and the client still holds one.
    let rotated_out = held_ids(&router, &user_pool_id, &rotating)
        .await
        .into_iter()
        .find(|id| *id != rotated_in)
        .expect("the rotating client holds the secret its create minted");
    delete_secret(&router, &user_pool_id, &rotating, &rotated_out).await;

    let clients = [
        ClientUnderTest {
            case: "a public app client, holding no secret at all",
            client_id: public.clone(),
            held: Vec::new(),
            expected: NO_SECRET_OF_THE_CLIENT,
        },
        ClientUnderTest {
            case: "an app client holding the one secret its create generated",
            client_id: generated.clone(),
            held: held_ids(&router, &user_pool_id, &generated).await,
            expected: NO_SUCH_SECRET,
        },
        ClientUnderTest {
            case: "an app client holding the one secret its create supplied",
            client_id: supplied.clone(),
            held: held_ids(&router, &user_pool_id, &supplied).await,
            expected: NO_SUCH_SECRET,
        },
        ClientUnderTest {
            case: "an app client holding a secret an add stored after a delete",
            client_id: rotating.clone(),
            held: held_ids(&router, &user_pool_id, &rotating).await,
            expected: NO_SUCH_SECRET,
        },
    ];

    let mut rows: Vec<Row> = Vec::new();
    for under_test in &clients {
        // Ids no add ever minted, in each form a caller writes one in.
        let mut unresolvable = vec![
            (
                "a 32-hex id no add minted".to_string(),
                "0123456789abcdef0123456789abcdef".to_string(),
            ),
            (
                "a UUID no add minted".to_string(),
                "0e042c11-84c3-4b28-a91a-831ab2bbb450".to_string(),
            ),
            (
                "an unminted id in the minted <client-id>--<epoch-millis> form".to_string(),
                format!("{}--1700000000000", under_test.client_id),
            ),
            (
                "the id of a secret an app client of ANOTHER pool holds".to_string(),
                elsewhere_secret.clone(),
            ),
            (
                "the id of a secret a delete took away".to_string(),
                rotated_out.clone(),
            ),
        ];
        for length in SECRET_ID_LENGTH_BOUNDS {
            unresolvable.push((
                format!("an unminted id of the bound length {length}"),
                "z".repeat(length),
            ));
        }
        for (why, client_secret_id) in unresolvable {
            // The one id `rotating` still holds is not an unresolvable id for it.
            if under_test.held.contains(&client_secret_id) {
                continue;
            }
            rows.push(Row {
                case: format!("{why}, asked of {}", under_test.case),
                client_id: under_test.client_id.clone(),
                client_secret_id,
                expected: under_test.expected,
            });
        }
    }

    for Row {
        case,
        client_id,
        client_secret_id,
        expected,
    } in &rows
    {
        let answered = call(
            &router,
            "DeleteUserPoolClientSecret",
            &json!({
                "UserPoolId": user_pool_id,
                "ClientId": client_id,
                "ClientSecretId": client_secret_id,
            }),
        )
        .await;
        assert_eq!(
            (answered.status, error_code(&answered.payload)),
            (REFUSED, NOT_FOUND),
            "case {case:?}: expected a {REFUSED} {NOT_FOUND}, got {} with {}",
            answered.status,
            answered.payload
        );
        assert_eq!(
            answered.errortype.as_deref(),
            Some(format!("{NOT_FOUND}:").as_str()),
            "case {case:?}: expected {ERRORTYPE} {NOT_FOUND:?} and a colon, got {:?}",
            answered.errortype
        );
        let answered_message = message(&answered.payload);
        assert_eq!(
            answered_message, *expected,
            "case {case:?}: expected {expected:?}, got {answered_message:?}"
        );
        for supplied_by_the_request in [client_secret_id, client_id, &user_pool_id] {
            assert!(
                !answered_message.contains(supplied_by_the_request.as_str()),
                "case {case:?}: the refusal must quote nothing the request supplied, but \
                 {answered_message:?} carries {supplied_by_the_request:?}"
            );
        }
    }

    // Not one of those refusals wrote anything: every app client still holds exactly the
    // secrets it held before them.
    for under_test in &clients {
        let held_now = held_ids(&router, &user_pool_id, &under_test.client_id).await;
        assert_eq!(
            held_now, under_test.held,
            "case {:?}: the refusals must leave the held secrets alone, expected {:?}, got {held_now:?}",
            under_test.case, under_test.held
        );
    }
    assert_eq!(
        held_ids(&router, &other_pool, &elsewhere).await,
        vec![elsewhere_secret],
        "the refusals must leave the other pool's app client's secret alone"
    );
}

/// The two sentences are DIFFERENT facts, and neither is the sentence a lookup that fails
/// earlier answers: giving an app client its first secret moves it from one to the other,
/// and an app client no create minted is reported as itself.
#[tokio::test]
async fn giving_an_app_client_a_secret_moves_it_from_one_sentence_to_the_other() {
    let router = router();
    let user_pool_id = pool(&router, "client-secret-absence-transition-pool").await;
    let public = client(&router, &user_pool_id, "starts-public", Created::Public).await;
    let unminted_id = "0123456789abcdef0123456789abcdef";

    let body = json!({
        "UserPoolId": user_pool_id,
        "ClientId": public,
        "ClientSecretId": unminted_id,
    });

    // Holding none, the app client's secret is what does not exist.
    let before = call(&router, "DeleteUserPoolClientSecret", &body).await;
    assert_eq!(
        (before.status, message(&before.payload)),
        (REFUSED, NO_SECRET_OF_THE_CLIENT),
        "a client holding no secret: expected a {REFUSED} {NO_SECRET_OF_THE_CLIENT:?}, got {} with {}",
        before.status,
        before.payload
    );

    // One add later the client holds a secret, so the SAME request now reports that one id
    // as what does not exist.
    let added = add_secret(&router, &user_pool_id, &public).await;
    let after = call(&router, "DeleteUserPoolClientSecret", &body).await;
    assert_eq!(
        (after.status, message(&after.payload)),
        (REFUSED, NO_SUCH_SECRET),
        "a client holding one secret: expected a {REFUSED} {NO_SUCH_SECRET:?}, got {} with {}",
        after.status,
        after.payload
    );
    assert_eq!(
        held_ids(&router, &user_pool_id, &public).await,
        vec![added.clone()],
        "the refusal must leave the added secret stored"
    );

    // An app client no create minted fails to resolve BEFORE any secret of it does, so it
    // is reported as itself rather than with either secret sentence.
    let unminted_client = "notanappclient1234567890ab";
    let earlier = call(
        &router,
        "DeleteUserPoolClientSecret",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": unminted_client,
            "ClientSecretId": added,
        }),
    )
    .await;
    assert_eq!(
        (earlier.status, error_code(&earlier.payload)),
        (REFUSED, NOT_FOUND),
        "an app client no create minted: expected a {REFUSED} {NOT_FOUND}, got {} with {}",
        earlier.status,
        earlier.payload
    );
    let earlier_message = message(&earlier.payload);
    assert!(
        earlier_message != NO_SECRET_OF_THE_CLIENT && earlier_message != NO_SUCH_SECRET,
        "an app client no create minted must not be reported as an absent client secret, \
         got {earlier_message:?}"
    );
}
