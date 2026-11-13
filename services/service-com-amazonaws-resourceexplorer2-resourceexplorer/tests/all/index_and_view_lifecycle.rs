// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the Resource Explorer index/view/search slice.
//!
//! Every assertion here goes over the wire: the suite mounts this service on a
//! [`MultiServiceRouter`] over ONE shared engine and POSTs restJson1 requests at it,
//! so nothing is proved about a handler that is not also proved about the serialized
//! request and response an SDK sends and receives.
//!
//! The Region of a request is the Region of its SigV4 credential scope, which is why
//! every call carries an `Authorization` header: it is the only place the wire states
//! which Region "the Region in which you call this operation" means.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Comfortably past `crate::state::INDEX_SETTLE_DELAY`, so an index that was left in
/// `CREATING` or `DELETING` is certain to have settled by the next call.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// One emulator: the service mounted on a router over a single shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one call answered: the HTTP status, the restJson1 error code (the
/// `x-amzn-errortype` header), and the decoded JSON body.
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body of a call that had to succeed.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({:?}) with body {}",
            self.status, self.error, self.body
        );
        &self.body
    }

    /// Assert this call failed with `status` and the error code `code`.
    fn failed(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected HTTP {status} {code}, got {} ({:?}) with body {}",
            self.status,
            self.error,
            self.body
        );
    }

    /// The error's `Message` member, however the envelope cases it.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_resourceexplorer2_resourceexplorer::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// POST `/{operation}` as a client signed for `region` would.
    async fn call(&self, region: &str, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}\
                     /resource-explorer-2/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }
}

