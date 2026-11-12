// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless collection lifecycle and the encryption/network security
//! policies beside it, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: each of these operations computes its answer from the single
//! shared store, so a create is what every later read returns, a server-derived field is
//! derived from the record's own id and region, and a delete is observable by its absence.
//! The tables below span what the rule generalises over — collection type, the enum
//! members' whole value lists, region, page bound, policy type, id/name addressing — not
//! the one request the issue quoted.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_opensearchserverless_opensearchserverless::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator mints carries (`engine::emulator_account_id`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The `X-Amz-Target` prefix from this crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "OpenSearchServerless";

/// This service's SigV4 signing name, which is what routes a request to it
/// (`smithy_query(query="service_info")` -> `sigv4Name`).
const SIGNING_NAME: &str = "aoss";

/// The HTTP status this service answers EVERY modelled client error with.
///
/// aoss speaks `awsJson1_0` (`smithy_query(query="service_info")` -> `protocols`), where
/// the status is not how a client discriminates an error — the body's `__type` is — so the
/// endpoint answers one status, 400, for every client error whatever `@httpError` its
/// shape carries (`ConflictException` 409, `ResourceNotFoundException` 404,
/// `ServiceQuotaExceededException` 402, `ValidationException` 400). The suites below
/// assert the code from the body and this status.
pub(crate) const CLIENT_ERROR: u16 = 400;

/// Long enough for the `CREATING -> ACTIVE` transition `CreateCollection` starts to fall
/// due; the emulator's span is 500 ms and nothing sleeps for it inside a handler.
const SETTLE_WAIT: Duration = Duration::from_millis(700);

/// One emulator with an empty store: the shared engine the router builds in production,
/// carrying only this service.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// [`emulator`], keeping the shared engine so a test can inspect the store directly.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise the service, not SigV4: the request carries a credential scope so
    // the router can name the target service and the region, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The access key id [`call_in`] presents, which is what `emulator_server::extract_akid`
/// reports in an audit record. A suite that reads records back signs with one of its own
/// instead ([`call_signed_as`]), so its records are told apart from a parallel sibling's.
pub(crate) const ACCESS_KEY_ID: &str = "AKIDTEST";

/// A SigV4 `Authorization` header scoped to this service in `region`, presenting `akid` as
/// the access key id.
fn authorization(region: &str, akid: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={akid}/20260101/{region}/{SIGNING_NAME}/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// Call `operation` with `body`, signed for `region` presenting `akid`; answer its HTTP
/// status, the error code when it failed, and the parsed JSON body.
pub(crate) async fn call_signed_as(
    router: &MultiServiceRouter,
    region: &str,
    akid: &str,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{SIGNING_NAME}.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization(region, akid))
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let code = shape_name(
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or(&header_code),
    );
    (status, code, parsed)
}

/// [`call_signed_as`] presenting the harness's own [`ACCESS_KEY_ID`].
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    call_signed_as(router, region, ACCESS_KEY_ID, operation, body).await
}

/// [`call_in`] in `us-east-1`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, Value) {
    call_in(router, "us-east-1", operation, body).await
}

/// The bare shape name of an awsJson error code, with any namespace or URI prefix dropped
/// (`com.amazonaws.opensearchserverless#ConflictException` -> `ConflictException`).
fn shape_name(code: &str) -> String {
    code.split('#')
        .next_back()
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string()
}

/// The member of `value` named by `path` (`"createCollectionDetail.id"`), or `Value::Null`.
pub(crate) fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment).unwrap_or(&Value::Null);
    }
    current
}

/// The string at `path`, or the empty string.
pub(crate) fn text(value: &Value, path: &str) -> String {
    at(value, path).as_str().unwrap_or_default().to_string()
}

