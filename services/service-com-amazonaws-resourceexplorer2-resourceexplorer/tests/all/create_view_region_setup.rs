// SPDX-License-Identifier: CC0-1.0

//! `CreateView` and the Region setup it needs.
//!
//! The rule under test: turning Resource Explorer on in a Region is ACCOUNT setup, made
//! outside the operations this model declares, so a well-formed `CreateView` addressed
//! to a Region no call has acted on is SERVED — the Region is treated as one the account
//! already turned on, and the index and default view that implies are materialised with
//! the view. Only a Region this account itself turned off, or a credential scope naming
//! no Amazon Web Services Region, is refused, and then with the `ValidationException`
//! class as AWS words it rather than with instructions of the emulator's own.
//!
//! Every assertion goes over the wire: the suite mounts this service on a
//! [`MultiServiceRouter`] over one shared engine and POSTs restJson1 requests at it, so
//! nothing is proved about a handler that is not also proved about the serialized
//! request and response an SDK sends and receives. The Region of a request is the Region
//! of its SigV4 credential scope, which is why every call carries an `Authorization`
//! header.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

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

/// The `ViewArn` values `ListViews` reports in `region`, as a set.
async fn listed_views(emulator: &Emulator, region: &str, case: &str) -> BTreeSet<String> {
    let answer = emulator.call(region, "ListViews", json!({})).await;
    answer.ok(case)["Views"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|arn| arn.as_str().map(str::to_string))
        .collect()
}

/// The `Arn` values `ListIndexes` reports for `region`, as a set.
async fn listed_indexes(emulator: &Emulator, region: &str, case: &str) -> BTreeSet<String> {
    let answer = emulator
        .call(region, "ListIndexes", json!({ "Regions": [region] }))
        .await;
    answer.ok(case)["Indexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|index| index["Arn"].as_str().map(str::to_string))
        .collect()
}

/// One row of the table: a Region nothing has acted on, and the `CreateView` request
/// made in it.
struct Row {
    case: &'static str,
    region: &'static str,
    request: Value,
}

/// The rows span what the rule generalises over: the Region a request is signed for, and
/// every way a `CreateView` request can be shaped — the `ViewName` pattern's own bounds
/// (`^[a-zA-Z0-9\-]{1,64}$`, smithy_query: shape `ViewName`), with and without `Filters`,
/// `IncludedProperties`, `Tags`, `Scope` and the `@idempotencyToken` `ClientToken`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the minimal request",
            region: "us-east-1",
            request: json!({ "ViewName": "wire-probe-1" }),
        },
        Row {
            case: "a one-character name, the pattern's lower bound",
            region: "eu-west-1",
            request: json!({ "ViewName": "a" }),
        },
        Row {
            case: "a 64-character name, the pattern's upper bound",
            region: "ap-northeast-1",
            request: json!({ "ViewName": "b".repeat(64) }),
        },
        Row {
            case: "mixed case, digits and dashes",
            region: "ap-southeast-2",
            request: json!({ "ViewName": "View-With-MixedCase-99" }),
        },
        Row {
            case: "a filtered view",
            region: "sa-east-1",
            request: json!({
                "ViewName": "filtered-view",
                "Filters": { "FilterString": "region:sa-east-1" },
            }),
        },
        Row {
            case: "an idempotency token only",
            region: "us-west-2",
            request: json!({ "ViewName": "token-view", "ClientToken": "6c6f-token-1" }),
        },
        Row {
            case: "every optional member at once",
            region: "eu-central-1",
            request: json!({
                "ViewName": "everything-view",
                "Filters": { "FilterString": "service:resource-explorer-2" },
                "IncludedProperties": [{ "Name": "tags" }],
                "Tags": { "env": "prod", "team": "core" },
                "ClientToken": "6c6f-token-2",
                "Scope": engine::emulator_arn("iam", "", "root"),
            }),
        },
    ]
}