/// `CreateIndex` in `region`, asserting success, and the index ARN it answered with.
async fn create_index(emulator: &Emulator, region: &str, token: &str) -> String {
    let answer = emulator
        .call(region, "CreateIndex", json!({ "ClientToken": token }))
        .await;
    answer.ok(region)["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string()
}

/// The default view turning Resource Explorer on in `region` left there: a Region's
/// index is created with the view a query that names none uses, and that view is itself
/// one of the resources the Region's index holds.
async fn turn_on_view(emulator: &Emulator, region: &str) -> String {
    let answer = emulator.call(region, "GetDefaultView", json!({})).await;
    answer.ok(&format!("the default view of {region}"))["ViewArn"]
        .as_str()
        .expect("a Region that was turned on has a default view")
        .to_string()
}

/// `CreateView` in `region`, asserting success, and the view ARN it answered with.
async fn create_view(emulator: &Emulator, region: &str, body: Value) -> String {
    let case = format!("{region} {body}");
    let answer = emulator.call(region, "CreateView", body).await;
    answer.ok(&case)["View"]["ViewArn"]
        .as_str()
        .expect("CreateView answers a View.ViewArn")
        .to_string()
}

/// The string members of a JSON array, as a set.
fn string_set(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// CreateIndex
// ---------------------------------------------------------------------------

/// One index per Region, in EVERY Region: the second `CreateIndex` with a fresh token
/// conflicts, while replaying the creating token returns the same index. The row set
/// spans Regions in three partitions-worth of naming shapes to show the rule is keyed
/// on the request's Region, not on one value.
#[tokio::test]
async fn create_index_is_one_per_region_and_replays_its_client_token() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let mut arns = BTreeSet::new();

    for region in regions {
        let arn = create_index(&emulator, region, "token-for-the-first-index").await;
        assert!(
            arn.starts_with(&format!("arn:aws:resource-explorer-2:{region}:")),
            "case {region:?}: the index ARN must name the request's Region, got {arn:?}"
        );
        assert!(
            arn.contains(":index/"),
            "case {region:?}: the index ARN must name an index resource, got {arn:?}"
        );
        assert!(
            arns.insert(arn.clone()),
            "case {region:?}: every Region's index must have its own ARN, got a repeat of {arn:?}"
        );

        // The modelled first state of an asynchronous creation.
        let created = emulator
            .call(region, "CreateIndex", json!({ "ClientToken": "a-different-token" }))
            .await;
        created.failed(region, 409, "ConflictException");

        let replay = emulator
            .call(
                region,
                "CreateIndex",
                json!({ "ClientToken": "token-for-the-first-index" }),
            )
            .await;
        assert_eq!(
            replay.ok(region)["Arn"].as_str(),
            Some(arn.as_str()),
            "case {region:?}: replaying the creating ClientToken must return the same index, \
             got {}",
            replay.body
        );
    }

    // Each Region's index is listed once, and ListIndexes is account-wide.
    let listed = emulator.call("us-east-1", "ListIndexes", json!({})).await;
    let body = listed.ok("ListIndexes");
    let listed_arns: BTreeSet<String> = body["Indexes"]
        .as_array()
        .expect("ListIndexes answers Indexes")
        .iter()
        .map(|index| {
            assert_eq!(
                index["Type"].as_str(),
                Some("LOCAL"),
                "CreateIndex creates only a local index, got {index}"
            );
            index["Arn"]
                .as_str()
                .expect("every Index carries an Arn")
                .to_string()
        })
        .collect();
    assert_eq!(
        listed_arns, arns,
        "ListIndexes must report exactly the indexes that were created, got {listed_arns:?}"
    );

    // And the Regions filter narrows it to the Regions asked for.
    for count in 1..=regions.len() {
        let wanted: Vec<&str> = regions[..count].to_vec();
        let answer = emulator
            .call("us-east-1", "ListIndexes", json!({ "Regions": wanted }))
            .await;
        let returned = answer.ok("Regions filter")["Indexes"]
            .as_array()
            .expect("ListIndexes answers Indexes")
            .len();
        assert_eq!(
            returned, count,
            "case Regions={wanted:?}: expected {count} indexes, got {returned}"
        );
    }
}

/// The index's state is the one AWS models for an asynchronous create and delete:
/// `CREATING` right after the call, `ACTIVE` once the clock has settled it, `DELETING`
/// right after `DeleteIndex`, and then gone — taking the Region's views with it.
#[tokio::test]
async fn the_index_settles_from_creating_to_active_and_deleting_to_gone() {
    let emulator = Emulator::new();
    let region = "us-east-1";

    let created = emulator
        .call(region, "CreateIndex", json!({ "ClientToken": "settle-token" }))
        .await;
    assert_eq!(
        created.ok("create")["State"].as_str(),
        Some("CREATING"),
        "case create: a just-created index is still coming up, got {}",
        created.body
    );
    let arn = created.body["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string();

    tokio::time::sleep(PAST_SETTLE).await;
    let replay = emulator
        .call(region, "CreateIndex", json!({ "ClientToken": "settle-token" }))
        .await;
    assert_eq!(
        replay.ok("settled")["State"].as_str(),
        Some("ACTIVE"),
        "case settled: the clock owes the index ACTIVE, got {}",
        replay.body
    );

    let view_arn = create_view(&emulator, region, json!({ "ViewName": "doomed-view" })).await;

    let deleted = emulator
        .call(region, "DeleteIndex", json!({ "Arn": arn.clone() }))
        .await;
    assert_eq!(
        deleted.ok("delete")["State"].as_str(),
        Some("DELETING"),
        "case delete: deletion is asynchronous, got {}",
        deleted.body
    );
    // @idempotent: repeating the call reports the same state and does not push the
    // removal further out.
    let again = emulator
        .call(region, "DeleteIndex", json!({ "Arn": arn }))
        .await;
    assert_eq!(
        again.ok("delete twice")["LastUpdatedAt"],
        deleted.body["LastUpdatedAt"],
        "case delete twice: a repeated delete must not restart the deletion, got {}",
        again.body
    );

    tokio::time::sleep(PAST_SETTLE).await;
    let listed = emulator.call(region, "ListIndexes", json!({})).await;
    assert_eq!(
        listed.ok("after delete")["Indexes"].as_array().map(Vec::len),
        Some(0),
        "case after delete: the deleted index must be gone, got {}",
        listed.body
    );
    let views = emulator.call(region, "ListViews", json!({})).await;
    assert_eq!(
        views.ok("after delete")["Views"].as_array().map(Vec::len),
        Some(0),
        "case after delete: deleting an index deletes the Region's views, got {}",
        views.body
    );
    emulator
        .call(region, "GetView", json!({ "ViewArn": view_arn }))
        .await
        .failed("view of a deleted index", 401, "UnauthorizedException");

    // With the index gone a view cannot be created, and a new index can.
    emulator
        .call(region, "CreateView", json!({ "ViewName": "orphan-view" }))
        .await
        .failed("no index", 400, "ValidationException");
    let fresh = emulator
        .call(region, "CreateIndex", json!({ "ClientToken": "second-index" }))
        .await;
    fresh.ok("re-create");
}

/// A Region this account TURNED OFF holds no views and can be given none: the refusal
/// is the modelled 400, not a 501 and not a success. A Region nothing has ever acted on
/// is a different case — the account's own setup is what turns a Region on, so an
/// untouched Region is served (see the `create_view_region_setup` suite).
#[tokio::test]
async fn create_view_is_refused_in_a_region_whose_index_was_deleted() {
    let emulator = Emulator::new();
    for region in ["eu-west-1", "ap-northeast-1"] {
        let arn = create_index(&emulator, region, "to-be-turned-off").await;
        emulator
            .call(region, "DeleteIndex", json!({ "Arn": arn }))
            .await
            .ok(region);
        // Whether the teardown has settled or not, the Region was turned off by this
        // account and holds no index a view could be queried through.
        emulator
            .call(region, "CreateView", json!({ "ViewName": "no-index-here" }))
            .await
            .failed(region, 400, "ValidationException");
    }
}

/// An index belongs to an Amazon Web Services Region, so a credential scope naming
/// something that is not one gets the modelled 400 rather than an index.
#[tokio::test]
async fn create_index_refuses_a_region_that_does_not_exist() {
    let emulator = Emulator::new();
    // Rows: grammar-shaped but not a Region, a partition-ish prefix, a bare word, and a
    // Region name with trailing text. Each must be refused.
    for region in [
        "xx-west-1",
        "us-east-99",
        "not-a-region",
        "us-east-1-extra",
    ] {
        emulator
            .call(region, "CreateIndex", json!({ "ClientToken": "bad-region" }))
            .await
            .failed(region, 400, "ValidationException");
    }
    // And a real Region in the same emulator still works.
    create_index(&emulator, "sa-east-1", "good-region").await;
}

/// Views are bounded by the Region's "Views per Region" quota: the request past it is
/// the operation's modelled 402, and the view is not stored.
#[tokio::test]
async fn create_view_stops_at_the_views_per_region_quota() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, "quota").await;

    // The quota is the only bound on the stored view map, so fill it exactly. Turning
    // the Region on already left one view there, which counts against the quota like any
    // other.
    let quota = 100;
    for n in 0..quota - 1 {
        create_view(&emulator, region, json!({ "ViewName": format!("view-{n}") })).await;
    }
    emulator
        .call(region, "CreateView", json!({ "ViewName": "one-too-many" }))
        .await
        .failed("past the quota", 402, "ServiceQuotaExceededException");

    // Nothing was stored for the refused request, and freeing a slot lets one in.
    let first_page = emulator
        .call(region, "ListViews", json!({ "MaxResults": 1 }))
        .await;
    let body = first_page.ok("first page").clone();
    assert!(
        body["NextToken"].is_string(),
        "case after the quota: a full Region still pages, got {body}"
    );
    let victim = body["Views"][0]
        .as_str()
        .expect("the first page carries a view ARN")
        .to_string();
    emulator
        .call(region, "DeleteView", json!({ "ViewArn": victim }))
        .await
        .ok("free a slot");
    emulator
        .call(region, "CreateView", json!({ "ViewName": "now-it-fits" }))
        .await
        .ok("after freeing a slot");
}

