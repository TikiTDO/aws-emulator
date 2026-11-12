// SPDX-License-Identifier: CC0-1.0

//! Every entity type the model names is a type this catalog serves, end to end over HTTP.
//!
//! One rule is under test, in the three places an `EntityType` is judged: the set of
//! entity types this service recognises is the UNION of every type the model names — the
//! valid values `ListEntitiesRequest$EntityType` enumerates in prose and the types the
//! model declares per-type summary, filter and sort shapes for — so listing any of them
//! answers a page (empty while the catalog holds nothing of that type), a change set may
//! create an entity of any of them, and such an entity is reported by the members every
//! entity carries while a type with no sort shape of its own orders only by the two
//! attributes `Sort$SortBy` publishes for every type.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, ENTITY_SORT_BY,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`).
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The entity types `ListEntitiesRequest$EntityType` documents as its valid values,
/// written out here from the model rather than read from the crate
/// (`smithy_query(query="doc", shape="ListEntitiesRequest", member="EntityType")`,
/// verbatim: "Valid values are: AmiProduct, ContainerProduct, DataProduct, SaaSProduct,
/// ProcurementPolicy, Experience, Audience, BrandingSettings, Offer, OfferSet, Seller,
/// ResaleAuthorization, Solution").
const DOCUMENTED_ENTITY_TYPES: [&str; 13] = [
    "AmiProduct",
    "ContainerProduct",
    "DataProduct",
    "SaaSProduct",
    "ProcurementPolicy",
    "Experience",
    "Audience",
    "BrandingSettings",
    "Offer",
    "OfferSet",
    "Seller",
    "ResaleAuthorization",
    "Solution",
];

/// The entity types the model names only through the per-type shapes it declares for them
/// (`EntitySummary$MachineLearningProductSummary`,
/// `EntitySummary$ExpressPrivateOfferConfigurationSummary` and the matching
/// `EntityTypeFilters` / `EntityTypeSort` members), which
/// [`DOCUMENTED_ENTITY_TYPES`] leaves out.
const SHAPE_ONLY_ENTITY_TYPES: [&str; 2] = [
    "MachineLearningProduct",
    "ExpressPrivateOfferConfiguration",
];

/// The per-type summary member of `EntitySummary` a type carries, for the types the model
/// declares one for: the type's name with `Summary` appended. A type absent from this
/// table carries no per-type summary at all.
const SUMMARY_MEMBERS: &[(&str, &str)] = &[
    ("AmiProduct", "AmiProductSummary"),
    ("ContainerProduct", "ContainerProductSummary"),
    ("DataProduct", "DataProductSummary"),
    ("SaaSProduct", "SaaSProductSummary"),
    ("MachineLearningProduct", "MachineLearningProductSummary"),
    ("Offer", "OfferSummary"),
    ("OfferSet", "OfferSetSummary"),
    ("ResaleAuthorization", "ResaleAuthorizationSummary"),
    (
        "ExpressPrivateOfferConfiguration",
        "ExpressPrivateOfferConfigurationSummary",
    ),
];

/// Every entity type the model names, either way.
fn every_entity_type() -> Vec<&'static str> {
    let mut types = DOCUMENTED_ENTITY_TYPES.to_vec();
    types.extend_from_slice(&SHAPE_ONLY_ENTITY_TYPES);
    types
}