/// A well-formed `CreateView` in a Region nothing has acted on is SERVED with the stored
/// view, and the Region it creates the view in is really turned on: its index is there
/// and `ACTIVE`, it has the default view a query that names none uses, and a query
/// through that default view reports both the index and the new view.
///
/// Each row runs against its own freshly booted emulator, so the Region it names has
/// never been acted on by any call — which is the whole point of the rule.
#[tokio::test]
async fn create_view_serves_a_region_the_account_has_not_turned_off() {
    for row in rows() {
        let emulator = Emulator::new();
        let case = row.case;
        let region = row.region;

        let created = emulator.call(region, "CreateView", row.request.clone()).await;
        let view = created.ok(case)["View"].clone();
        let view_arn = view["ViewArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let requested_name = row.request["ViewName"].as_str().unwrap_or_default();
        assert_eq!(
            view["ViewName"].as_str(),
            Some(requested_name),
            "case {case:?}: the view carries the requested name, got {view}"
        );
        assert!(
            view_arn.contains(region) && view_arn.contains(requested_name),
            "case {case:?}: the view ARN names its Region and view, got {view_arn:?}"
        );
        assert_eq!(
            view["Owner"].as_str(),
            Some(engine::emulator_account_id()),
            "case {case:?}: the view is owned by this account, got {view}"
        );
        assert!(
            view["Scope"].as_str().is_some_and(|scope| !scope.is_empty()),
            "case {case:?}: a view always reports a Scope, got {view}"
        );
        assert!(
            !view["LastUpdatedAt"].is_null(),
            "case {case:?}: a view reports when it was last updated, got {view}"
        );

        // Persisted, not echoed: the reader answers the IDENTICAL structure.
        let read = emulator
            .call(region, "GetView", json!({ "ViewArn": view_arn }))
            .await;
        assert_eq!(
            read.ok(case)["View"],
            view,
            "case {case:?}: GetView must read the created view back unchanged, got {}",
            read.body
        );

        // The Region is turned on: its index is there, settled, and local.
        let index = emulator.call(region, "GetIndex", json!({})).await;
        let index_body = index.ok(case).clone();
        assert_eq!(
            index_body["State"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: setup the account already did is not work starting now, got \
             {index_body}"
        );
        assert_eq!(
            index_body["Type"].as_str(),
            Some("LOCAL"),
            "case {case:?}: a Region's own index is local, got {index_body}"
        );
        let index_arn = index_body["Arn"]
            .as_str()
            .expect("GetIndex answers an Arn")
            .to_string();
        assert_eq!(
            listed_indexes(&emulator, region, case).await,
            BTreeSet::from([index_arn.clone()]),
            "case {case:?}: the Region holds exactly the index it was turned on with"
        );

        // A Region that is turned on has the default view a query that names none uses,
        // and it is not the view this request created.
        let default_view = emulator.call(region, "GetDefaultView", json!({})).await;
        let default_arn = default_view.ok(case)["ViewArn"]
            .as_str()
            .expect("a Region that is turned on has a default view")
            .to_string();
        assert_ne!(
            default_arn, view_arn,
            "case {case:?}: the created view is not the Region's default"
        );
        assert_eq!(
            listed_views(&emulator, region, case).await,
            BTreeSet::from([default_arn.clone(), view_arn.clone()]),
            "case {case:?}: the Region holds the view it was turned on with and the new one"
        );

        // The view is BACKED by the Region's index: an unfiltered query through the
        // Region's default view reports both the index and the view just created.
        let searched = emulator
            .call(
                region,
                "Search",
                json!({ "QueryString": "", "ViewArn": default_arn }),
            )
            .await;
        let found: BTreeSet<String> = searched.ok(case)["Resources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|resource| resource["Arn"].as_str().map(str::to_string))
            .collect();
        assert!(
            found.contains(&index_arn) && found.contains(&view_arn),
            "case {case:?}: the Region's index must hold its index and the new view, got \
             {found:?}"
        );
    }
}

/// How the Region came to be turned on changes nothing about the answer: the same
/// request, made in a Region turned on by an explicit `CreateIndex` and in one nothing
/// has acted on, is answered with the same view and leaves the same two views behind.
#[tokio::test]
async fn an_untouched_region_answers_create_view_as_one_turned_on_by_hand_does() {
    let emulator = Emulator::new();
    let by_hand = "us-east-2";
    let untouched = "ap-south-1";
    emulator
        .call(by_hand, "CreateIndex", json!({ "ClientToken": "by-hand" }))
        .await
        .ok("CreateIndex in the Region turned on by hand");

    let request = json!({
        "ViewName": "comparable-view",
        "Filters": { "FilterString": "service:resource-explorer-2" },
        "IncludedProperties": [{ "Name": "tags" }],
        "Tags": { "env": "prod" },
    });
    let mut answers = Vec::new();
    for region in [by_hand, untouched] {
        let case = format!("CreateView in {region}");
        let answer = emulator.call(region, "CreateView", request.clone()).await;
        let view = answer.ok(&case)["View"].clone();
        assert_eq!(
            listed_views(&emulator, region, &case).await.len(),
            2,
            "case {case:?}: the Region holds the view it was turned on with and the new one"
        );
        answers.push((region.to_string(), view));
    }

    // Everything but the ARN, which carries its own Region and minted identifier, is
    // identical — the answer does not depend on who turned the Region on.
    let (left_region, left) = &answers[0];
    let (right_region, right) = &answers[1];
    for member in ["ViewName", "Owner", "Scope", "Filters", "IncludedProperties"] {
        assert_eq!(
            left[member], right[member],
            "case {member:?}: {left_region} and {right_region} must answer the same, got \
             {left} and {right}"
        );
    }
    for (region, view) in &answers {
        assert!(
            view["ViewArn"].as_str().is_some_and(|arn| arn.contains(region)),
            "case {region:?}: the view ARN names its own Region, got {view}"
        );
    }
}

/// Turning a Region on happens ONCE, and only for the Region the request is addressed
/// to: a second `CreateView` in the same Region adds a view and no second index, and a
/// Region nothing wrote in is still untouched — a read never turns one on.
#[tokio::test]
async fn create_view_turns_on_its_own_region_once() {
    let emulator = Emulator::new();
    let region = "eu-north-1";
    let other = "ca-central-1";

    let mut views = BTreeSet::new();
    for name in ["first-view", "second-view"] {
        let answer = emulator
            .call(region, "CreateView", json!({ "ViewName": name }))
            .await;
        views.insert(
            answer.ok(name)["View"]["ViewArn"]
                .as_str()
                .expect("CreateView answers a View.ViewArn")
                .to_string(),
        );
    }
    assert_eq!(
        listed_indexes(&emulator, region, "after two views").await.len(),
        1,
        "case \"after two views\": a Region is turned on once, not once per view"
    );
    let listed = listed_views(&emulator, region, "after two views").await;
    assert!(
        views.is_subset(&listed) && listed.len() == 3,
        "case \"after two views\": expected the default view and both new ones, got {listed:?}"
    );

    // The other Region was only ever READ, so it holds nothing: a report never
    // materialises setup.
    for operation in ["GetIndex", "ListIndexes", "ListViews", "GetDefaultView"] {
        emulator.call(other, operation, json!({})).await;
    }
    emulator
        .call(other, "GetIndex", json!({}))
        .await
        .failed("a Region only ever read", 404, "ResourceNotFoundException");
    assert!(
        listed_indexes(&emulator, other, "a Region only ever read")
            .await
            .is_empty(),
        "case \"a Region only ever read\": reading a Region must not turn it on"
    );
    assert!(
        listed_views(&emulator, other, "a Region only ever read")
            .await
            .is_empty(),
        "case \"a Region only ever read\": reading a Region must not give it a view"
    );
}

/// A request refused on one of its own MEMBERS never reaches the Region's state, so it
/// cannot turn a Region on: the pattern the `ViewName` member carries and the closed
/// `IncludedProperty$Name` catalog are both judged before that, and the Region stays
/// exactly as untouched as it was.
#[tokio::test]
async fn a_member_level_refusal_leaves_the_region_untouched() {
    // (case, Region, request, status, error code). Each Region is used once, so what the
    // row leaves behind is only ever its own refusal's doing.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a name outside the modelled pattern",
            "eu-west-2",
            json!({ "ViewName": "under_score" }),
            401,
            "UnauthorizedException",
        ),
        (
            "an IncludedProperties name outside the catalog",
            "eu-west-3",
            json!({
                "ViewName": "unsupported-property-view",
                "IncludedProperties": [{ "Name": "arn" }],
            }),
            400,
            "ValidationException",
        ),
    ];
    for (case, region, request, status, code) in rows {
        let emulator = Emulator::new();
        emulator
            .call(region, "CreateView", request)
            .await
            .failed(case, status, code);
        emulator
            .call(region, "GetIndex", json!({}))
            .await
            .failed(case, 404, "ResourceNotFoundException");
        assert!(
            listed_views(&emulator, region, case).await.is_empty(),
            "case {case:?}: a refused request must not turn the Region on"
        );
    }
}

/// Where the Region a request is addressed to can hold no index, `CreateView` is refused
/// with the modelled 400 `ValidationException` — and the refusal reads the way AWS words
/// that class: the token at fault where the request carries one, and the class's own
/// documented sentence where it does not. Nothing is stored either way.
#[tokio::test]
async fn create_view_is_refused_where_the_region_can_hold_no_index() {
    let emulator = Emulator::new();

    // A credential scope that is not an Amazon Web Services Region: grammar-shaped but
    // unknown, a bad suffix, a bare word, and a Region name with trailing text.
    for region in ["xx-west-1", "us-east-99", "not-a-region", "us-east-1-extra"] {
        let answer = emulator
            .call(region, "CreateView", json!({ "ViewName": "nowhere-view" }))
            .await;
        answer.failed(region, 400, "ValidationException");
        assert_eq!(
            answer.error_message(),
            format!("{region} is not an Amazon Web Services Region."),
            "case {region:?}: the refusal names the token at fault, got {}",
            answer.body
        );
    }

    // A Region this account TURNED OFF: the index record it left behind is what says the
    // account acted, so the Region is not served as one that was never touched.
    let region = "me-south-1";
    let index_arn = emulator
        .call(region, "CreateIndex", json!({ "ClientToken": "turned-off" }))
        .await
        .ok("CreateIndex")["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string();
    emulator
        .call(region, "DeleteIndex", json!({ "Arn": index_arn }))
        .await
        .ok("DeleteIndex");
    let answer = emulator
        .call(region, "CreateView", json!({ "ViewName": "orphan-view" }))
        .await;
    answer.failed("a Region the account turned off", 400, "ValidationException");
    assert_eq!(
        answer.error_message(),
        "You provided an invalid value for one of the operation's parameters. Check the \
         syntax for the operation, and try again.",
        "case \"a Region the account turned off\": the refusal carries the wording of its \
         class, got {}",
        answer.body
    );
    assert!(
        !listed_views(&emulator, region, "a Region the account turned off")
            .await
            .iter()
            .any(|arn| arn.contains("orphan-view")),
        "case \"a Region the account turned off\": a refused CreateView stores nothing"
    );
}