// ---------------------------------------------------------------------------
// CreateView / GetView / DeleteView
// ---------------------------------------------------------------------------

/// Whatever shape a view is created in, `GetView` reads back the IDENTICAL structure
/// and the stored tag map — the view is persisted, not echoed.
///
/// The rows differ along everything `CreateView` can vary: the name's form and length,
/// whether a filter is attached, how many optional properties are included, and how
/// many tags there are.
#[tokio::test]
async fn every_created_view_round_trips_through_get_view() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, "round-trip").await;

    // `ViewName` is `^[a-zA-Z0-9\-]{1,64}$` (smithy_query: shape ViewName), so the rows
    // walk that pattern: one character, digits and dashes, and the full 64.
    let rows: Vec<Value> = vec![
        json!({ "ViewName": "a" }),
        json!({ "ViewName": "View-With-MixedCase-99" }),
        json!({ "ViewName": "a".repeat(64) }),
        json!({
            "ViewName": "filtered-view",
            "Filters": { "FilterString": "service:resource-explorer-2" },
        }),
        json!({
            "ViewName": "tagged-view",
            "Tags": { "env": "prod", "team": "core", "cost-centre": "42" },
        }),
        json!({
            "ViewName": "everything-view",
            "Filters": { "FilterString": "region:us-east-1 -tag:env=dev" },
            "IncludedProperties": [{ "Name": "tags" }],
            "Scope": "arn:aws:iam::000000000000:root",
            "Tags": { "env": "dev" },
        }),
    ];

    for body in rows {
        let case = body.to_string();
        let created = emulator.call(region, "CreateView", body.clone()).await;
        let created_view = created.ok(&case)["View"].clone();

        assert_eq!(
            created_view["ViewName"].as_str(),
            body["ViewName"].as_str(),
            "case {case}: the created view must carry the requested name, got {created_view}"
        );
        assert_eq!(
            created_view["Owner"].as_str(),
            Some(engine::emulator_account_id()),
            "case {case}: the view's Owner is the calling account, got {created_view}"
        );
        assert!(
            created_view["LastUpdatedAt"].is_string() || created_view["LastUpdatedAt"].is_number(),
            "case {case}: the view must carry a LastUpdatedAt, got {created_view}"
        );
        // The account root ARN of the calling account, which is the documented default.
        let account_root = format!("arn:aws:iam::{}:root", engine::emulator_account_id());
        let expected_scope = body["Scope"].as_str().unwrap_or(&account_root);
        assert_eq!(
            created_view["Scope"].as_str(),
            Some(expected_scope),
            "case {case}: Scope defaults to the calling account, got {created_view}"
        );

        let arn = created_view["ViewArn"]
            .as_str()
            .expect("CreateView answers a View.ViewArn");
        let fetched = emulator
            .call(region, "GetView", json!({ "ViewArn": arn }))
            .await;
        let fetched_body = fetched.ok(&case).clone();
        assert_eq!(
            fetched_body["View"], created_view,
            "case {case}: GetView must return the view CreateView stored, got {fetched_body}"
        );
        let expected_tags = body.get("Tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            fetched_body["Tags"], expected_tags,
            "case {case}: GetView must return the stored tag map, got {fetched_body}"
        );
    }

    // An ARN nothing created resolves to no view, which is an authorization failure —
    // never a 501, never an empty success, and never a report that the view is absent.
    for arn in [
        "arn:aws:resource-explorer-2:us-east-1:000000000000:view/nope/00000000-0000-0000-0000-000000000000",
        "arn:aws:resource-explorer-2:eu-west-1:000000000000:view/also-nope/1",
        "not-even-an-arn",
    ] {
        let answer = emulator
            .call(region, "GetView", json!({ "ViewArn": arn }))
            .await;
        answer.failed(arn, 401, "UnauthorizedException");
        assert_eq!(
            answer.error_message(),
            "Unauthorized",
            "case {arn:?}: the refusal carries nothing from the request, got {}",
            answer.body
        );
    }
}

