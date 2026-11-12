// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the LF-tag lifecycle: `CreateLFTag`, `GetLFTag`, `ListLFTags`,
//! `UpdateLFTag`, `DeleteLFTag`.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store.
//!
//! The tables below span the classes the rules generalise over (value, count, ordering,
//! page bound, catalog, share type); the issue's own request is one row among them.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lakeformation_awslakeformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated service presents, and therefore the Data Catalog a
/// request without `CatalogId` addresses ("The identifier for the Data Catalog. By
/// default, the account ID." — `CatalogIdString`'s documentation).
fn account_catalog() -> String {
    engine::emulator_account_id().to_string()
}

/// One emulator over one shared engine, reached exactly as the production router reaches
/// it: `MultiServiceRouter::dispatch` of a buffered HTTP request.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the shared engine handed back so a test can seed the store it
/// serves (the quota test fills a catalog to its ceiling without a thousand round trips).
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// `POST /<operation>` with `body` as the JSON payload, answering `(status, body)`.
///
/// The `Authorization` credential scope names `lakeformation`, which is the routing
/// signal `resolve_route_key` reads first; no authenticator is registered, so the
/// signature itself is not checked.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://lakeformation.us-east-1.amazonaws.com/{operation}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/lakeformation/aws4_request, \
             SignedHeaders=host, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// The wire error code (`__type`) of an error envelope, or `""` for a success body.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `TagValues` of a `GetLFTag` / `LFTagPair` payload as owned strings.
fn tag_values(payload: &Value) -> Vec<String> {
    payload
        .get("TagValues")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Create a tag and assert it was accepted.
async fn create(router: &MultiServiceRouter, catalog: Option<&str>, key: &str, values: &[&str]) {
    let mut body = json!({ "TagKey": key, "TagValues": values });
    if let Some(catalog) = catalog {
        body["CatalogId"] = json!(catalog);
    }
    let (status, payload) = call(router, "CreateLFTag", &body).await;
    assert_eq!(
        status, 200,
        "case create {key:?} in {catalog:?}: expected 200, got {status} {payload:?}"
    );
}

#[tokio::test]
async fn create_then_get_reads_the_stored_values_back() {
    // Rows differ along what the rule generalises over: the number of values, the order
    // they are supplied in, repeated values, the character classes `LFTagKey` /
    // `LFTagValue` admit, and the modelled upper bound of `TagValueList` (len 1..50).
    // A tag's values are a SET, reported in the order the tag was created with them.
    let fifty: Vec<String> = (0..50).map(|i| format!("v{i}")).collect();
    let fifty: Vec<&str> = fifty.iter().map(String::as_str).collect();
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("environment", vec!["dev", "prod"], vec!["dev", "prod"]),
        ("single", vec!["only"], vec!["only"]),
        // Values are a set: a repeated value is stored once, at the position it was
        // first supplied at.
        ("repeated", vec!["b", "a", "b"], vec!["b", "a"]),
        // The order is the order given, not a sorted order.
        ("ordered", vec!["z", "m", "a"], vec!["z", "m", "a"]),
        // `LFTagValue` is len 0..256, so the empty string is a legal value.
        ("empty-value", vec![""], vec![""]),
        ("empty-value-among-others", vec!["b", "", "a"], vec!["b", "", "a"]),
        (
            "shared-prefix",
            vec!["tier-2", "tier-10", "tier-1"],
            vec!["tier-2", "tier-10", "tier-1"],
        ),
        // `LFTagKey` admits letters, marks, numbers and `_.:/=+-@%`.
        ("cost.centre:eu/west=1+a-b@c%d", vec!["x"], vec!["x"]),
        // A key outside ASCII: `\p{L}` is any letter.
        ("données", vec!["oui"], vec!["oui"]),
        // The modelled maximum number of values.
        ("fifty", fifty.clone(), fifty),
    ];

    let router = emulator();
    for (key, values, expected) in &cases {
        create(&router, None, key, values).await;

        let (status, payload) = call(&router, "GetLFTag", &json!({ "TagKey": key })).await;
        assert_eq!(
            status, 200,
            "case {key:?}: expected GetLFTag 200, got {status} {payload:?}"
        );
        assert_eq!(
            payload.get("CatalogId").and_then(Value::as_str),
            Some(account_catalog().as_str()),
            "case {key:?}: CatalogId must default to the account id, got {payload:?}"
        );
        assert_eq!(
            payload.get("TagKey").and_then(Value::as_str),
            Some(*key),
            "case {key:?}: TagKey must be the stored key, got {payload:?}"
        );
        assert_eq!(
            tag_values(&payload),
            *expected,
            "case {key:?}: expected TagValues {expected:?}, got {:?}",
            tag_values(&payload)
        );
    }
}

