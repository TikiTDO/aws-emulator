// SPDX-License-Identifier: CC0-1.0

//! Application Components of an app version, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: `CreateAppVersionAppComponent` persists the Application
//! Component the request describes in the addressed application's DRAFT version,
//! keyed by its `id` (the caller's, or the component's own `name` when the request
//! names none), and reports it with that draft version's id; a later
//! `DescribeAppVersionAppComponent` of the same `appArn`/`appVersion`/`id` returns
//! that stored component member-for-member, and refuses with the modelled 404 for
//! each link of the path that does not resolve.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    DRAFT_APP_VERSION, MAX_APP_COMPONENTS_PER_APP_VERSION,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-east-1";

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests
    // the operations, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype`; empty on success.
    error_code: String,
    body: Value,
}

impl Reply {
    /// The response body, asserting the call succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?} {:?}",
            self.status, self.error_code, self.body
        );
        &self.body
    }

    /// Assert the call was refused with `status` and `code`.
    fn refused(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {:?} {:?}",
            self.status,
            self.error_code,
            self.body
        );
    }
}

/// POST one request through the router, exactly as the server front door does.
async fn post(router: &MultiServiceRouter, target: &str, body: &Value) -> Reply {
    let payload = serde_json::to_vec(body).expect("serializable body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://resiliencehub.{REGION}.amazonaws.com{target}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/resiliencehub/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&raw)}))
    };
    Reply {
        status,
        error_code,
        body,
    }
}

/// An application to hang Application Components off, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// A row of the component table: a `CreateAppVersionAppComponent` body (minus the
/// `appArn`, which the test fills in) spanning what the rule generalises over —
/// whether the caller names the `id`, the `type`'s form, the `name`'s length, and
/// whether `additionalInfo` is absent, a single entry or a full-width one.
struct Component {
    label: &'static str,
    request: fn() -> Value,
}

/// A `name`/`id` at the top of `String255`'s modelled `length` range (1..255, from
/// `smithy_query query="operation" operation="CreateAppVersionAppComponent"`).
fn max_length_name() -> String {
    format!("c{}", "x".repeat(254))
}

/// The table: one row per form of the request the rule has to hold for.
fn component_table() -> Vec<Component> {
    vec![
        Component {
            label: "only the required name and type, id left to the server",
            request: || {
                json!({
                    "name": "compute-tier",
                    // One of the Application Component types the Resilience Hub
                    // user guide's "Grouping resources in an AppComponent" lists:
                    // the member is free-form `String255` in the model, and the
                    // service's own component-type namespace is what a value has to
                    // name.
                    "type": "AWS::ResilienceHub::ComputeAppComponent",
                })
            },
        },
        Component {
            label: "a caller-supplied id",
            request: || {
                json!({
                    "id": "database-tier",
                    "name": "database-tier",
                    "type": "AWS::ResilienceHub::DatabaseAppComponent",
                })
            },
        },
        Component {
            label: "an id and name that are not each other",
            request: || {
                json!({
                    "id": "ac-7",
                    "name": "storage-tier",
                    "type": "AWS::ResilienceHub::StorageAppComponent",
                })
            },
        },
        Component {
            label: "an id carrying punctuation the EntityName pattern does not admit",
            request: || {
                json!({
                    "id": "custom.tier/1",
                    "name": "a custom tier",
                    "type": "AWS::ResilienceHub::MessageQueueAppComponent",
                })
            },
        },
        Component {
            label: "one additionalInfo entry",
            request: || {
                json!({
                    "id": "networking-tier",
                    "name": "networking-tier",
                    "type": "AWS::ResilienceHub::NetworkingAppComponent",
                    // The one key AWS documents for `additionalInfo`.
                    "additionalInfo": {
                        "failover-regions": ["[{\"region\":\"us-west-2\",\"accounts\":[{\"id\":\"111122223333\"}]}]"],
                    },
                })
            },
        },
        Component {
            label: "additionalInfo at the top of AdditionalInfoValueList's 1..10 length",
            request: || {
                json!({
                    "id": "queue-tier",
                    "name": "queue-tier",
                    "type": "AWS::ResilienceHub::MessageQueueAppComponent",
                    "additionalInfo": {
                        "one": (1..=10).map(|n| format!("value-{n}")).collect::<Vec<_>>(),
                        "two": ["only-one"],
                    },
                })
            },
        },
        Component {
            label: "a name at the top of String255's 1..255 length",
            request: || {
                json!({
                    "id": "long-name",
                    "name": max_length_name(),
                    "type": "AWS::ResilienceHub::AppCommonAppComponent",
                })
            },
        },
        Component {
            label: "an id at the top of String255's 1..255 length",
            request: || {
                json!({
                    "id": max_length_name(),
                    "name": "long-id",
                    "type": "AWS::ResilienceHub::ComputeAppComponent",
                })
            },
        },
    ]
}