/// A view name is unique within its Region — and only within its Region. The
/// comparison is exact: the modelled `ViewName` pattern admits mixed case, and AWS
/// documents only that the name is unique within the Region.
#[tokio::test]
async fn view_names_are_unique_per_region() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1"];
    for region in regions {
        create_index(&emulator, region, "unique-names").await;
    }

    let mut arns = BTreeSet::new();
    for region in regions {
        for name in ["shared-name", "another-name"] {
            let arn = create_view(&emulator, region, json!({ "ViewName": name })).await;
            assert!(
                arns.insert(arn.clone()),
                "case {region:?}/{name:?}: each view needs its own ARN, got a repeat of {arn:?}"
            );
            // The same name in the same Region conflicts.
            emulator
                .call(region, "CreateView", json!({ "ViewName": name }))
                .await
                .failed(&format!("{region}/{name}"), 409, "ConflictException");
        }
    }
    assert_eq!(
        arns.len(),
        4,
        "two names in each of two Regions are four distinct views, got {arns:?}"
    );
}

/// Replaying `CreateView`'s `@idempotencyToken` returns the view it created; reusing it
/// for a different request is the conflict the model documents.
#[tokio::test]
async fn create_view_replays_its_client_token() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, "idempotent-views").await;

    let request = json!({
        "ViewName": "idempotent-view",
        "ClientToken": "a-client-token",
        "Tags": { "env": "prod" },
    });
    let first = emulator.call(region, "CreateView", request.clone()).await;
    let created = first.ok("first")["View"].clone();
    let replay = emulator.call(region, "CreateView", request).await;
    assert_eq!(
        replay.ok("replay")["View"],
        created,
        "case replay: the same ClientToken and parameters must return the same view, got {}",
        replay.body
    );

    emulator
        .call(
            region,
            "CreateView",
            json!({ "ViewName": "different-view", "ClientToken": "a-client-token" }),
        )
        .await
        .failed("token reused for other parameters", 409, "ConflictException");

    let views = emulator.call(region, "ListViews", json!({})).await;
    assert_eq!(
        string_set(&views.ok("after replay")["Views"]).len(),
        // The view the Region was turned on with, plus the one created here.
        2,
        "case after replay: an idempotent replay must not create a second view, got {}",
        views.body
    );
}