/// How long after the start a change set has certainly reached `SUCCEEDED`.
fn wait_for_success() {
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The array at `pointer`, or an empty one.
    fn list(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn list_entities(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/ListEntities", Some(request)).await
}

async fn describe_entity(router: &MultiServiceRouter, entity_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
        None,
    )
    .await
}

/// Create one entity per `(entity_type, details)` through its own change set, wait once
/// for the clock to apply them all, and answer the entity ids in the order asked for.
async fn created_entities(router: &MultiServiceRouter, specs: &[(&str, Value)]) -> Vec<String> {
    let mut change_sets: Vec<String> = Vec::new();
    for (entity_type, details) in specs {
        let started = call(
            router,
            "POST",
            "/StartChangeSet",
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": format!("{entity_type}@1.0")},
                    "DetailsDocument": details,
                }],
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {entity_type:?}: a change set creating an entity of a type the model \
             names must be accepted, got {} with {:?}",
            started.status, started.body
        );
        change_sets.push(started.text("/ChangeSetId"));
    }
    wait_for_success();

    let listed = call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG, "MaxResults": 20})),
    )
    .await;
    let summaries = listed.list("/ChangeSetSummaryList");
    change_sets
        .iter()
        .map(|change_set| {
            let summary = summaries
                .iter()
                .find(|item| item["ChangeSetId"] == json!(change_set))
                .unwrap_or_else(|| {
                    panic!(
                        "the fixture's change set {change_set} must be listed, got {:?}",
                        listed.body
                    )
                });
            assert_eq!(
                summary["Status"],
                json!("SUCCEEDED"),
                "the fixture's change set must have succeeded, got {summary:?}"
            );
            summary["EntityIdList"][0]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("change set {change_set} must name its entity, got {summary:?}")
                })
                .to_string()
        })
        .collect()
}