/// True when `id` satisfies `CollectionId`'s modelled `^[a-z0-9]{3,40}$` — which is also
/// `CollectionGroupId`'s own pattern and length, so the collection-group suite shares it.
pub(crate) fn is_collection_id(id: &str) -> bool {
    (3..=40).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// True when `version` satisfies `PolicyVersion`'s modelled `@length(min: 20, max: 36)` and
/// `^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$`.
pub(crate) fn is_policy_version(version: &str) -> bool {
    if !(20..=36).contains(&version.len()) || !version.len().is_multiple_of(4) {
        return false;
    }
    let (body, padding) = match version.strip_suffix("==") {
        Some(body) => (body, 2),
        None => match version.strip_suffix('=') {
            Some(body) => (body, 1),
            None => (version, 0),
        },
    };
    let group = body.len() % 4;
    let shape_ok = match padding {
        2 => group == 2,
        1 => group == 3,
        _ => group == 0,
    };
    shape_ok
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// Create the collection `body` describes in `region`, asserting it was accepted, and
/// answer its `createCollectionDetail`.
///
/// A collection's encryption setting has to come from somewhere — the request's
/// `encryptionConfig` or an `encryption` security policy covering its name — so a row that
/// is not about encryption gets `aWSOwnedKey` here rather than repeating it everywhere.
async fn create_collection(router: &MultiServiceRouter, region: &str, body: Value) -> Value {
    let case = body.to_string();
    let mut body = body;
    if body.get("encryptionConfig").is_none() {
        body["encryptionConfig"] = json!({ "aWSOwnedKey": true });
    }
    let (status, code, answer) = call_in(router, region, "CreateCollection", body).await;
    assert_eq!(
        status, 200,
        "case CreateCollection({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "createCollectionDetail").clone()
}

/// Poll `BatchGetCollection` until `id` is no longer reported at all: the purge of a
/// `DELETING` collection is owed by the clock, so this waits for the wall clock rather
/// than for a call.
async fn await_purged(router: &MultiServiceRouter, region: &str, id: &str) {
    for _ in 0..10 {
        let (status, code, answer) =
            call_in(router, region, "BatchGetCollection", json!({ "ids": [id] })).await;
        assert_eq!(
            status, 200,
            "case BatchGetCollection({id}): expected 200, got {status} {code} with {answer}"
        );
        if at(&answer, "collectionDetails")
            .as_array()
            .is_some_and(Vec::is_empty)
        {
            return;
        }
        tokio::time::sleep(SETTLE_WAIT / 5).await;
    }
    panic!("case await_purged({id}): the collection was never purged");
}

/// Poll `BatchGetCollection` until `id` reports `ACTIVE`, answering its `CollectionDetail`.
///
/// The `CREATING -> ACTIVE` move is owed by the clock, so this waits for the wall clock
/// rather than for a call: no handler sleeps for it.
async fn await_active(router: &MultiServiceRouter, region: &str, id: &str) -> Value {
    for _ in 0..10 {
        let (status, code, answer) =
            call_in(router, region, "BatchGetCollection", json!({ "ids": [id] })).await;
        assert_eq!(
            status, 200,
            "case BatchGetCollection({id}): expected 200, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "collectionDetails")
            .as_array()
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        if text(&detail, "status") == "ACTIVE" {
            return detail;
        }
        tokio::time::sleep(SETTLE_WAIT / 5).await;
    }
    panic!("case await_active({id}): the collection never reached ACTIVE");
}

// ---------------------------------------------------------------------------
// CreateCollection: the server-derived fields, over a table that spans every
// `CollectionType`, both values of each enum member, the defaults for the members a
// request may omit, and three regions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_collection_reports_fields_derived_from_its_own_id_and_region() {
    let router = emulator();

    // `CollectionType` = SEARCH | TIMESERIES | VECTORSEARCH, `StandbyReplicas` and
    // `DeletionProtection` = ENABLED | DISABLED (smithy_query shape values). The last row
    // omits every optional member, so it pins the defaults; the row before it names a
    // customer managed key, so it pins that `kmsKeyArn` is the key the request named and
    // not the AWS-owned marker.
    let customer_key = format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/12345678-1234-1234-1234-123456789012");
    let cases: Vec<(&str, &str, Value, &str, &str, &str)> = vec![
        (
            "us-east-1",
            "alpha-search",
            json!({
                "name": "alpha-search",
                "type": "SEARCH",
                "description": "the search collection",
                "standbyReplicas": "DISABLED",
                "deletionProtection": "DISABLED",
                "tags": [{ "key": "team", "value": "search" }],
            }),
            "SEARCH",
            "DISABLED",
            "auto",
        ),
        (
            "us-west-2",
            "beta-timeseries",
            json!({
                "name": "beta-timeseries",
                "type": "TIMESERIES",
                "standbyReplicas": "ENABLED",
                "deletionProtection": "ENABLED",
            }),
            "TIMESERIES",
            "ENABLED",
            "auto",
        ),
        (
            "eu-west-1",
            "gamma-vectorsearch",
            json!({
                "name": "gamma-vectorsearch",
                "type": "VECTORSEARCH",
                "vectorOptions": { "ServerlessVectorAcceleration": "ENABLED" },
                "encryptionConfig": { "kmsKeyArn": customer_key },
            }),
            "VECTORSEARCH",
            "ENABLED",
            customer_key.as_str(),
        ),
        (
            "ap-southeast-2",
            "delta-defaults",
            json!({ "name": "delta-defaults" }),
            // `CreateCollection` defaults the collection type to TIMESERIES and standby
            // replicas to ENABLED when the request names neither.
            "TIMESERIES",
            "ENABLED",
            "auto",
        ),
    ];

    for (region, name, request, expected_type, expected_standby, expected_key) in cases {
        let case = format!("CreateCollection({name} in {region})");
        let detail = create_collection(&router, region, request).await;

        let id = text(&detail, "id");
        assert!(
            is_collection_id(&id),
            "case {case}: id must match ^[a-z0-9]{{3,40}}$, got {id:?}"
        );
        assert_eq!(
            text(&detail, "name"),
            name,
            "case {case}: name must be the requested one, got {detail}"
        );
        assert_eq!(
            text(&detail, "status"),
            "CREATING",
            "case {case}: a new collection is reported CREATING, got {detail}"
        );
        assert_eq!(
            text(&detail, "type"),
            expected_type,
            "case {case}: type must be {expected_type}, got {detail}"
        );
        assert_eq!(
            text(&detail, "standbyReplicas"),
            expected_standby,
            "case {case}: standbyReplicas must be {expected_standby}, got {detail}"
        );
        assert_eq!(
            text(&detail, "kmsKeyArn"),
            expected_key,
            "case {case}: kmsKeyArn must be {expected_key}, got {detail}"
        );
        assert_eq!(
            text(&detail, "arn"),
            format!("arn:aws:aoss:{region}:{ACCOUNT}:collection/{id}"),
            "case {case}: arn is derived from the id and the signed region, got {detail}"
        );
        let created = at(&detail, "createdDate").as_i64().unwrap_or_default();
        assert!(
            created > 1_600_000_000_000,
            "case {case}: createdDate must be epoch milliseconds, got {created}"
        );
        assert_eq!(
            at(&detail, "lastModifiedDate").as_i64(),
            Some(created),
            "case {case}: a just-created collection was last modified when it was created, got {detail}"
        );

        // The read-back: every member of `CollectionDetail` comes from the stored record,
        // and every endpoint names the address this emulator SERVES the collection at
        // (the collection's own listener, `crate::state::collection_plane`) rather than
        // AWS's own host, which resolves nowhere on the machine the emulator runs on. The
        // suite that drives those addresses is `announced_collection_endpoints`.
        let read = await_active(&router, region, &id).await;
        let endpoint = text(&read, "collectionEndpoint");
        assert!(
            !endpoint.contains("amazonaws.com") && endpoint.starts_with("https://"),
            "case {case}: collectionEndpoint must name an address this emulator serves, got \
             {read}"
        );
        assert_eq!(
            text(&read, "dashboardEndpoint"),
            format!("{endpoint}/_dashboards"),
            "case {case}: dashboardEndpoint is the collection endpoint plus /_dashboards, got {read}"
        );
        assert_eq!(
            text(&read, "fipsEndpoints.collectionEndpoint"),
            endpoint,
            "case {case}: the FIPS endpoint names the one address this emulator serves the \
             collection at, got {read}"
        );
        assert_eq!(
            text(&read, "type"),
            expected_type,
            "case {case}: the stored type survives the round trip, got {read}"
        );
        assert_eq!(
            text(&read, "description"),
            text(&detail, "description"),
            "case {case}: the stored description survives the round trip, got {read}"
        );
        assert_eq!(
            at(&read, "createdDate").as_i64(),
            Some(created),
            "case {case}: createdDate is stable across reads, got {read}"
        );
        assert_eq!(
            at(&read, "vectorOptions"),
            at(&detail, "vectorOptions"),
            "case {case}: the stored vectorOptions survive the round trip, got {read}"
        );
    }

    // Every collection got its own id, and the ids are what the listing reports.
    let (status, code, listed) = call(&router, "ListCollections", json!({ "maxResults": 100 })).await;
    assert_eq!(
        status, 200,
        "case ListCollections: expected 200, got {status} {code} with {listed}"
    );
    let ids: BTreeSet<String> = at(&listed, "collectionSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| text(summary, "id"))
        .collect();
    assert_eq!(
        ids.len(),
        4,
        "case ListCollections: four creates must have minted four distinct ids, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// BatchGetCollection: id addressing, name addressing, and the documented
// `NOT_FOUND` error detail for a key that names no collection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_get_collection_answers_from_the_store_and_reports_missing_keys() {
    let router = emulator();

    let mut ids = Vec::new();
    let names = ["one-collection", "two-collection", "three-collection"];
    for name in names {
        let detail = create_collection(&router, "us-east-1", json!({ "name": name })).await;
        ids.push(text(&detail, "id"));
    }

    // Addressing the same records by id and by name reports the same details, so neither
    // path echoes the request.
    let (_, _, by_id) = call(&router, "BatchGetCollection", json!({ "ids": ids })).await;
    let (_, _, by_name) = call(&router, "BatchGetCollection", json!({ "names": names })).await;
    assert_eq!(
        at(&by_id, "collectionDetails"),
        at(&by_name, "collectionDetails"),
        "case BatchGetCollection(ids vs names): both must report the same stored records, got {by_id} and {by_name}"
    );
    assert_eq!(
        at(&by_id, "collectionErrorDetails"),
        &json!([]),
        "case BatchGetCollection(all present): collectionErrorDetails must be empty, got {by_id}"
    );

    // A key that names no collection is reported in `collectionErrorDetails`, under the
    // same member it was requested under. The table spans both addressing modes and both
    // mixes of present and absent keys.
    let absent_id = "zzzzzzzzzzzzzzzzzzzzzzzz";
    let absent_name = "nosuch-collection";
    let cases: Vec<(&str, Value, usize, Value)> = vec![
        (
            "one absent id",
            json!({ "ids": [absent_id] }),
            0,
            json!([{ "id": absent_id, "errorCode": "NOT_FOUND", "errorMessage": "The specified Collection is not found." }]),
        ),
        (
            "one present and one absent id",
            json!({ "ids": [ids[0].clone(), absent_id.to_string()] }),
            1,
            json!([{ "id": absent_id, "errorCode": "NOT_FOUND", "errorMessage": "The specified Collection is not found." }]),
        ),
        (
            "one absent name",
            json!({ "names": [absent_name] }),
            0,
            json!([{ "name": absent_name, "errorCode": "NOT_FOUND", "errorMessage": "The specified Collection is not found." }]),
        ),
        (
            "two present and one absent name",
            json!({ "names": [names[0], names[1], absent_name] }),
            2,
            json!([{ "name": absent_name, "errorCode": "NOT_FOUND", "errorMessage": "The specified Collection is not found." }]),
        ),
    ];
    for (case, request, expected_details, expected_errors) in cases {
        let (status, code, answer) = call(&router, "BatchGetCollection", request).await;
        assert_eq!(
            status, 200,
            "case BatchGetCollection({case}): expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            at(&answer, "collectionDetails")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            expected_details,
            "case BatchGetCollection({case}): expected {expected_details} details, got {answer}"
        );
        assert_eq!(
            at(&answer, "collectionErrorDetails"),
            &expected_errors,
            "case BatchGetCollection({case}): expected {expected_errors}, got {answer}"
        );
    }

    // From `ids`' and `names`' own documentation: "You can't provide names and IDs in the
    // same request"; and a request naming neither selects nothing at all.
    for (case, request) in [
        ("both", json!({ "ids": [ids[0].clone()], "names": [names[0]] })),
        ("neither", json!({})),
    ] {
        let (status, code, answer) = call(&router, "BatchGetCollection", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case BatchGetCollection({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListCollections: the filters it declares, and the invariant a page bound owes —
// every record exactly once, in one order, over a range of page sizes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_collections_filters_and_pages_over_the_stored_records() {
    let router = emulator();

    let total = 7;
    let mut created: BTreeSet<String> = BTreeSet::new();
    for index in 0..total {
        let name = format!("paged-collection-{index}");
        let detail = create_collection(&router, "us-east-1", json!({ "name": name })).await;
        created.insert(text(&detail, "id"));
    }

    // The pagination invariant, over every page bound from 1 to one past the record count
    // plus the whole published maximum: walking the pages visits each stored record
    // exactly once, and a `nextToken` comes back only while records remain.
    for bound in [1, 2, 3, 6, 7, 8, 100] {
        let case = format!("ListCollections(maxResults={bound})");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= total + 2,
                "case {case}: pagination did not terminate after {pages} pages"
            );
            let mut request = json!({ "maxResults": bound });
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, code, answer) = call(&router, "ListCollections", request).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {code} with {answer}"
            );
            let page = at(&answer, "collectionSummaries")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= usize::try_from(bound).unwrap_or(1),
                "case {case}: a page may hold at most {bound} records, got {}",
                page.len()
            );
            for summary in &page {
                let id = text(summary, "id");
                assert_eq!(
                    text(summary, "arn"),
                    format!("arn:aws:aoss:us-east-1:{ACCOUNT}:collection/{id}"),
                    "case {case}: a summary's arn is derived from its id, got {summary}"
                );
                assert_eq!(
                    text(summary, "kmsKeyArn"),
                    "auto",
                    "case {case}: a summary reports the stored kmsKeyArn, got {summary}"
                );
                seen.push(id);
            }
            match at(&answer, "nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: no record may appear on two pages, got {seen:?}"
        );
        assert_eq!(
            unique, created,
            "case {case}: the pages together must be exactly the stored records, got {seen:?}"
        );
    }

    // A `nextToken` this service never issued is refused rather than served as page one.
    let (status, code, answer) = call(
        &router,
        "ListCollections",
        json!({ "nextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case ListCollections(forged nextToken): expected 400 ValidationException, got {status} {code} with {answer}"
    );

    // Each filter the model declares is an equality test on the stored record, and the
    // filters compose.
    let target = "paged-collection-3";
    let cases: Vec<(&str, Value, usize)> = vec![
        ("name of a stored collection", json!({ "name": target }), 1),
        ("name of no collection", json!({ "name": "absent-collection" }), 0),
        ("status every record is in", json!({ "status": "CREATING" }), total),
        ("status no record is in", json!({ "status": "DELETING" }), 0),
        (
            "name and matching status",
            json!({ "name": target, "status": "CREATING" }),
            1,
        ),
        (
            "name and mismatching status",
            json!({ "name": target, "status": "FAILED" }),
            0,
        ),
        (
            "a collection group none of them is in",
            json!({ "collectionGroupName": "some-group" }),
            0,
        ),
    ];
    for (case, filters, expected) in cases {
        let (status, code, answer) = call(
            &router,
            "ListCollections",
            json!({ "collectionFilters": filters, "maxResults": 100 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case ListCollections({case}): expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            at(&answer, "collectionSummaries")
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            expected,
            "case ListCollections({case}): expected {expected} summaries, got {answer}"
        );
    }
}

// ---------------------------------------------------------------------------
// DeleteCollection: the refusals it owes, and the removal every later read sees.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_collection_removes_it_and_refuses_what_it_must() {
    let router = emulator();

    // A collection still in `CREATING` cannot be deleted: `ConflictException` is "thrown
    // when the resource is not in the ACTIVE, FAILED, or UPDATE_FAILED state". The
    // transition is owed by the clock, so the transient status and its state error are
    // observable to a caller acting straight after the create.
    let transient = create_collection(&router, "us-east-1", json!({ "name": "transient-collection" })).await;
    let transient_id = text(&transient, "id");
    let (status, code, answer) = call(
        &router,
        "DeleteCollection",
        json!({ "id": transient_id.clone() }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ConflictException"),
        "case DeleteCollection(still CREATING): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
    );
    await_active(&router, "us-east-1", &transient_id).await;

    // Deletion protection, over both values of the enum: ENABLED refuses, DISABLED (and
    // the default, which is DISABLED) deletes.
    let protected = create_collection(
        &router,
        "us-east-1",
        json!({ "name": "protected-collection", "deletionProtection": "ENABLED" }),
    )
    .await;
    let protected_id = text(&protected, "id");
    await_active(&router, "us-east-1", &protected_id).await;
    let (status, code, answer) = call(
        &router,
        "DeleteCollection",
        json!({ "id": protected_id.clone() }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ValidationException"),
        "case DeleteCollection(deletionProtection ENABLED): expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
    );

    for (case, name, request_protection) in [
        ("explicit DISABLED", "deletable-collection", Some("DISABLED")),
        ("omitted, so DISABLED", "default-collection", None),
    ] {
        let mut request = json!({ "name": name });
        if let Some(protection) = request_protection {
            request["deletionProtection"] = json!(protection);
        }
        let detail = create_collection(&router, "us-east-1", request).await;
        let id = text(&detail, "id");
        await_active(&router, "us-east-1", &id).await;

        let (status, code, answer) =
            call(&router, "DeleteCollection", json!({ "id": id.clone() })).await;
        assert_eq!(
            status, 200,
            "case DeleteCollection({case}): expected 200, got {status} {code} with {answer}"
        );
        let deleted = at(&answer, "deleteCollectionDetail").clone();
        assert_eq!(
            text(&deleted, "status"),
            "DELETING",
            "case DeleteCollection({case}): the delete reports DELETING, got {deleted}"
        );
        assert_eq!(
            (text(&deleted, "id"), text(&deleted, "name")),
            (id.clone(), name.to_string()),
            "case DeleteCollection({case}): the detail names the stored record, got {deleted}"
        );
        assert_eq!(
            text(&deleted, "deletionProtection"),
            "DISABLED",
            "case DeleteCollection({case}): the detail reports the stored deletionProtection, got {deleted}"
        );

        // The delete is ACCEPTED, not completed: the collection stays readable in
        // `DELETING` until the clock makes its purge due, so a caller polling the deletion
        // sees that state instead of the resource vanishing in one step.
        let (_, _, deleting) =
            call(&router, "BatchGetCollection", json!({ "ids": [id.clone()] })).await;
        let visible = at(&deleting, "collectionDetails")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            visible.len(),
            1,
            "case DeleteCollection({case}): the collection is still visible right after the delete, got {deleting}"
        );
        assert_eq!(
            text(&visible[0], "status"),
            "DELETING",
            "case DeleteCollection({case}): it is visible in DELETING, got {deleting}"
        );
        assert_eq!(
            at(&deleting, "collectionErrorDetails"),
            &json!([]),
            "case DeleteCollection({case}): a DELETING collection is not an error detail, got {deleting}"
        );
        // A second delete while it is DELETING is the state conflict, not a not-found: the
        // collection is still there.
        let (status, code, answer) =
            call(&router, "DeleteCollection", json!({ "id": id.clone() })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case DeleteCollection({case}, while DELETING): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
        );

        // And after the purge the record is gone: a read reports it as missing, the
        // listing no longer carries it, and a later delete is the not-found refusal.
        await_purged(&router, "us-east-1", &id).await;
        let (_, _, read) = call(&router, "BatchGetCollection", json!({ "ids": [id.clone()] })).await;
        assert_eq!(
            at(&read, "collectionDetails"),
            &json!([]),
            "case DeleteCollection({case}): a purged collection has no details, got {read}"
        );
        assert_eq!(
            text(at(&read, "collectionErrorDetails").get(0).unwrap_or(&Value::Null), "errorCode"),
            "NOT_FOUND",
            "case DeleteCollection({case}): a purged collection is reported NOT_FOUND, got {read}"
        );

        let (_, _, listed) = call(
            &router,
            "ListCollections",
            json!({ "collectionFilters": { "name": name }, "maxResults": 100 }),
        )
        .await;
        assert_eq!(
            at(&listed, "collectionSummaries"),
            &json!([]),
            "case DeleteCollection({case}): a purged collection is no longer listed, got {listed}"
        );

        let (status, code, answer) = call(&router, "DeleteCollection", json!({ "id": id })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case DeleteCollection({case}, after the purge): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );

        // And the name is free again, which is what proves the name index was dropped
        // with the record.
        create_collection(&router, "us-east-1", json!({ "name": name })).await;
    }

    // An id of the right shape that names no collection is the `ResourceNotFoundException`,
    // not an empty success.
    let (status, code, answer) = call(
        &router,
        "DeleteCollection",
        json!({ "id": "aaaaaaaaaaaaaaaaaaaaaaaa" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ResourceNotFoundException"),
        "case DeleteCollection(unknown id): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
    );
}

// ---------------------------------------------------------------------------
// The refusals a create owes: a name already taken, and a value outside a
// modelled constraint or enum value list.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_collection_refuses_a_duplicate_name_and_an_invalid_input() {
    let router = emulator();

    create_collection(&router, "us-east-1", json!({ "name": "taken-collection" })).await;
    for (case, request) in [
        (
            "same members",
            json!({ "name": "taken-collection", "encryptionConfig": { "aWSOwnedKey": true } }),
        ),
        (
            "different members",
            json!({
                "name": "taken-collection",
                "type": "SEARCH",
                "description": "another",
                "encryptionConfig": { "aWSOwnedKey": true },
            }),
        ),
    ] {
        let (status, code, answer) = call(&router, "CreateCollection", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ConflictException"),
            "case CreateCollection(duplicate name, {case}): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
        );
        assert_eq!(
            text(&answer, "message"),
            "A collection with name 'taken-collection' already exists.",
            "case CreateCollection(duplicate name, {case}): the message is built from the request's own name, got {answer}"
        );
    }

    // Every modelled constraint on the input is refused with a 400 `ValidationException`:
    // the `name` length bounds and pattern, and each enum member's value list. The rows
    // walk both published `name` bounds and one past each.
    let cases: Vec<(&str, Value)> = vec![
        ("name one short of the 3-character minimum", json!({ "name": "ab" })),
        ("name one past the 64-character maximum", json!({ "name": "a".repeat(65) })),
        ("name starting with a digit", json!({ "name": "1bad-collection" })),
        ("name with an uppercase letter", json!({ "name": "Bad-collection" })),
        ("name with an underscore", json!({ "name": "bad_collection" })),
        ("type outside CollectionType", json!({ "name": "ok-collection", "type": "GRAPH" })),
        (
            "type in the wrong case",
            json!({ "name": "ok-collection", "type": "search" }),
        ),
        (
            "standbyReplicas outside StandbyReplicas",
            json!({ "name": "ok-collection", "standbyReplicas": "MAYBE" }),
        ),
        (
            "deletionProtection outside DeletionProtection",
            json!({ "name": "ok-collection", "deletionProtection": "ON" }),
        ),
        (
            "vectorOptions outside ServerlessVectorAccelerationStatus",
            json!({ "name": "ok-collection", "vectorOptions": { "ServerlessVectorAcceleration": "SOMETIMES" } }),
        ),
        ("name absent", json!({ "description": "no name" })),
    ];
    for (case, request) in cases {
        let (status, code, answer) = call(&router, "CreateCollection", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case CreateCollection({case}): expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }

    // The three `name` values just inside the published bounds are accepted, so the
    // refusals above are the bounds and not a blanket rejection.
    for name in ["abc", &"a".repeat(64), "a-b-c-1-2-3"] {
        create_collection(&router, "us-east-1", json!({ "name": name })).await;
    }
}

// ---------------------------------------------------------------------------
// The `@idempotencyToken` contract: a replayed `clientToken` answers the record
// the first call created rather than creating or conflicting with a second one.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_answers_the_record_it_first_created() {
    let router = emulator();

    let request = json!({ "name": "idempotent-collection", "type": "SEARCH", "clientToken": "token-one" });
    let first = create_collection(&router, "us-east-1", request.clone()).await;
    let second = create_collection(&router, "us-east-1", request).await;
    assert_eq!(
        first, second,
        "case CreateCollection(replayed clientToken): the retry must answer the first record, got {first} and {second}"
    );

    // A different token for the same name is a genuinely new request, so it conflicts.
    let (status, code, answer) = call(
        &router,
        "CreateCollection",
        json!({
            "name": "idempotent-collection",
            "clientToken": "token-two",
            "encryptionConfig": { "aWSOwnedKey": true },
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ConflictException"),
        "case CreateCollection(new token, taken name): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
    );

    // The same contract on the policy create.
    let policy_request = json!({
        "type": "encryption",
        "name": "idempotent-policy",
        "policy": r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/idempotent*"]}],"AWSOwnedKey":true}"#,
        "clientToken": "policy-token",
    });
    let (_, _, first) = call(&router, "CreateSecurityPolicy", policy_request.clone()).await;
    let (status, code, second) = call(&router, "CreateSecurityPolicy", policy_request).await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy(replayed clientToken): expected 200, got {status} {code} with {second}"
    );
    assert_eq!(
        at(&first, "securityPolicyDetail"),
        at(&second, "securityPolicyDetail"),
        "case CreateSecurityPolicy(replayed clientToken): the retry must answer the first record, got {first} and {second}"
    );
}

// ---------------------------------------------------------------------------
// The `clientToken` replay index is BOUNDED: a token never outlives the record
// it created, and the index is held at its cap by oldest-first eviction.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_client_token_index_stays_bounded() {
    use service_com_amazonaws_opensearchserverless_opensearchserverless::state::{
        service_state, MAX_IDEMPOTENCY_TOKENS,
    };

    let (router, engine) = emulator_with_engine();
    let state = service_state(&*engine).expect("the store is reachable from the shared engine");

    let tokens = |case: &str| -> usize {
        let count = state.lock().idempotency_token_count();
        assert!(
            count <= MAX_IDEMPOTENCY_TOKENS,
            "case {case}: the clientToken index must never exceed {MAX_IDEMPOTENCY_TOKENS}, got {count}"
        );
        count
    };

    // A create/delete loop with a FRESH token each cycle frees its resource slot, so
    // without forget-on-delete it would grow the index once per request forever. The
    // table runs more cycles than the per-type policy quota allows live records, which is
    // what shows the index is bounded by the records and not by the request count.
    for cycle in 0..25 {
        let name = format!("cycle-{cycle}-policy");
        let (status, code, answer) = call(
            &router,
            "CreateSecurityPolicy",
            json!({
                "type": "encryption",
                "name": name,
                "policy": r#"{"Rules":[]}"#,
                "clientToken": format!("fresh-token-{cycle}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            tokens(&format!("cycle {cycle}, policy live")),
            1,
            "case cycle {cycle}: the live policy's token is the only entry"
        );

        let (status, code, answer) = call(
            &router,
            "DeleteSecurityPolicy",
            json!({ "type": "encryption", "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: expected 200 from the delete, got {status} {code} with {answer}"
        );
        assert_eq!(
            tokens(&format!("cycle {cycle}, policy deleted")),
            0,
            "case cycle {cycle}: deleting the policy must forget the token that created it"
        );
    }

    // The same for a collection, whose delete goes through the other removal path.
    let detail = create_collection(
        &router,
        "us-east-1",
        json!({ "name": "token-collection", "clientToken": "collection-token" }),
    )
    .await;
    let id = text(&detail, "id");
    assert_eq!(
        tokens("collection live"),
        1,
        "case collection live: the create remembered its token"
    );
    await_active(&router, "us-east-1", &id).await;
    let (status, code, answer) = call(&router, "DeleteCollection", json!({ "id": id.clone() })).await;
    assert_eq!(
        status, 200,
        "case collection delete: expected 200, got {status} {code} with {answer}"
    );
    // The token is forgotten with the RECORD, which the delete only schedules: it goes
    // when the purge the clock owes removes the collection.
    await_purged(&router, "us-east-1", &id).await;
    assert_eq!(
        tokens("collection purged"),
        0,
        "case collection purged: purging the collection must forget the token that created it"
    );

    // And the cap itself, over twice its own size: the index is held at exactly the cap by
    // oldest-first eviction, and the newest tokens are the ones still replayable.
    {
        let mut store = state.lock();
        for index in 0..(MAX_IDEMPOTENCY_TOKENS * 2) {
            store.remember_token("CreateCollection", &format!("bulk-{index}"), "some-record");
        }
        assert_eq!(
            store.idempotency_token_count(),
            MAX_IDEMPOTENCY_TOKENS,
            "case twice the cap: the index is held at exactly {MAX_IDEMPOTENCY_TOKENS}"
        );
    }
}

// ---------------------------------------------------------------------------
// Security policies: the create/read/delete lifecycle over both policy types,
// a table of policy documents, and the refusals each step owes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_security_policy_lifecycle_is_served_from_state() {
    let router = emulator();

    // `SecurityPolicyType` = encryption | network (smithy_query shape values). The policy
    // documents span an object with a nested list, an object with a boolean and a number,
    // and a bare JSON array — so what is stored is the parsed document, not its text.
    let cases: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "encryption",
            "demo-encryption",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/demo*"]}],"AWSOwnedKey":true}"#,
            Some("the encryption policy"),
        ),
        (
            "network",
            "demo-network",
            r#"[{"Rules":[{"ResourceType":"dashboard","Resource":["collection/demo*"]}],"AllowFromPublic":true}]"#,
            None,
        ),
        (
            // The same NAME under the other type is a different policy: the pair is the
            // identity, not the name.
            "network",
            "demo-encryption",
            r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/other*"]}],"AllowFromPublic":false,"SourceVPCEs":[]}"#,
            Some("a network policy reusing the encryption policy's name"),
        ),
    ];

    let mut versions: BTreeSet<String> = BTreeSet::new();
    for (kind, name, policy, description) in &cases {
        let case = format!("CreateSecurityPolicy({kind}/{name})");
        let mut request = json!({ "type": kind, "name": name, "policy": policy });
        if let Some(description) = description {
            request["description"] = json!(description);
        }
        let (status, code, answer) = call(&router, "CreateSecurityPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "securityPolicyDetail").clone();
        assert_eq!(
            (text(&detail, "type"), text(&detail, "name")),
            (kind.to_string(), name.to_string()),
            "case {case}: the detail names the stored policy, got {detail}"
        );
        let version = text(&detail, "policyVersion");
        assert!(
            is_policy_version(&version),
            "case {case}: policyVersion must match PolicyVersion's length and pattern, got {version:?}"
        );
        assert!(
            versions.insert(version.clone()),
            "case {case}: every policy write mints a fresh policyVersion, got a repeat of {version:?}"
        );
        // The document is reported PARSED, so it equals the JSON the request carried
        // whatever whitespace that text had.
        let expected: Value = serde_json::from_str(policy).expect("the case policy is JSON");
        assert_eq!(
            at(&detail, "policy"),
            &expected,
            "case {case}: the policy document is the parsed request document, got {detail}"
        );
        assert_eq!(
            text(&detail, "description"),
            description.unwrap_or_default(),
            "case {case}: the stored description is reported, got {detail}"
        );

        // The read-back through `GetSecurityPolicy` reports exactly what the create
        // stored, down to the version and the timestamps.
        let (status, code, read) = call(
            &router,
            "GetSecurityPolicy",
            json!({ "type": kind, "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case GetSecurityPolicy({kind}/{name}): expected 200, got {status} {code} with {read}"
        );
        assert_eq!(
            at(&read, "securityPolicyDetail"),
            &detail,
            "case GetSecurityPolicy({kind}/{name}): the read must answer the created record, got {read}"
        );
    }

    // A second create for a pair already stored is the conflict, and it leaves the stored
    // policy alone.
    let (status, code, answer) = call(
        &router,
        "CreateSecurityPolicy",
        json!({ "type": "encryption", "name": "demo-encryption", "policy": r#"{"Rules":[]}"# }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, "ConflictException"),
        "case CreateSecurityPolicy(duplicate pair): expected {CLIENT_ERROR} ConflictException, got {status} {code} with {answer}"
    );

    // The refusals a policy create owes: a `type` outside the value list, a `policy` that
    // is not a JSON document, and the `name` bounds.
    let valid_policy = r#"{"Rules":[]}"#;
    let refusals: Vec<(&str, Value)> = vec![
        (
            "type outside SecurityPolicyType",
            json!({ "type": "access", "name": "bad-policy", "policy": valid_policy }),
        ),
        (
            "type in the wrong case",
            json!({ "type": "Encryption", "name": "bad-policy", "policy": valid_policy }),
        ),
        (
            "policy that is not JSON",
            json!({ "type": "encryption", "name": "bad-policy", "policy": "not json at all" }),
        ),
        (
            "name one short of the 3-character minimum",
            json!({ "type": "encryption", "name": "ab", "policy": valid_policy }),
        ),
        (
            "name one past the 32-character maximum",
            json!({ "type": "encryption", "name": "a".repeat(33), "policy": valid_policy }),
        ),
        (
            "policy absent",
            json!({ "type": "encryption", "name": "bad-policy" }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "CreateSecurityPolicy", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case CreateSecurityPolicy({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // Deleting removes exactly the pair named, leaving the others readable; a second
    // delete, and a read, then report it missing.
    for (kind, name, _, _) in &cases {
        let (status, code, answer) = call(
            &router,
            "DeleteSecurityPolicy",
            json!({ "type": kind, "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case DeleteSecurityPolicy({kind}/{name}): expected 200, got {status} {code} with {answer}"
        );
        for (operation, expected) in [
            ("DeleteSecurityPolicy", "ResourceNotFoundException"),
            ("GetSecurityPolicy", "ResourceNotFoundException"),
        ] {
            let (status, code, answer) =
                call(&router, operation, json!({ "type": kind, "name": name })).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, expected),
                "case {operation}({kind}/{name}, after delete): expected {CLIENT_ERROR} {expected}, got {status} {code} with {answer}"
            );
        }
    }

    // And a pair that was never created is the `ResourceNotFoundException` too, for both
    // types.
    for kind in ["encryption", "network"] {
        let (status, code, answer) = call(
            &router,
            "GetSecurityPolicy",
            json!({ "type": kind, "name": "never-created" }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ResourceNotFoundException"),
            "case GetSecurityPolicy({kind}/never-created): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
        );
    }
}