/// `DeleteView` removes the view from the store: its ARN stops resolving, so `GetView`
/// and a second `DeleteView` are both refused as authorization failures, and `ListViews`
/// no longer carries it.
#[tokio::test]
async fn deleting_a_view_removes_it_everywhere() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, "deletions").await;
    let turn_on = turn_on_view(&emulator, region).await;

    let keep = create_view(&emulator, region, json!({ "ViewName": "keeper" })).await;
    let doomed = create_view(&emulator, region, json!({ "ViewName": "doomed" })).await;

    let deleted = emulator
        .call(region, "DeleteView", json!({ "ViewArn": doomed.clone() }))
        .await;
    assert_eq!(
        deleted.ok("delete")["ViewArn"].as_str(),
        Some(doomed.as_str()),
        "case delete: DeleteView answers the ARN it deleted, got {}",
        deleted.body
    );

    emulator
        .call(region, "GetView", json!({ "ViewArn": doomed.clone() }))
        .await
        .failed("get after delete", 401, "UnauthorizedException");
    emulator
        .call(region, "DeleteView", json!({ "ViewArn": doomed.clone() }))
        .await
        .failed("delete twice", 401, "UnauthorizedException");

    let listed = emulator.call(region, "ListViews", json!({})).await;
    assert_eq!(
        string_set(&listed.ok("after delete")["Views"]),
        BTreeSet::from([turn_on.clone(), keep.clone()]),
        "case after delete: only the surviving views may be listed, got {}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// ListViews pagination
// ---------------------------------------------------------------------------

/// Paging is lossless at EVERY page size: following `NextToken` from the first page
/// yields each view exactly once, in the same order, whatever `MaxResults` is — and the
/// listing is scoped to the Region the call was made in.
#[tokio::test]
async fn list_views_pages_through_every_view_at_every_page_size() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let other_region = "eu-west-1";
    create_index(&emulator, region, "paging").await;
    create_index(&emulator, other_region, "paging").await;

    let mut expected = Vec::new();
    // The view the Region was turned on with is listed like any other.
    expected.push(turn_on_view(&emulator, region).await);
    for n in 0..7 {
        expected.push(create_view(&emulator, region, json!({ "ViewName": format!("view-{n}") })).await);
    }
    expected.sort();
    let elsewhere = create_view(&emulator, other_region, json!({ "ViewName": "elsewhere" })).await;

    for page_size in 1..=9_usize {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let answer = emulator.call(region, "ListViews", request).await;
            let body = answer.ok(&format!("page size {page_size}")).clone();
            let page = body["Views"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page may not exceed MaxResults, got {}",
                page.len()
            );
            seen.extend(page.iter().filter_map(|arn| arn.as_str().map(str::to_string)));
            token = body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case page size {page_size}: paging must terminate, saw {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: paging must yield every view of the Region exactly \
             once, got {seen:?}"
        );
        assert!(
            !seen.contains(&elsewhere),
            "case page size {page_size}: another Region's view must not be listed, got {seen:?}"
        );
    }

    // A token this listing never issued is a bad request, not a silent first page.
    for token in ["not-a-token", "00", "4c6973745669657773ffff"] {
        emulator
            .call(region, "ListViews", json!({ "NextToken": token }))
            .await
            .failed(token, 400, "ValidationException");
    }
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// A query is answered from the Region's index, and `Count` is taken from the same
/// match as `Resources` — so the two can never disagree.
///
/// The rows span the documented query grammar: the empty query, free-form text (in both
/// cases), each supported filter prefix, a `*` wildcard, negation, and two terms ANDed.
#[tokio::test]
async fn search_answers_from_the_index_with_a_count_that_matches() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let index_arn = create_index(&emulator, region, "searching").await;
    // The rows below include the documented tag filters, and a view can only answer
    // those when it includes the `tags` property.
    let alpha = create_view(
        &emulator,
        region,
        json!({
            "ViewName": "alpha-view",
            "Tags": { "env": "prod" },
            "IncludedProperties": [{ "Name": "tags" }],
        }),
    )
    .await;
    let beta = create_view(
        &emulator,
        region,
        json!({ "ViewName": "beta-view", "Tags": { "env": "dev", "team": "core" } }),
    )
    .await;

    let account = engine::emulator_account_id();
    let turn_on = turn_on_view(&emulator, region).await;
    let everything = BTreeSet::from([
        index_arn.clone(),
        turn_on.clone(),
        alpha.clone(),
        beta.clone(),
    ]);
    let views_only = BTreeSet::from([turn_on.clone(), alpha.clone(), beta.clone()]);

    // (query, the ARNs it must match). Filter names are those of the Resource Explorer
    // search query syntax reference.
    let rows: Vec<(String, BTreeSet<String>)> = vec![
        ("".to_string(), everything.clone()),
        ("alpha".to_string(), BTreeSet::from([alpha.clone()])),
        ("ALPHA".to_string(), BTreeSet::from([alpha.clone()])),
        ("nosuchresource".to_string(), BTreeSet::new()),
        (
            "service:resource-explorer-2".to_string(),
            everything.clone(),
        ),
        (
            "resourcetype:resource-explorer-2:view".to_string(),
            views_only.clone(),
        ),
        (
            "resourcetype:resource-explorer-2:index".to_string(),
            BTreeSet::from([index_arn.clone()]),
        ),
        (
            "-resourcetype:resource-explorer-2:index".to_string(),
            views_only.clone(),
        ),
        (format!("region:{region}"), everything.clone()),
        ("region:us*".to_string(), everything.clone()),
        ("region:eu-west-1".to_string(), BTreeSet::new()),
        (format!("accountid:{account}"), everything.clone()),
        ("tag:env=prod".to_string(), BTreeSet::from([alpha.clone()])),
        ("tag.key:team".to_string(), BTreeSet::from([beta.clone()])),
        ("tag.value:dev".to_string(), BTreeSet::from([beta.clone()])),
        ("globalresourcetype:resource-explorer-2:view".to_string(), BTreeSet::new()),
        (
            "resourcetype:resource-explorer-2:view -alpha".to_string(),
            BTreeSet::from([turn_on.clone(), beta.clone()]),
        ),
        (
            "resourcetype:resource-explorer-2:view beta".to_string(),
            BTreeSet::from([beta.clone()]),
        ),
    ];

    for (query, expected) in rows {
        let answer = emulator
            .call(
                region,
                "Search",
                json!({ "QueryString": query, "ViewArn": alpha }),
            )
            .await;
        let body = answer.ok(&query).clone();
        let returned: BTreeSet<String> = body["Resources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|resource| resource["Arn"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            returned, expected,
            "case {query:?}: expected {expected:?}, got {body}"
        );
        assert_eq!(
            body["Count"]["TotalResources"].as_u64(),
            Some(expected.len() as u64),
            "case {query:?}: Count must agree with the resources returned, got {body}"
        );
        assert_eq!(
            body["Count"]["Complete"].as_bool(),
            Some(true),
            "case {query:?}: a count below the result limit is complete, got {body}"
        );
        assert_eq!(
            body["ViewArn"].as_str(),
            Some(alpha.as_str()),
            "case {query:?}: Search reports the view it used, got {body}"
        );
    }

    // Every resource is described from the store, not from the request.
    let answer = emulator
        .call(
            region,
            "Search",
            json!({ "QueryString": "alpha", "ViewArn": alpha }),
        )
        .await;
    let resource = answer.ok("described")["Resources"][0].clone();
    assert_eq!(
        (
            resource["Arn"].as_str(),
            resource["Region"].as_str(),
            resource["Service"].as_str(),
            resource["ResourceType"].as_str(),
            resource["OwningAccountId"].as_str(),
        ),
        (
            Some(alpha.as_str()),
            Some(region),
            Some("resource-explorer-2"),
            Some("resource-explorer-2:view"),
            Some(account),
        ),
        "a searched resource is described from its stored ARN, got {resource}"
    );
    assert!(
        resource["LastReportedAt"].is_string() || resource["LastReportedAt"].is_number(),
        "a searched resource carries when the index last saw it, got {resource}"
    );

    // `beta` includes no optional properties, so a resource it reports carries none.
    let answer = emulator
        .call(
            region,
            "Search",
            json!({ "QueryString": "beta", "ViewArn": beta }),
        )
        .await;
    let resource = answer.ok("described through beta")["Resources"][0].clone();
    assert!(
        resource.get("Properties").is_none() || resource["Properties"].is_null(),
        "a view that includes no optional properties returns none, got {resource}"
    );
}