#[tokio::test]
async fn create_refuses_a_tag_key_that_already_exists_in_the_catalog() {
    // The key is unique per CATALOG, so the same key in another catalog is a new tag.
    let foreign = "210987654321";
    // The third element is what the tag is created with and the fourth what the refused
    // repeat names; the stored set survives in the order it was created with.
    let cases = [
        (None, "environment", vec!["dev"], vec!["prod"]),
        (None, "team", vec!["red", "blue"], vec!["green"]),
        (Some(foreign), "environment", vec!["dev"], vec!["staging"]),
    ];

    let router = emulator();
    for (catalog, key, first, second) in &cases {
        create(&router, *catalog, key, first).await;

        let mut body = json!({ "TagKey": key, "TagValues": second });
        if let Some(catalog) = catalog {
            body["CatalogId"] = json!(catalog);
        }
        let (status, payload) = call(&router, "CreateLFTag", &body).await;
        assert_eq!(
            status, 400,
            "case {catalog:?}/{key:?}: a duplicate key must be refused with 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {catalog:?}/{key:?}: expected InvalidInputException, got {payload:?}"
        );

        // The refusal must not have overwritten what was stored.
        let mut get = json!({ "TagKey": key });
        if let Some(catalog) = catalog {
            get["CatalogId"] = json!(catalog);
        }
        let (_, payload) = call(&router, "GetLFTag", &get).await;
        assert_eq!(
            tag_values(&payload),
            *first,
            "case {catalog:?}/{key:?}: expected the stored values {first:?} to survive, got {:?}",
            tag_values(&payload)
        );
    }
}

#[tokio::test]
async fn an_unknown_tag_key_is_entity_not_found_everywhere_it_is_addressed() {
    // One rule, three operations and several addressings: a tag key the catalog does not
    // hold is `EntityNotFoundException` (HTTP 400), never an empty 200.
    let router = emulator();
    create(&router, None, "environment", &["dev"]).await;

    let cases: Vec<(&str, Value)> = vec![
        ("GetLFTag", json!({ "TagKey": "never-created" })),
        ("DeleteLFTag", json!({ "TagKey": "never-created" })),
        (
            "UpdateLFTag",
            json!({ "TagKey": "never-created", "TagValuesToAdd": ["x"] }),
        ),
        // The key exists, but in another catalog than the one addressed.
        (
            "GetLFTag",
            json!({ "CatalogId": "210987654321", "TagKey": "environment" }),
        ),
        (
            "DeleteLFTag",
            json!({ "CatalogId": "210987654321", "TagKey": "environment" }),
        ),
        (
            "UpdateLFTag",
            json!({ "CatalogId": "210987654321", "TagKey": "environment", "TagValuesToAdd": ["x"] }),
        ),
    ];

    for (operation, body) in &cases {
        let (status, payload) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {operation} {body}: expected 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "EntityNotFoundException",
            "case {operation} {body}: expected EntityNotFoundException, got {payload:?}"
        );
    }
}