/// Every entity type the model names is listed, and an empty catalog answers each of them
/// a present, empty page rather than a refusal.
#[tokio::test]
async fn every_modelled_entity_type_is_listed() {
    let router = emulator();

    for entity_type in every_entity_type() {
        let answer = list_entities(
            &router,
            json!({"Catalog": CATALOG, "EntityType": entity_type}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {entity_type:?}: a type the model names must be listed, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.pointer("/EntitySummaryList"),
            Some(&json!([])),
            "case {entity_type:?}: a catalog holding nothing of the type must answer a \
             present, empty page, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body.pointer("/NextToken"),
            None,
            "case {entity_type:?}: a single empty page continues nowhere, got {:?}",
            answer.body
        );
    }
}

/// An entity of any type the model names round-trips: a change set creates it,
/// `DescribeEntity` reports it under that type, and `ListEntities` of that type serves it
/// with the members every entity carries — and only under its own type.
#[tokio::test]
async fn an_entity_of_any_modelled_type_round_trips() {
    let router = emulator();

    // The rows span the types whose summary, filter and sort shapes the model declares and
    // the types it names only in `ListEntitiesRequest$EntityType`'s valid values, each
    // naming its entity through a details member `EntitySummary$Name` is read from.
    let specs: Vec<(&str, Value)> = vec![
        (
            "AmiProduct",
            json!({"ProductTitle": "an ami product", "Visibility": "Public"}),
        ),
        (
            "ProcurementPolicy",
            json!({"Name": "a procurement policy", "Visibility": "Limited"}),
        ),
        ("Experience", json!({"Name": "an experience"})),
        (
            "Audience",
            json!({"Name": "an audience", "Visibility": "Restricted"}),
        ),
        ("BrandingSettings", json!({"Name": "branding settings"})),
        ("Seller", json!({"Name": "a seller"})),
        (
            "Solution",
            json!({"Name": "a solution", "Visibility": "Draft"}),
        ),
        (
            "MachineLearningProduct",
            json!({"ProductTitle": "an ml product", "Visibility": "Limited"}),
        ),
    ];
    let ids = created_entities(&router, &specs).await;

    for ((entity_type, details), entity_id) in specs.iter().zip(ids.iter()) {
        let described = describe_entity(&router, entity_id).await;
        assert_eq!(
            (described.status, described.text("/EntityType")),
            (200, (*entity_type).to_string()),
            "case {entity_type:?}: the created entity must be described under its own \
             type, got {} with {:?}",
            described.status,
            described.body
        );

        let listed = list_entities(
            &router,
            json!({"Catalog": CATALOG, "EntityType": entity_type}),
        )
        .await;
        let summaries = listed.list("/EntitySummaryList");
        assert_eq!(
            (listed.status, summaries.len()),
            (200, 1),
            "case {entity_type:?}: the one entity of the type must be served, got {} \
             with {:?}",
            listed.status,
            listed.body
        );
        let summary = &summaries[0];
        let expected_name = details["Name"]
            .as_str()
            .or_else(|| details["ProductTitle"].as_str())
            .expect("every row names its entity");
        assert_eq!(
            (
                summary["EntityType"].as_str().unwrap_or_default(),
                summary["EntityId"].as_str().unwrap_or_default(),
                summary["Name"].as_str().unwrap_or_default(),
            ),
            (*entity_type, entity_id.as_str(), expected_name),
            "case {entity_type:?}: the summary must report the persisted entity, got \
             {summary:?}"
        );
        assert!(
            summary["EntityArn"]
                .as_str()
                .is_some_and(|arn| arn.contains(&format!("/{entity_type}/{entity_id}"))),
            "case {entity_type:?}: the summary must carry the entity's own ARN, got \
             {summary:?}"
        );

        // The per-type summary is reported for exactly the types the model declares one
        // for, and a type it declares none for carries none of the others either.
        let own_member = SUMMARY_MEMBERS
            .iter()
            .find(|(named, _)| named == entity_type)
            .map(|(_, member)| *member);
        for (_, member) in SUMMARY_MEMBERS {
            let present = summary.get(*member).is_some_and(|value| !value.is_null());
            assert_eq!(
                present,
                own_member == Some(*member),
                "case {entity_type:?}: {member} must be reported only for the type it \
                 belongs to, got {summary:?}"
            );
        }

        // The entity belongs to its own type alone: no other type the model names lists it.
        for other in every_entity_type() {
            if other == *entity_type {
                continue;
            }
            let elsewhere = list_entities(
                &router,
                json!({"Catalog": CATALOG, "EntityType": other}),
            )
            .await;
            let served_elsewhere = elsewhere
                .list("/EntitySummaryList")
                .iter()
                .any(|item| item["EntityId"].as_str() == Some(entity_id.as_str()));
            assert!(
                !served_elsewhere,
                "case {entity_type:?}: listing {other:?} must not serve it, got {:?}",
                elsewhere.body
            );
        }
    }
}

/// A type the model declares no sort shape for orders by the two attributes `Sort$SortBy`
/// publishes for every type, and by nothing else.
#[tokio::test]
async fn a_type_without_its_own_sort_shape_orders_by_the_common_attributes() {
    let router = emulator();
    let specs: Vec<(&str, Value)> = vec![
        ("Seller", json!({"Name": "seller one"})),
        ("Seller", json!({"Name": "seller two"})),
    ];
    let ids = created_entities(&router, &specs).await;

    for sort_by in ENTITY_SORT_BY {
        for order in ["ASCENDING", "DESCENDING"] {
            let answer = list_entities(
                &router,
                json!({
                    "Catalog": CATALOG,
                    "EntityType": "Seller",
                    "Sort": {"SortBy": sort_by, "SortOrder": order},
                }),
            )
            .await;
            let served: Vec<String> = answer
                .list("/EntitySummaryList")
                .iter()
                .map(|item| item["EntityId"].as_str().unwrap_or_default().to_string())
                .collect();
            let mut expected = ids.clone();
            if order == "DESCENDING" {
                expected.reverse();
            }
            assert_eq!(
                (answer.status, served.clone()),
                (200, expected.clone()),
                "case {sort_by:?}/{order:?}: both entities must be served in the order \
                 asked for, got {} with {:?}",
                answer.status,
                answer.body
            );
        }
    }

    // A field belonging to a type's own sort shape orders nothing on a type that has none,
    // so it is refused.
    for sort_by in ["ProductTitle", "Visibility", "BuyerAccounts", "SolutionId"] {
        let refused = list_entities(
            &router,
            json!({
                "Catalog": CATALOG,
                "EntityType": "Seller",
                "Sort": {"SortBy": sort_by},
            }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {sort_by:?}: a field this type publishes no sort for must be refused, \
             got {} with {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.text("/Message").contains(sort_by),
            "case {sort_by:?}: the refusal must name the attribute asked for, got {:?}",
            refused.body
        );
    }

    // And a letters-only value naming no type the model names is still refused rather than
    // listed as an empty page.
    for entity_type in ["Sellers", "seller", "Frobnicator", "ProcurementPolicies"] {
        let refused = list_entities(
            &router,
            json!({"Catalog": CATALOG, "EntityType": entity_type}),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {entity_type:?}: a value naming no modelled type must be refused, got \
             {} with {:?}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("/Message"),
            format!("[Requested entity type '{entity_type}' is invalid.]"),
            "case {entity_type:?}: the fault must name the value asked for, got {:?}",
            refused.body
        );
    }
}