/// A malformed, degenerate or very large query is still answered — the grammar has no
/// recursion and no backtracking, so every one of these terminates with a `Count` that
/// agrees with the resources returned.
#[tokio::test]
async fn search_answers_degenerate_queries() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let index_arn = create_index(&emulator, region, "degenerate").await;
    // Some rows below are degenerate TAG filters, which a view can only answer when it
    // includes the `tags` property.
    let alpha = create_view(
        &emulator,
        region,
        json!({
            "ViewName": "alpha-view",
            "Tags": { "env": "prod" },
            "IncludedProperties": [{ "Name": "tags" }],
        }),
    )
    .await;
    let beta = create_view(
        &emulator,
        region,
        json!({ "ViewName": "beta-view", "Tags": { "env": "dev" } }),
    )
    .await;
    let turn_on = turn_on_view(&emulator, region).await;
    let everything = BTreeSet::from([index_arn, turn_on, alpha.clone(), beta.clone()]);
    // Only the two views the rows tagged carry a tag at all.
    let tagged = BTreeSet::from([alpha.clone(), beta.clone()]);

    let rows: Vec<(String, BTreeSet<String>)> = vec![
        // A filter name with no value: nothing equals the empty string.
        ("tag:".to_string(), BTreeSet::new()),
        // A bare separator is not a filter name, so it is text — and every ARN has one.
        (":".to_string(), everything.clone()),
        // A negation with nothing to negate, and a negated match-everything.
        ("-".to_string(), everything.clone()),
        ("-:".to_string(), BTreeSet::new()),
        // Wildcard-only values: a bare prefix match, which only tagged resources have.
        ("tag.key:*".to_string(), tagged.clone()),
        ("tag.value:*".to_string(), tagged.clone()),
        // Wildcards only, as free text.
        ("***".to_string(), everything.clone()),
        // A whitespace-only query has no terms at all.
        ("   ".to_string(), everything.clone()),
        // Many terms (98 of them, inside the 1280-character bound on QueryString).
        ("-zzzznomatch ".repeat(98), everything.clone()),
        // One very long term, just inside the same bound.
        ("z".repeat(1279), BTreeSet::new()),
    ];

    for (query, expected) in rows {
        let case = format!("{} chars starting {:?}", query.len(), &query[..query.len().min(24)]);
        let answer = emulator
            .call(
                region,
                "Search",
                json!({ "QueryString": query, "ViewArn": alpha }),
            )
            .await;
        let body = answer.ok(&case).clone();
        let returned: BTreeSet<String> = body["Resources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|resource| resource["Arn"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            returned, expected,
            "case {case}: expected {expected:?}, got {body}"
        );
        assert_eq!(
            body["Count"]["TotalResources"].as_u64(),
            Some(expected.len() as u64),
            "case {case}: Count must agree with the resources returned, got {body}"
        );
    }

    // Over-length input is refused by the modelled constraint, so it never reaches the
    // grammar: QueryString is `len 0..1280` and SearchFilter$FilterString `len 0..2048`.
    emulator
        .call(
            region,
            "Search",
            json!({ "QueryString": "z".repeat(1281), "ViewArn": alpha }),
        )
        .await
        .failed("QueryString past 1280", 400, "ValidationException");
    emulator
        .call(
            region,
            "CreateView",
            json!({
                "ViewName": "over-long-filter",
                "Filters": { "FilterString": "z".repeat(2049) },
            }),
        )
        .await
        .failed("FilterString past 2048", 400, "ValidationException");
}