#[tokio::test]
async fn list_pages_through_every_stored_tag_and_terminates() {
    // Rows differ along the tag count and the page bound, including no bound at all and
    // a bound above what `LFTagsList` (len 1..50) can carry.
    let cases: [(usize, Option<i64>, usize); 7] = [
        (0, None, 0),
        (1, Some(1), 1),
        (3, Some(1), 1),
        (5, Some(2), 2),
        (7, None, 7),
        // `MaxResults` is modelled range 1..1000, but one page can hold at most the 50
        // entries `LFTagsList` admits.
        (51, Some(1000), 50),
        (51, None, 50),
    ];

    for (count, max_results, expected_first_page) in cases {
        let router = emulator();
        let mut created: Vec<String> = (0..count).map(|i| format!("tag-{i:03}")).collect();
        for key in &created {
            create(&router, None, key, &["v"]).await;
        }
        created.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({});
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let (status, payload) = call(&router, "ListLFTags", &body).await;
            assert_eq!(
                status, 200,
                "case count={count} max={max_results:?}: expected 200, got {status} {payload:?}"
            );

            let page: Vec<String> = payload
                .get("LFTags")
                .and_then(Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .map(|tag| {
                            assert_eq!(
                                tag.get("CatalogId").and_then(Value::as_str),
                                Some(account_catalog().as_str()),
                                "case count={count} max={max_results:?}: every pair carries its catalog, got {tag:?}"
                            );
                            assert_eq!(
                                tag_values(tag),
                                vec!["v".to_string()],
                                "case count={count} max={max_results:?}: expected the stored values, got {tag:?}"
                            );
                            tag.get("TagKey")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default();

            if pages == 0 {
                assert_eq!(
                    page.len(),
                    expected_first_page,
                    "case count={count} max={max_results:?}: expected a first page of {expected_first_page}, got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;

            next_token = payload
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= count + 1,
                "case count={count} max={max_results:?}: a paginating client must terminate, \
                 but page {pages} still returned a NextToken"
            );
        }

        assert_eq!(
            seen, created,
            "case count={count} max={max_results:?}: the pages together must be every stored tag, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_selects_catalogs_by_resource_share_type() {
    // `ResourceShareType`: ALL "returns both in-account LF-tags and shared LF-tags",
    // FOREIGN "returns all share LF-tags", absent lists "LF-tags in the given catalog
    // ID" (`smithy_query(query="operation",operation="ListLFTags")`). A catalog other
    // than the caller's own account is a shared (foreign) one.
    let foreign = "210987654321";
    let router = emulator();
    create(&router, None, "own", &["v"]).await;
    create(&router, Some(foreign), "shared", &["v"]).await;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no share type, default catalog", json!({}), vec!["own"]),
        (
            "no share type, explicit account catalog",
            json!({ "CatalogId": account_catalog() }),
            vec!["own"],
        ),
        (
            "no share type, explicit foreign catalog",
            json!({ "CatalogId": foreign }),
            vec!["shared"],
        ),
        (
            "ALL, default catalog",
            json!({ "ResourceShareType": "ALL" }),
            vec!["own", "shared"],
        ),
        // ALL is in-account PLUS shared, so addressing a foreign catalog does not drop
        // the caller's own in-account tags. Catalogs enumerate in sorted order, and the
        // account catalog sorts before this foreign one.
        (
            "ALL, explicit foreign catalog",
            json!({ "CatalogId": foreign, "ResourceShareType": "ALL" }),
            vec!["own", "shared"],
        ),
        (
            "FOREIGN, default catalog",
            json!({ "ResourceShareType": "FOREIGN" }),
            vec!["shared"],
        ),
        (
            "FOREIGN, explicit foreign catalog",
            json!({ "CatalogId": foreign, "ResourceShareType": "FOREIGN" }),
            vec!["shared"],
        ),
    ];

    for (label, body, expected) in &cases {
        let (status, payload) = call(&router, "ListLFTags", body).await;
        assert_eq!(
            status, 200,
            "case {label}: expected 200, got {status} {payload:?}"
        );
        let keys: Vec<String> = payload
            .get("LFTags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .map(|tag| {
                        tag.get("TagKey")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            keys, *expected,
            "case {label}: expected {expected:?}, got {keys:?}"
        );
    }

    // A value outside the modelled enum never reaches the handler: the boundary
    // validator refuses it against `ResourceShareType`'s own enum value set.
    let (status, payload) =
        call(&router, "ListLFTags", &json!({ "ResourceShareType": "SOMETHING" })).await;
    assert_eq!(
        status, 400,
        "case ResourceShareType=SOMETHING: expected 400, got {status} {payload:?}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case ResourceShareType=SOMETHING: expected ValidationException, got {payload:?}"
    );
}

#[tokio::test]
async fn list_refuses_a_continuation_token_it_did_not_mint() {
    let router = emulator();
    create(&router, None, "a", &["v"]).await;
    create(&router, None, "b", &["v"]).await;

    // Not hex; hex of a plaintext with the wrong prefix; hex of too few fields; empty.
    let cases = ["", "not-a-token", "zz", &hex::encode("other\u{1f}1\u{1f}c\u{1f}k"), &hex::encode("lftags")];
    for token in cases {
        let (status, payload) =
            call(&router, "ListLFTags", &json!({ "NextToken": token })).await;
        assert_eq!(
            status, 400,
            "case token={token:?}: expected 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case token={token:?}: expected InvalidInputException, got {payload:?}"
        );
    }

    // The token this service DID mint resumes the enumeration.
    let (_, first) = call(&router, "ListLFTags", &json!({ "MaxResults": 1 })).await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a first page of two tags carries a NextToken")
        .to_string();
    let (status, second) = call(&router, "ListLFTags", &json!({ "NextToken": token })).await;
    assert_eq!(status, 200, "resuming must succeed, got {status} {second:?}");
    let keys: Vec<&str> = second
        .get("LFTags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| tag.get("TagKey").and_then(Value::as_str).unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        keys,
        vec!["b"],
        "resuming after the first page must return exactly the rest, got {keys:?}"
    );
}

#[tokio::test]
async fn update_merges_additions_and_removals_into_the_stored_set() {
    // Rows differ along what is added, what is removed, order, repetition, and whether a
    // deletion names a value the tag does not hold.
    /// One row: tag key, the values it is created with, the values to add, the values to
    /// delete, and the value set `GetLFTag` must read back afterwards.
    type Row<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>);
    let cases: Vec<Row<'_>> = vec![
        // key, initial, to_add, to_delete, expected
        (
            "environment",
            vec!["dev", "prod"],
            vec!["staging"],
            vec!["dev"],
            vec!["prod", "staging"],
        ),
        // An addition goes on the end of the set, in the order it was named.
        ("reordered", vec!["m"], vec!["z", "a"], vec![], vec!["m", "z", "a"]),
        ("add-only", vec!["a"], vec!["b", "c"], vec![], vec!["a", "b", "c"]),
        ("delete-only", vec!["a", "b", "c"], vec![], vec!["b"], vec!["a", "c"]),
        // An addition already stored changes nothing: the values are a set.
        ("idempotent-add", vec!["a"], vec!["a"], vec![], vec!["a"]),
        // Repetitions within one request collapse.
        ("repeated-add", vec!["a"], vec!["b", "b"], vec![], vec!["a", "b"]),
        // Deleting a value the tag never held leaves the rest in place.
        ("absent-delete", vec!["a"], vec!["b"], vec!["zzz"], vec!["a", "b"]),
        // Every value replaced in one call, as long as one value survives.
        ("replace-all", vec!["a", "b"], vec!["c"], vec!["a", "b"], vec!["c"]),
    ];

    let router = emulator();
    for (key, initial, to_add, to_delete, expected) in &cases {
        create(&router, None, key, initial).await;

        let mut body = json!({ "TagKey": key });
        if !to_add.is_empty() {
            body["TagValuesToAdd"] = json!(to_add);
        }
        if !to_delete.is_empty() {
            body["TagValuesToDelete"] = json!(to_delete);
        }
        let (status, payload) = call(&router, "UpdateLFTag", &body).await;
        assert_eq!(
            status, 200,
            "case {key:?}: expected UpdateLFTag 200, got {status} {payload:?}"
        );

        let (_, payload) = call(&router, "GetLFTag", &json!({ "TagKey": key })).await;
        assert_eq!(
            tag_values(&payload),
            *expected,
            "case {key:?}: expected TagValues {expected:?}, got {:?}",
            tag_values(&payload)
        );
    }
}

#[tokio::test]
async fn update_refuses_a_change_that_would_leave_no_values_or_change_nothing() {
    // Rows: delete every value; delete every value one call at a time; neither list;
    // empty-but-present intent; and a merge past the 50 values `TagValueList` admits.
    let fifty: Vec<String> = (0..50).map(|i| format!("v{i}")).collect();
    let fifty_json: Vec<&str> = fifty.iter().map(String::as_str).collect();

    let cases: Vec<(&str, Vec<&str>, Value)> = vec![
        (
            "all-at-once",
            vec!["prod", "staging"],
            json!({ "TagValuesToDelete": ["prod", "staging"] }),
        ),
        ("last-one", vec!["only"], json!({ "TagValuesToDelete": ["only"] })),
        // Adds a value and deletes it again in the same call: nothing would remain.
        (
            "add-then-delete",
            vec!["a"],
            json!({ "TagValuesToAdd": ["b"], "TagValuesToDelete": ["a", "b"] }),
        ),
        ("no-change", vec!["a"], json!({})),
        // 50 stored + 1 added is past the modelled bound of `TagValueList`.
        ("too-many", fifty_json.clone(), json!({ "TagValuesToAdd": ["one-too-many"] })),
    ];

    let router = emulator();
    for (key, initial, change) in &cases {
        create(&router, None, key, initial).await;

        let mut body = json!({ "TagKey": key });
        if let Some(members) = change.as_object() {
            for (name, value) in members {
                body[name.as_str()] = value.clone();
            }
        }
        let (status, payload) = call(&router, "UpdateLFTag", &body).await;
        assert_eq!(
            status, 400,
            "case {key:?}: expected 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {key:?}: expected InvalidInputException, got {payload:?}"
        );

        // A refused update leaves the stored set exactly as it was.
        let (_, payload) = call(&router, "GetLFTag", &json!({ "TagKey": key })).await;
        assert_eq!(
            tag_values(&payload),
            *initial,
            "case {key:?}: a refused update must not mutate the tag, got {:?}",
            tag_values(&payload)
        );
    }
}

#[tokio::test]
async fn delete_removes_the_tag_from_every_reader() {
    // Rows differ along the catalog and along how many tags the catalog holds, so the
    // removal is observed both with and without siblings left behind.
    let foreign = "210987654321";
    let cases: [(Option<&str>, &str, Vec<&str>); 3] = [
        (None, "environment", vec!["environment", "team"]),
        (None, "only", vec!["only"]),
        (Some(foreign), "shared", vec!["shared", "other"]),
    ];

    for (catalog, victim, present) in &cases {
        let router = emulator();
        for key in present {
            create(&router, *catalog, key, &["v"]).await;
        }

        let mut body = json!({ "TagKey": victim });
        if let Some(catalog) = catalog {
            body["CatalogId"] = json!(catalog);
        }
        let (status, payload) = call(&router, "DeleteLFTag", &body).await;
        assert_eq!(
            status, 200,
            "case {catalog:?}/{victim:?}: expected DeleteLFTag 200, got {status} {payload:?}"
        );

        let (status, payload) = call(&router, "GetLFTag", &body).await;
        assert_eq!(
            status, 400,
            "case {catalog:?}/{victim:?}: GetLFTag after a delete must be 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "EntityNotFoundException",
            "case {catalog:?}/{victim:?}: expected EntityNotFoundException, got {payload:?}"
        );

        // Deleting it twice is the same refusal, not a silent success.
        let (status, payload) = call(&router, "DeleteLFTag", &body).await;
        assert_eq!(
            status, 400,
            "case {catalog:?}/{victim:?}: a second delete must be 400, got {status} {payload:?}"
        );

        let mut list = json!({});
        if let Some(catalog) = catalog {
            list["CatalogId"] = json!(catalog);
        }
        let (_, payload) = call(&router, "ListLFTags", &list).await;
        let keys: Vec<String> = payload
            .get("LFTags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .map(|tag| {
                        tag.get("TagKey")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<String> = present
            .iter()
            .filter(|key| *key != victim)
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            keys, expected,
            "case {catalog:?}/{victim:?}: expected ListLFTags to hold {expected:?}, got {keys:?}"
        );
    }
}

/// Seed `tags` LF-tags into `catalog` directly in the store the service serves, so a
/// ceiling can be exercised without thousands of round trips.
fn seed_catalog(engine: &Arc<engine::InMemoryEngine>, catalog_id: &str, tags: usize) {
    use service_com_amazonaws_lakeformation_awslakeformation::state::{
        ComAmazonawsLakeformationAwslakeformationState, LfTag,
    };

    let state = engine.state::<ComAmazonawsLakeformationAwslakeformationState>();
    let mut store = state.lock();
    let catalog = store.lf_tags.entry(catalog_id.to_string()).or_default();
    for i in 0..tags {
        let key = format!("seed-{i:05}");
        catalog.insert(
            key.clone(),
            LfTag {
                catalog_id: catalog_id.to_string(),
                tag_key: key,
                tag_values: vec!["v".to_string()],
            },
        );
    }
}

/// `CreateLFTag` of `key` in `catalog`, answering the wire status and error code.
async fn try_create(
    router: &MultiServiceRouter,
    catalog: Option<&str>,
    key: &str,
) -> (u16, String) {
    let mut body = json!({ "TagKey": key, "TagValues": ["v"] });
    if let Some(catalog) = catalog {
        body["CatalogId"] = json!(catalog);
    }
    let (status, payload) = call(router, "CreateLFTag", &body).await;
    (status, error_code(&payload))
}

#[tokio::test]
async fn create_refuses_a_store_that_is_already_at_one_of_its_ceilings() {
    use service_com_amazonaws_lakeformation_awslakeformation::state::{
        MAX_LF_TAGS_PER_CATALOG, MAX_LF_TAGS_TOTAL, MAX_LF_TAG_CATALOGS,
    };

    // The store has three ceilings, each refused with the same modelled error. The last
    // two are what bound it when the caller varies `CatalogId`, which is request-supplied.
    // Rows: (label, catalogs to seed, tags per seeded catalog, the create to attempt).
    let account = account_catalog();
    let cases: Vec<(&str, usize, usize, Option<String>)> = vec![
        // One catalog at its per-catalog ceiling: that catalog refuses.
        (
            "per-catalog ceiling",
            1,
            MAX_LF_TAGS_PER_CATALOG,
            Some(account.clone()),
        ),
        // Every catalog slot taken: a create naming a NEW catalog refuses, which is the
        // bypass a per-catalog-only bound would allow.
        (
            "catalog-count ceiling",
            MAX_LF_TAG_CATALOGS,
            1,
            Some("catalog-one-too-many".to_string()),
        ),
        // The store at its overall ceiling: a create in a fresh catalog refuses even
        // though neither that catalog nor the catalog count is full.
        (
            "store-wide ceiling",
            MAX_LF_TAGS_TOTAL / MAX_LF_TAGS_PER_CATALOG,
            MAX_LF_TAGS_PER_CATALOG,
            Some("another-catalog".to_string()),
        ),
    ];

    for (label, catalogs, tags_each, attempt_in) in &cases {
        let (router, engine) = emulator_with_engine();
        for c in 0..*catalogs {
            // The first seeded catalog is the account's own, so the per-catalog row
            // exercises the catalog a request addresses by default.
            let catalog_id = if c == 0 {
                account.clone()
            } else {
                format!("catalog-{c:04}")
            };
            seed_catalog(&engine, &catalog_id, *tags_each);
        }

        let (status, code) = try_create(&router, attempt_in.as_deref(), "one-too-many").await;
        assert_eq!(status, 400, "case {label}: expected 400, got {status}");
        assert_eq!(
            code, "ResourceNumberLimitExceededException",
            "case {label}: expected ResourceNumberLimitExceededException, got {code:?}"
        );

        // The refusal stored nothing.
        let mut get = json!({ "TagKey": "one-too-many" });
        if let Some(catalog) = attempt_in {
            get["CatalogId"] = json!(catalog);
        }
        let (status, payload) = call(&router, "GetLFTag", &get).await;
        assert_eq!(
            status, 400,
            "case {label}: the refused tag must not be stored, got {status} {payload:?}"
        );

        // And a slot freed by a delete is reusable: the ceilings bound the store, they
        // do not wedge it.
        let (status, payload) = call(
            &router,
            "DeleteLFTag",
            &json!({ "CatalogId": account, "TagKey": "seed-00000" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: expected DeleteLFTag 200, got {status} {payload:?}"
        );
        let (status, code) = try_create(&router, Some(&account), "now-fits").await;
        assert_eq!(
            status, 200,
            "case {label}: a create must fit after a delete freed a slot, got {status} {code:?}"
        );
    }
}

#[tokio::test]
async fn create_fits_below_the_per_catalog_ceiling() {
    use service_com_amazonaws_lakeformation_awslakeformation::state::MAX_LF_TAGS_PER_CATALOG;

    // One short of the ceiling is accepted, and the tag is readable: the bound refuses
    // only the create that would pass it.
    let (router, engine) = emulator_with_engine();
    seed_catalog(&engine, &account_catalog(), MAX_LF_TAGS_PER_CATALOG - 1);
    let (status, code) = try_create(&router, None, "last-slot").await;
    assert_eq!(
        status, 200,
        "case one below the ceiling: expected 200, got {status} {code:?}"
    );
    let (status, payload) = call(&router, "GetLFTag", &json!({ "TagKey": "last-slot" })).await;
    assert_eq!(
        status, 200,
        "case one below the ceiling: the tag must be readable, got {status} {payload:?}"
    );
}

#[tokio::test]
async fn state_is_shared_between_requests_not_per_request() {
    // The whole point of reaching the store through `crate::state::service_state`: a tag
    // created by one request is visible to the next, and two emulators are isolated.
    let first = emulator();
    create(&first, None, "environment", &["dev"]).await;
    let (status, _) = call(&first, "GetLFTag", &json!({ "TagKey": "environment" })).await;
    assert_eq!(status, 200, "the same emulator must see its own write");

    let second = emulator();
    let (status, payload) = call(&second, "GetLFTag", &json!({ "TagKey": "environment" })).await;
    assert_eq!(
        status, 400,
        "a second emulator holds no tags, got {status} {payload:?}"
    );
}