/// What the response's `appComponent` should be for `request`, once `id` is known:
/// every member the request carried, unchanged, plus the id, plus the component's
/// `additionalInfo` map — empty rather than absent when the request gave none.
fn expected_component(request: &Value, id: &str) -> Value {
    json!({
        "name": request["name"].clone(),
        "type": request["type"].clone(),
        "id": id,
        "additionalInfo": request
            .get("additionalInfo")
            .cloned()
            .unwrap_or_else(|| json!({})),
    })
}

/// The create/describe round trip, over the whole table: each row's component is
/// persisted in the draft version under its id and read back member-for-member.
#[tokio::test]
async fn each_created_app_component_is_read_back_from_the_draft_version() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avac-roundtrip").await;

    for row in component_table() {
        let case = row.label;
        let mut request = (row.request)();
        request["appArn"] = json!(app_arn);

        let created = post(&router, "/create-app-version-app-component", &request).await;
        let created = created.ok(case).clone();

        assert_eq!(
            created["appArn"],
            json!(app_arn),
            "case {case:?}: the response must echo the addressed appArn, got {created:?}"
        );
        assert_eq!(
            created["appVersion"],
            json!(DRAFT_APP_VERSION),
            "case {case:?}: a create updates the draft version, got {created:?}"
        );

        let id = created["appComponent"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no appComponent.id in {created:?}"))
            .to_string();
        match request.get("id").and_then(Value::as_str) {
            Some(asked) => assert_eq!(
                id, asked,
                "case {case:?}: the caller's id must be kept, got {id:?}"
            ),
            None => assert_eq!(
                json!(id),
                request["name"],
                "case {case:?}: a request naming no id is addressable by its name, got {id:?}"
            ),
        }

        assert_eq!(
            created["appComponent"],
            expected_component(&request, &id),
            "case {case:?}: the created component must carry the request's members, got {created:?}"
        );

        // The round trip: the stored component is what a later read returns.
        let described = post(
            &router,
            "/describe-app-version-app-component",
            &json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": id}),
        )
        .await;
        let described = described.ok(case).clone();
        assert_eq!(
            described,
            json!({
                "appArn": app_arn,
                "appVersion": DRAFT_APP_VERSION,
                "appComponent": expected_component(&request, &id),
            }),
            "case {case:?}: describe must return exactly what the create persisted"
        );
    }

    // Every row of the table is still there: a later create never replaces an
    // earlier component.
    for row in component_table() {
        let case = row.label;
        let request = (row.request)();
        let Some(id) = request.get("id").and_then(Value::as_str) else {
            continue;
        };
        let described = post(
            &router,
            "/describe-app-version-app-component",
            &json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": id}),
        )
        .await;
        assert_eq!(
            described.ok(case)["appComponent"],
            expected_component(&request, id),
            "case {case:?}: every component of the draft version survives later creates"
        );
    }
}