/// Pages of `Search` results are lossless too, and `Count` stays the WHOLE match rather
/// than the size of the page.
#[tokio::test]
async fn search_pages_without_changing_its_count() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, "search-paging").await;
    let view = create_view(&emulator, region, json!({ "ViewName": "pager" })).await;
    for n in 0..5 {
        create_view(&emulator, region, json!({ "ViewName": format!("extra-{n}") })).await;
    }
    // The index, the view the Region was turned on with, the pager view and five extras.
    let total = 8_u64;

    for page_size in 1..=9_usize {
        let mut seen = BTreeSet::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({
                "QueryString": "",
                "ViewArn": view,
                "MaxResults": page_size,
            });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let answer = emulator.call(region, "Search", request).await;
            let body = answer.ok(&format!("page size {page_size}")).clone();
            assert_eq!(
                body["Count"]["TotalResources"].as_u64(),
                Some(total),
                "case page size {page_size}: Count is the whole match, not the page, got {body}"
            );
            for resource in body["Resources"].as_array().cloned().unwrap_or_default() {
                let arn = resource["Arn"]
                    .as_str()
                    .expect("every resource carries an Arn")
                    .to_string();
                assert!(
                    seen.insert(arn.clone()),
                    "case page size {page_size}: {arn:?} was returned on two pages"
                );
            }
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen.len() as u64,
            total,
            "case page size {page_size}: paging must yield every match, got {seen:?}"
        );
    }

    // A token is bound to the request it was issued for, so replaying it against a
    // different QueryString is refused rather than answered with a slice of another
    // result set.
    let first = emulator
        .call(
            region,
            "Search",
            json!({ "QueryString": "", "ViewArn": view, "MaxResults": 1 }),
        )
        .await;
    let token = first.ok("issue a token")["NextToken"]
        .as_str()
        .expect("a first page of one leaves a NextToken")
        .to_string();
    emulator
        .call(
            region,
            "Search",
            json!({
                "QueryString": "pager",
                "ViewArn": view,
                "MaxResults": 1,
                "NextToken": token,
            }),
        )
        .await
        .failed("token from another query", 400, "ValidationException");
}

/// A query needs a view: the one the caller names, else the Region's default. Both ways
/// of failing to resolve one are the SAME authorization failure — "if the Region either
/// doesn't have a default view or if you don't have permission to use the default view,
/// then the operation fails with a 401 Unauthorized exception" (`ViewArn`'s own
/// documentation) — and a view in another Region cannot be the default for this one.
#[tokio::test]
async fn search_resolves_the_view_it_is_told_to_use() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let other = "eu-west-1";
    create_index(&emulator, region, "defaults").await;
    create_index(&emulator, other, "defaults").await;
    let view = create_view(&emulator, region, json!({ "ViewName": "the-default" })).await;
    let foreign = create_view(&emulator, other, json!({ "ViewName": "foreign" })).await;

    // Turning Resource Explorer on in a Region left that Region with a default view, so
    // having none is a state the Region has to be CLEARED into.
    emulator
        .call(region, "DisassociateDefaultView", json!({}))
        .await
        .ok("clear the Region's default view");
    emulator
        .call(region, "Search", json!({ "QueryString": "" }))
        .await
        .failed("no default view", 401, "UnauthorizedException");
    emulator
        .call(
            region,
            "Search",
            json!({
                "QueryString": "",
                "ViewArn": "arn:aws:resource-explorer-2:us-east-1:000000000000:view/ghost/1",
            }),
        )
        .await
        .failed("unknown view", 401, "UnauthorizedException");
    emulator
        .call(region, "AssociateDefaultView", json!({ "ViewArn": foreign }))
        .await
        .failed("foreign default", 400, "ValidationException");
    emulator
        .call(
            region,
            "AssociateDefaultView",
            json!({ "ViewArn": "arn:aws:resource-explorer-2:us-east-1:000000000000:view/ghost/1" }),
        )
        .await
        .failed("unknown default", 404, "ResourceNotFoundException");

    let associated = emulator
        .call(
            region,
            "AssociateDefaultView",
            json!({ "ViewArn": view.clone() }),
        )
        .await;
    assert_eq!(
        associated.ok("associate")["ViewArn"].as_str(),
        Some(view.as_str()),
        "case associate: AssociateDefaultView answers the ARN it stored, got {}",
        associated.body
    );

    let searched = emulator
        .call(region, "Search", json!({ "QueryString": "" }))
        .await;
    assert_eq!(
        searched.ok("default view")["ViewArn"].as_str(),
        Some(view.as_str()),
        "case default view: Search without ViewArn uses the Region's default, got {}",
        searched.body
    );

    // Deleting the default leaves the Region with no default again.
    emulator
        .call(region, "DeleteView", json!({ "ViewArn": view }))
        .await
        .ok("delete default");
    emulator
        .call(region, "Search", json!({ "QueryString": "" }))
        .await
        .failed("default deleted", 401, "UnauthorizedException");
}

/// The results are the INTERSECTION of the query and the view's own filter, and a view
/// that includes the `tags` property gets it in the results.
#[tokio::test]
async fn the_view_filter_intersects_the_query_string() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, "filters").await;
    let prod = create_view(
        &emulator,
        region,
        json!({ "ViewName": "prod-view", "Tags": { "env": "prod" } }),
    )
    .await;
    create_view(
        &emulator,
        region,
        json!({ "ViewName": "dev-view", "Tags": { "env": "dev" } }),
    )
    .await;

    // A view that only ever shows prod-tagged resources, with tags included.
    let filtered = create_view(
        &emulator,
        region,
        json!({
            "ViewName": "only-prod",
            "Filters": { "FilterString": "tag:env=prod" },
            "IncludedProperties": [{ "Name": "tags" }],
        }),
    )
    .await;

    // (query, expected ARNs) through the filtered view: the filter is ANDed in, so no
    // query can widen the view past it.
    let rows: Vec<(&str, BTreeSet<String>)> = vec![
        ("", BTreeSet::from([prod.clone()])),
        ("prod-view", BTreeSet::from([prod.clone()])),
        ("dev-view", BTreeSet::new()),
        ("resourcetype:resource-explorer-2:index", BTreeSet::new()),
        ("tag:env=dev", BTreeSet::new()),
    ];
    for (query, expected) in rows {
        let answer = emulator
            .call(
                region,
                "Search",
                json!({ "QueryString": query, "ViewArn": filtered }),
            )
            .await;
        let body = answer.ok(query).clone();
        let returned: BTreeSet<String> = body["Resources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|resource| resource["Arn"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            returned, expected,
            "case {query:?}: the view filter must intersect the query, got {body}"
        );
        assert_eq!(
            body["Count"]["TotalResources"].as_u64(),
            Some(expected.len() as u64),
            "case {query:?}: Count must agree with the resources returned, got {body}"
        );
    }

    // IncludedProperties: tags come back as a property of the matched resource.
    let answer = emulator
        .call(
            region,
            "Search",
            json!({ "QueryString": "", "ViewArn": filtered }),
        )
        .await;
    let resource = answer.ok("included properties")["Resources"][0].clone();
    assert_eq!(
        resource["Properties"][0]["Name"].as_str(),
        Some("tags"),
        "a view that includes `tags` reports them, got {resource}"
    );
    assert_eq!(
        resource["Properties"][0]["Data"],
        json!({ "env": "prod" }),
        "the reported tags are the stored ones, got {resource}"
    );
}