/// Components belong to the application they were created under: the same id in
/// two applications is two components, and neither application sees the other's.
#[tokio::test]
async fn app_components_are_scoped_to_their_application() {
    let router = emulator();
    let first = create_an_app(&router, "avac-scope-one").await;
    let second = create_an_app(&router, "avac-scope-two").await;

    for (case, app_arn, component_type) in [
        ("first app", &first, "AWS::ResilienceHub::ComputeAppComponent"),
        ("second app", &second, "AWS::ResilienceHub::DatabaseAppComponent"),
    ] {
        post(
            &router,
            "/create-app-version-app-component",
            &json!({
                "appArn": app_arn,
                "id": "shared-id",
                "name": "shared-id",
                "type": component_type,
            }),
        )
        .await
        .ok(case);
    }

    for (case, app_arn, expected_type) in [
        ("first app", &first, "AWS::ResilienceHub::ComputeAppComponent"),
        ("second app", &second, "AWS::ResilienceHub::DatabaseAppComponent"),
    ] {
        let described = post(
            &router,
            "/describe-app-version-app-component",
            &json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": "shared-id"}),
        )
        .await;
        assert_eq!(
            described.ok(case)["appComponent"]["type"],
            json!(expected_type),
            "case {case:?}: each application holds its own component for one id, got {:?}",
            described.body
        );
    }

    // Deleting an application takes its components with it.
    post(&router, "/delete-app", &json!({"appArn": first}))
        .await
        .ok("delete the first app");
    post(
        &router,
        "/describe-app-version-app-component",
        &json!({"appArn": first, "appVersion": DRAFT_APP_VERSION, "id": "shared-id"}),
    )
    .await
    .refused(
        "a component of a deleted application",
        404,
        "ResourceNotFoundException",
    );
    let survivor = post(
        &router,
        "/describe-app-version-app-component",
        &json!({"appArn": second, "appVersion": DRAFT_APP_VERSION, "id": "shared-id"}),
    )
    .await;
    assert_eq!(
        survivor.ok("the other app's component")["appComponent"]["id"],
        json!("shared-id"),
        "deleting one application must not touch another's components, got {:?}",
        survivor.body
    );
}

/// Each link of a component's path has to resolve, and a create under an unknown
/// application persists nothing.
#[tokio::test]
async fn an_unresolvable_path_is_refused_with_the_modeled_404() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avac-not-found").await;
    let unknown_app = format!(
        "arn:aws:resiliencehub:{REGION}:000000000000:app/00000000-0000-0000-0000-000000000000"
    );

    post(
        &router,
        "/create-app-version-app-component",
        &json!({
            "appArn": unknown_app,
            "id": "orphan",
            "name": "orphan",
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await
    .refused(
        "CreateAppVersionAppComponent under an unknown appArn",
        404,
        "ResourceNotFoundException",
    );

    post(
        &router,
        "/create-app-version-app-component",
        &json!({
            "appArn": app_arn,
            "id": "present",
            "name": "present",
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await
    .ok("a component to read around");

    let refusals = [
        (
            "describe under an unknown appArn",
            json!({"appArn": unknown_app, "appVersion": DRAFT_APP_VERSION, "id": "present"}),
        ),
        (
            "describe an app version that was never published",
            json!({"appArn": app_arn, "appVersion": "1", "id": "present"}),
        ),
        (
            "describe an id the draft version does not hold",
            json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": "absent"}),
        ),
    ];
    for (case, body) in refusals {
        post(&router, "/describe-app-version-app-component", &body)
            .await
            .refused(case, 404, "ResourceNotFoundException");
    }

    // The draft version of an application nobody has written to exists, and holds
    // nothing: a read of it is a component-not-found, not a version-not-found.
    let empty = create_an_app(&router, "avac-empty-draft").await;
    let reply = post(
        &router,
        "/describe-app-version-app-component",
        &json!({"appArn": empty, "appVersion": DRAFT_APP_VERSION, "id": "present"}),
    )
    .await;
    reply.refused("read an empty draft version", 404, "ResourceNotFoundException");
    assert_eq!(
        reply.body["resourceType"],
        json!("AppComponent"),
        "an empty draft version exists, so the missing resource is the component, got {:?}",
        reply.body
    );
}

/// An `id` already in use in the app version is a conflict, and the stored
/// component is left as it was.
#[tokio::test]
async fn a_duplicate_id_is_refused_and_changes_nothing() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avac-duplicate").await;
    let first = json!({
        "appArn": app_arn,
        "id": "compute",
        "name": "compute",
        "type": "AWS::ResilienceHub::ComputeAppComponent",
    });
    post(&router, "/create-app-version-app-component", &first)
        .await
        .ok("the first component");

    for (case, differing) in [
        ("a different type", json!("AWS::ResilienceHub::DatabaseAppComponent")),
        ("the same type", json!("AWS::ResilienceHub::ComputeAppComponent")),
    ] {
        let mut second = first.clone();
        second["type"] = differing;
        second["name"] = json!("compute-again");
        post(&router, "/create-app-version-app-component", &second)
            .await
            .refused(
                &format!("a second component with the same id and {case}"),
                409,
                "ConflictException",
            );
    }

    let described = post(
        &router,
        "/describe-app-version-app-component",
        &json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": "compute"}),
    )
    .await;
    assert_eq!(
        described.ok("after the refused duplicates")["appComponent"],
        expected_component(&first, "compute"),
        "a refused duplicate must not overwrite the stored component, got {:?}",
        described.body
    );
}

/// A replayed `@idempotencyToken` replays the first result; the same token with
/// different parameters is a conflict. Every member the operation stores
/// participates in that comparison.
#[tokio::test]
async fn a_replayed_client_token_replays_the_first_component() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avac-idempotent").await;

    let request = json!({
        "appArn": app_arn,
        "name": "token-tier",
        "type": "AWS::ResilienceHub::ComputeAppComponent",
        "additionalInfo": {"failover-regions": ["[{\"region\":\"us-west-2\"}]"]},
        "clientToken": "avac-token-1",
    });
    let first = post(&router, "/create-app-version-app-component", &request)
        .await
        .ok("the first call")
        .clone();
    let id = first["appComponent"]["id"]
        .as_str()
        .expect("the component's id")
        .to_string();

    let replay = post(&router, "/create-app-version-app-component", &request).await;
    assert_eq!(
        replay.ok("the replayed call"),
        &first,
        "a replayed clientToken must return the first result, got {:?}",
        replay.body
    );

    // ... and must not have created a second component: the id is the first one's.
    assert_eq!(
        replay.body["appComponent"]["id"],
        json!(id),
        "a replay must not mint a second component, got {:?}",
        replay.body
    );

    for (case, member, value) in [
        ("name", "name", json!("other-tier")),
        ("type", "type", json!("AWS::ResilienceHub::DatabaseAppComponent")),
        (
            "additionalInfo",
            "additionalInfo",
            json!({"failover-regions": ["[{\"region\":\"eu-west-1\"}]"]}),
        ),
        ("id", "id", json!("explicitly-different")),
    ] {
        let mut divergent = request.clone();
        divergent[member] = value;
        post(&router, "/create-app-version-app-component", &divergent)
            .await
            .refused(
                &format!("the same clientToken with a different {case}"),
                409,
                "ConflictException",
            );
    }

    // The same token aimed at another application is a conflict too: it recorded a
    // component of the first one.
    let other = create_an_app(&router, "avac-idempotent-other").await;
    let mut elsewhere = request.clone();
    elsewhere["appArn"] = json!(other);
    post(&router, "/create-app-version-app-component", &elsewhere)
        .await
        .refused(
            "the same clientToken against another application",
            409,
            "ConflictException",
        );
}

/// The modelled input constraints are refused at the boundary, so no component is
/// stored for a request outside them: `String255`'s 1..255 `length` on `name` and
/// `id`, and `AdditionalInfoValueList`'s 1..10.
#[tokio::test]
async fn a_request_outside_a_modeled_constraint_is_refused() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avac-constraints").await;
    let past_max = format!("c{}", "x".repeat(255));

    let refusals = [
        (
            "a name one character past String255",
            json!({
                "appArn": app_arn,
                "name": past_max,
                "type": "AWS::ResilienceHub::ComputeAppComponent",
            }),
        ),
        (
            "an id one character past String255",
            json!({
                "appArn": app_arn,
                "id": past_max,
                "name": "past-max-id",
                "type": "AWS::ResilienceHub::ComputeAppComponent",
            }),
        ),
        (
            "an empty name",
            json!({
                "appArn": app_arn,
                "name": "",
                "type": "AWS::ResilienceHub::ComputeAppComponent",
            }),
        ),
        (
            "eleven additionalInfo values",
            json!({
                "appArn": app_arn,
                "id": "too-much-info",
                "name": "too-much-info",
                "type": "AWS::ResilienceHub::ComputeAppComponent",
                "additionalInfo": {
                    "one": (1..=11).map(|n| format!("value-{n}")).collect::<Vec<_>>(),
                },
            }),
        ),
    ];
    for (case, body) in refusals {
        post(&router, "/create-app-version-app-component", &body)
            .await
            .refused(case, 400, "ValidationException");
    }

    post(
        &router,
        "/describe-app-version-app-component",
        &json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": "too-much-info"}),
    )
    .await
    .refused(
        "a refused request stores nothing",
        404,
        "ResourceNotFoundException",
    );
}

/// The component store is BOUNDED: past its documented ceiling the create answers
/// the modelled `ServiceQuotaExceededException` instead of growing.
#[tokio::test]
async fn the_app_component_store_is_bounded() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avac-bounded").await;

    for index in 0..MAX_APP_COMPONENTS_PER_APP_VERSION {
        post(
            &router,
            "/create-app-version-app-component",
            &json!({
                "appArn": app_arn,
                "id": format!("bounded-{index}"),
                "name": format!("bounded-{index}"),
                "type": "AWS::ResilienceHub::ComputeAppComponent",
            }),
        )
        .await
        .ok(&format!(
            "component {index} of {MAX_APP_COMPONENTS_PER_APP_VERSION}"
        ));
    }

    post(
        &router,
        "/create-app-version-app-component",
        &json!({
            "appArn": app_arn,
            "id": "bounded-overflow",
            "name": "bounded-overflow",
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await
    .refused(
        "a component past MAX_APP_COMPONENTS_PER_APP_VERSION",
        402,
        "ServiceQuotaExceededException",
    );

    // Another application still has its own room: the ceiling is per app version.
    let other = create_an_app(&router, "avac-bounded-other").await;
    post(
        &router,
        "/create-app-version-app-component",
        &json!({
            "appArn": other,
            "id": "bounded-0",
            "name": "bounded-0",
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await
    .ok("the ceiling is per app version");
}

/// A read changes nothing a later caller sees: describing a component twice
/// returns the same answer, and a refused read leaves the version untouched.
#[tokio::test]
async fn describing_a_component_changes_nothing() {
    let router = emulator();
    let app_arn = create_an_app(&router, "avac-read-only").await;
    let request = json!({
        "appArn": app_arn,
        "id": "read-only",
        "name": "read-only",
        "type": "AWS::ResilienceHub::ComputeAppComponent",
    });
    post(&router, "/create-app-version-app-component", &request)
        .await
        .ok("the component to read");

    let read = json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": "read-only"});
    let first = post(&router, "/describe-app-version-app-component", &read)
        .await
        .ok("the first read")
        .clone();
    // A refused read in between must not change anything either.
    post(
        &router,
        "/describe-app-version-app-component",
        &json!({"appArn": app_arn, "appVersion": DRAFT_APP_VERSION, "id": "nothing-here"}),
    )
    .await
    .refused("a read of an absent id", 404, "ResourceNotFoundException");
    let second = post(&router, "/describe-app-version-app-component", &read).await;
    assert_eq!(
        second.ok("the second read"),
        &first,
        "two reads of one component must agree, got {:?}",
        second.body
    );
}
