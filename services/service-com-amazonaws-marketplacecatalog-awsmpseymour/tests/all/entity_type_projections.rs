// SPDX-License-Identifier: CC0-1.0

//! The per-`EntityType` surface of `ListEntities`, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! One rule is under test, in its three uses: an entity type's own fields are named after
//! the `EntitySummary` member that reports them, so `ListEntities` selects on them
//! (`EntityTypeFilters`), orders by them (`EntityTypeSort`, and the generic `Sort`, whose
//! `SortBy` documents "each `EntityType` might support additional fields") and reports
//! them in the per-type summary of the listed entity's own type — each read from the
//! entity the catalog persisted and from nothing else.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    entity_type_sort_fields, CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, ENTITY_SORT_BY,
    SHAPED_ENTITY_TYPES,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`).
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The per-type summary member of `EntitySummary` each entity type reports, as the model
/// names them (`EntitySummary$AmiProductSummary`, `$OfferSummary`, …): the entity type's
/// name with `Summary` appended.
const SUMMARY_MEMBERS: &[(&str, &str)] = &[
    ("AmiProduct", "AmiProductSummary"),
    ("ContainerProduct", "ContainerProductSummary"),
    ("DataProduct", "DataProductSummary"),
    (
        "ExpressPrivateOfferConfiguration",
        "ExpressPrivateOfferConfigurationSummary",
    ),
    ("MachineLearningProduct", "MachineLearningProductSummary"),
    ("Offer", "OfferSummary"),
    ("OfferSet", "OfferSetSummary"),
    ("ResaleAuthorization", "ResaleAuthorizationSummary"),
    ("SaaSProduct", "SaaSProductSummary"),
];

/// How long after the start a change set has certainly reached `SUCCEEDED`: the two spans
/// the implementation publishes, plus a margin for a slow machine.
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
            "the fixture must start a change set for {entity_type}, got {} with {:?}",
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

/// The entities every case of the filter and sort tables selects from: two of a product
/// type and one each of the types whose fields are their own.
fn fixtures() -> Vec<(&'static str, Value)> {
    vec![
        (
            "AmiProduct",
            json!({"ProductTitle": "alpha ami", "Visibility": "Limited"}),
        ),
        (
            "AmiProduct",
            json!({"ProductTitle": "beta ami", "Visibility": "Public"}),
        ),
        (
            "Offer",
            json!({
                "Name": "alpha offer",
                "ProductId": "prod-1",
                "State": "Released",
                "BuyerAccounts": ["111122223333", "444455556666"],
                "Targeting": ["BuyerAccounts"],
                "ReleaseDate": "2026-01-01T00:00:00Z",
            }),
        ),
        (
            "Offer",
            json!({
                "Name": "beta offer",
                "ProductId": "prod-2",
                "State": "Draft",
                "BuyerAccounts": ["777788889999"],
                "ReleaseDate": "2027-01-01T00:00:00Z",
            }),
        ),
        (
            "ResaleAuthorization",
            json!({
                "Name": "resale one",
                "ProductId": "prod-1",
                "Status": "Active",
                "ResellerAccountID": "444455556666",
                "CreatedDate": "2026-02-02T00:00:00Z",
            }),
        ),
        (
            "OfferSet",
            json!({
                "Name": "set one",
                "State": "Released",
                "AssociatedOfferIds": ["offer-one", "offer-two"],
                "SolutionId": "solution-1",
            }),
        ),
        (
            "ExpressPrivateOfferConfiguration",
            json!({"ProductId": "prod-9", "State": "ACTIVE"}),
        ),
        (
            "SaaSProduct",
            json!({"ProductTitle": "saas one", "Visibility": "Restricted"}),
        ),
    ]
}

/// The `EntityId`s a `ListEntities` answer reported, in the order it served them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer
        .list("/EntitySummaryList")
        .iter()
        .map(|item| {
            item["EntityId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One request built from the per-type surface, the generic one, or both, and the
/// entities every page of it must serve.
#[derive(Default)]
struct Composition {
    case: &'static str,
    entity_type: &'static str,
    entity_type_filters: Option<Value>,
    entity_type_sort: Option<Value>,
    filter_list: Option<Value>,
    sort: Option<Value>,
    /// The page size each page asks for; every case walks `NextToken` to the end.
    max_results: i64,
    /// Fixture indices, in the order the pages must serve them when `ordered`.
    expected: Vec<usize>,
    /// Whether the case asked for an ordering, so the order served is asserted too.
    ordered: bool,
}

/// Walk every page of `request`, and answer the ids served in order and how many pages it
/// took. A page is asked for with the `NextToken` the one before it answered, so the walk
/// is the one a paginating client performs.
async fn paged_ids(
    router: &MultiServiceRouter,
    request: &Value,
    case: &str,
) -> (Vec<String>, usize) {
    let mut served: Vec<String> = Vec::new();
    let mut pages = 0_usize;
    let mut token: Option<String> = None;
    loop {
        let mut page = request.clone();
        if let Some(held) = token.as_deref() {
            page["NextToken"] = json!(held);
        }
        let answer = list_entities(router, page).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: every page must be served, got {} with {:?}",
            answer.status, answer.body
        );
        pages += 1;
        served.extend(listed_ids(&answer));
        let next = answer.text("/NextToken");
        if next.is_empty() {
            return (served, pages);
        }
        assert!(
            pages < 16,
            "case {case:?}: the walk must reach a page carrying no NextToken, got {served:?}"
        );
        token = Some(next);
    }
}

/// The per-type filter and sort surfaces compose: with each other, with the generic
/// `FilterList` and `Sort`, and with `MaxResults`/`NextToken` paging — the selection is
/// the conjunction of every filter asked for, the ordering is the one asked for, and the
/// pages of one walk serve exactly the selected entities once each.
///
/// A sub-member of the listed type that no stored entity carries selects nothing and
/// orders nothing, which is a served page rather than a refusal.
#[tokio::test]
async fn the_per_type_surfaces_compose_with_the_generic_ones_and_with_paging() {
    let router = emulator();
    let ids = created_entities(&router, &fixtures()).await;

    let cases: Vec<Composition> = vec![
        Composition {
            case: "a per-type filter and sort, one entity a page",
            entity_type: "AmiProduct",
            entity_type_filters: Some(
                json!({"AmiProductFilters": {"Visibility": {"ValueList": ["Limited", "Public"]}}}),
            ),
            entity_type_sort: Some(
                json!({"AmiProductSort": {"SortBy": "ProductTitle", "SortOrder": "ASCENDING"}}),
            ),
            max_results: 1,
            expected: vec![0, 1],
            ordered: true,
            ..Composition::default()
        },
        Composition {
            case: "the same filter and sort on one page",
            entity_type: "AmiProduct",
            entity_type_filters: Some(
                json!({"AmiProductFilters": {"Visibility": {"ValueList": ["Limited", "Public"]}}}),
            ),
            entity_type_sort: Some(
                json!({"AmiProductSort": {"SortBy": "ProductTitle", "SortOrder": "ASCENDING"}}),
            ),
            max_results: 50,
            expected: vec![0, 1],
            ordered: true,
            ..Composition::default()
        },
        Composition {
            case: "a per-type filter conjoined with the generic FilterList",
            entity_type: "AmiProduct",
            entity_type_filters: Some(
                json!({"AmiProductFilters": {"Visibility": {"ValueList": ["Limited", "Public"]}}}),
            ),
            filter_list: Some(json!([{"Name": "EntityId", "ValueList": [ids[0].clone()]}])),
            max_results: 50,
            expected: vec![0],
            ..Composition::default()
        },
        Composition {
            case: "a generic FilterList naming the entity the per-type filter excludes",
            entity_type: "AmiProduct",
            entity_type_filters: Some(
                json!({"AmiProductFilters": {"Visibility": {"ValueList": ["Public"]}}}),
            ),
            filter_list: Some(json!([{"Name": "EntityId", "ValueList": [ids[0].clone()]}])),
            max_results: 50,
            expected: vec![],
            ..Composition::default()
        },
        Composition {
            case: "a per-type filter with the generic Sort ordering the pages",
            entity_type: "AmiProduct",
            entity_type_filters: Some(
                json!({"AmiProductFilters": {"ProductTitle": {"WildCardValue": "ami"}}}),
            ),
            sort: Some(json!({"SortBy": "ProductTitle", "SortOrder": "DESCENDING"})),
            max_results: 1,
            expected: vec![1, 0],
            ordered: true,
            ..Composition::default()
        },
        Composition {
            case: "offers ordered by a per-type field, one a page",
            entity_type: "Offer",
            entity_type_sort: Some(
                json!({"OfferSort": {"SortBy": "Name", "SortOrder": "DESCENDING"}}),
            ),
            max_results: 1,
            expected: vec![3, 2],
            ordered: true,
            ..Composition::default()
        },
        Composition {
            case: "a per-type filter on a sub-member no stored offer carries",
            entity_type: "Offer",
            entity_type_filters: Some(
                json!({"OfferFilters": {"TargetAgreementId": {"ValueList": ["agreement-absent"]}}}),
            ),
            max_results: 50,
            expected: vec![],
            ..Composition::default()
        },
        Composition {
            case: "a per-type sort by a field no stored offer carries",
            entity_type: "Offer",
            entity_type_sort: Some(json!({"OfferSort": {"SortBy": "TargetAgreementIntent"}})),
            max_results: 50,
            expected: vec![2, 3],
            ..Composition::default()
        },
        Composition {
            case: "a per-type filter, a per-type sort and the generic FilterList together",
            entity_type: "Offer",
            entity_type_filters: Some(
                json!({"OfferFilters": {"State": {"ValueList": ["Draft", "Released"]}}}),
            ),
            entity_type_sort: Some(
                json!({"OfferSort": {"SortBy": "ReleaseDate", "SortOrder": "ASCENDING"}}),
            ),
            filter_list: Some(
                json!([{"Name": "EntityId", "ValueList": [ids[2].clone(), ids[3].clone()]}]),
            ),
            max_results: 1,
            expected: vec![2, 3],
            ordered: true,
            ..Composition::default()
        },
        Composition {
            case: "a resale authorization filtered and sorted by its own fields",
            entity_type: "ResaleAuthorization",
            entity_type_filters: Some(
                json!({"ResaleAuthorizationFilters": {"Status": {"ValueList": ["Active"]}}}),
            ),
            entity_type_sort: Some(
                json!({"ResaleAuthorizationSort": {"SortBy": "Name", "SortOrder": "ASCENDING"}}),
            ),
            max_results: 50,
            expected: vec![4],
            ordered: true,
            ..Composition::default()
        },
    ];

    for spec in cases {
        let case = spec.case;
        let mut request = json!({
            "Catalog": CATALOG,
            "EntityType": spec.entity_type,
            "MaxResults": spec.max_results,
        });
        for (member, value) in [
            ("EntityTypeFilters", spec.entity_type_filters),
            ("EntityTypeSort", spec.entity_type_sort),
            ("FilterList", spec.filter_list),
            ("Sort", spec.sort),
        ] {
            if let Some(value) = value {
                request[member] = value;
            }
        }

        let (served, pages) = paged_ids(&router, &request, case).await;
        let wanted: Vec<String> = spec
            .expected
            .iter()
            .map(|index| ids[*index].clone())
            .collect();
        if spec.ordered {
            assert_eq!(
                served, wanted,
                "case {case:?}: the pages must serve the selected entities in the order asked \
                 for, got {served:?}"
            );
        } else {
            let mut sorted = served.clone();
            sorted.sort();
            let mut sorted_wanted = wanted.clone();
            sorted_wanted.sort();
            assert_eq!(
                sorted, sorted_wanted,
                "case {case:?}: the pages must serve exactly the selected entities, \
                 got {served:?}"
            );
        }
        let page_size = usize::try_from(spec.max_results).expect("the page size is positive");
        let expected_pages = wanted.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} entities must be served {page_size} a page, got {pages} pages \
             of {served:?}",
            wanted.len()
        );
    }
}

/// `EntityTypeFilters` selects on the fields of the entity type being listed, over a
/// value list, a wildcard substring, a date range, a list-valued field and two fields at
/// once — each row a different way the one filter surface is asked.
#[tokio::test]
async fn a_type_specific_filter_selects_on_the_listed_types_own_fields() {
    let router = emulator();
    let ids = created_entities(&router, &fixtures()).await;

    // (case, EntityType, EntityTypeFilters, the fixture indices that must be served)
    let cases: Vec<(&str, &str, Value, Vec<usize>)> = vec![
        (
            "a value list on a product's visibility",
            "AmiProduct",
            json!({"AmiProductFilters": {"Visibility": {"ValueList": ["Public"]}}}),
            vec![1],
        ),
        (
            "a value list naming both visibilities",
            "AmiProduct",
            json!({"AmiProductFilters": {"Visibility": {"ValueList": ["Limited", "Public"]}}}),
            vec![0, 1],
        ),
        (
            "a wildcard on a product title",
            "AmiProduct",
            json!({"AmiProductFilters": {"ProductTitle": {"WildCardValue": "beta"}}}),
            vec![1],
        ),
        (
            "a value list on a product title",
            "AmiProduct",
            json!({"AmiProductFilters": {"ProductTitle": {"ValueList": ["alpha ami"]}}}),
            vec![0],
        ),
        (
            "a date range spanning every entity",
            "AmiProduct",
            json!({"AmiProductFilters": {"LastModifiedDate": {"DateRange": {
                "AfterValue": "2000-01-01T00:00:00Z",
                "BeforeValue": "2999-01-01T00:00:00Z",
            }}}}),
            vec![0, 1],
        ),
        (
            "a date range that ended before anything was stored",
            "AmiProduct",
            json!({"AmiProductFilters": {"LastModifiedDate": {"DateRange": {
                "BeforeValue": "2000-01-01T00:00:00Z",
            }}}}),
            vec![],
        ),
        (
            "two fields, which conjoin",
            "AmiProduct",
            json!({"AmiProductFilters": {
                "Visibility": {"ValueList": ["Public"]},
                "ProductTitle": {"WildCardValue": "alpha"},
            }}),
            vec![],
        ),
        (
            "the entity id of one product",
            "AmiProduct",
            json!({"AmiProductFilters": {"EntityId": {"ValueList": [ids[0].clone()]}}}),
            vec![0],
        ),
        (
            "an offer's state",
            "Offer",
            json!({"OfferFilters": {"State": {"ValueList": ["Released"]}}}),
            vec![2],
        ),
        (
            "an offer's product id",
            "Offer",
            json!({"OfferFilters": {"ProductId": {"ValueList": ["prod-2"]}}}),
            vec![3],
        ),
        (
            "a wildcard on an offer's buyer accounts, which is list-valued",
            "Offer",
            json!({"OfferFilters": {"BuyerAccounts": {"WildCardValue": "4444"}}}),
            vec![2],
        ),
        (
            "an offer released after a bound",
            "Offer",
            json!({"OfferFilters": {"ReleaseDate": {"DateRange": {
                "AfterValue": "2026-06-01T00:00:00Z",
            }}}}),
            vec![3],
        ),
        (
            "a resale authorization's reseller account",
            "ResaleAuthorization",
            json!({"ResaleAuthorizationFilters": {
                "ResellerAccountID": {"ValueList": ["444455556666"]},
            }}),
            vec![4],
        ),
        (
            "a resale authorization's created date as a value list",
            "ResaleAuthorization",
            json!({"ResaleAuthorizationFilters": {
                "CreatedDate": {"ValueList": ["2026-02-02T00:00:00Z"]},
            }}),
            vec![4],
        ),
        (
            "a resale authorization created after a bound",
            "ResaleAuthorization",
            json!({"ResaleAuthorizationFilters": {"CreatedDate": {"DateRange": {
                "AfterValue": "2026-03-03T00:00:00Z",
            }}}}),
            vec![],
        ),
        (
            "an offer set's associated offer ids, which is list-valued",
            "OfferSet",
            json!({"OfferSetFilters": {"AssociatedOfferIds": {"ValueList": ["offer-two"]}}}),
            vec![5],
        ),
        (
            "an offer set's solution id",
            "OfferSet",
            json!({"OfferSetFilters": {"SolutionId": {"ValueList": ["solution-2"]}}}),
            vec![],
        ),
        (
            "an express private offer configuration's state",
            "ExpressPrivateOfferConfiguration",
            json!({"ExpressPrivateOfferConfigurationFilters": {
                "State": {"ValueList": ["ACTIVE"]},
            }}),
            vec![6],
        ),
        (
            "a SaaS product's visibility, which no entity carries",
            "SaaSProduct",
            json!({"SaaSProductFilters": {"Visibility": {"ValueList": ["Draft"]}}}),
            vec![],
        ),
    ];

    for (case, entity_type, filters, expected) in cases {
        let answer = list_entities(
            &router,
            json!({
                "Catalog": CATALOG,
                "EntityType": entity_type,
                "EntityTypeFilters": filters,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the per-type filter must be served, got {} with {:?}",
            answer.status, answer.body
        );
        let mut served = listed_ids(&answer);
        served.sort();
        let mut wanted: Vec<String> = expected.iter().map(|index| ids[*index].clone()).collect();
        wanted.sort();
        assert_eq!(
            served, wanted,
            "case {case:?}: the filter must select the entities carrying the field, \
             got {served:?}"
        );
    }
}

/// `EntityTypeSort` orders the page by a field of the entity type being listed, in the
/// direction asked for; the generic `Sort` accepts those same fields, and a field
/// belonging to another type is refused.
#[tokio::test]
async fn a_type_specific_sort_orders_by_the_listed_types_own_fields() {
    let router = emulator();
    let ids = created_entities(&router, &fixtures()).await;

    // (case, EntityType, EntityTypeSort, the fixture indices in the order served)
    let cases: Vec<(&str, &str, Value, Vec<usize>)> = vec![
        (
            "a product title ascending",
            "AmiProduct",
            json!({"AmiProductSort": {"SortBy": "ProductTitle", "SortOrder": "ASCENDING"}}),
            vec![0, 1],
        ),
        (
            "a product title descending",
            "AmiProduct",
            json!({"AmiProductSort": {"SortBy": "ProductTitle", "SortOrder": "DESCENDING"}}),
            vec![1, 0],
        ),
        (
            "a visibility ascending, which orders Limited before Public",
            "AmiProduct",
            json!({"AmiProductSort": {"SortBy": "Visibility", "SortOrder": "ASCENDING"}}),
            vec![0, 1],
        ),
        (
            "an offer name ascending",
            "Offer",
            json!({"OfferSort": {"SortBy": "Name", "SortOrder": "ASCENDING"}}),
            vec![2, 3],
        ),
        (
            "an offer state ascending, which orders Draft before Released",
            "Offer",
            json!({"OfferSort": {"SortBy": "State", "SortOrder": "ASCENDING"}}),
            vec![3, 2],
        ),
        (
            "an offer release date descending",
            "Offer",
            json!({"OfferSort": {"SortBy": "ReleaseDate", "SortOrder": "DESCENDING"}}),
            vec![3, 2],
        ),
        (
            "an offer's buyer accounts ascending, which sorts by its least value",
            "Offer",
            json!({"OfferSort": {"SortBy": "BuyerAccounts", "SortOrder": "ASCENDING"}}),
            vec![2, 3],
        ),
        (
            "an offer product id descending",
            "Offer",
            json!({"OfferSort": {"SortBy": "ProductId", "SortOrder": "DESCENDING"}}),
            vec![3, 2],
        ),
    ];

    for (case, entity_type, sort, expected) in cases {
        let answer = list_entities(
            &router,
            json!({
                "Catalog": CATALOG,
                "EntityType": entity_type,
                "EntityTypeSort": sort,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the per-type sort must be served, got {} with {:?}",
            answer.status, answer.body
        );
        let wanted: Vec<String> = expected.iter().map(|index| ids[*index].clone()).collect();
        assert_eq!(
            listed_ids(&answer),
            wanted,
            "case {case:?}: the page must be ordered by the field asked for, got {:?}",
            answer.body
        );
    }

    // The generic `Sort` orders by the same fields: `Sort$SortBy` publishes the two every
    // type carries and adds that each type may support more.
    for sort_by in ["ProductTitle", "Visibility", "EntityId", "LastModifiedDate"] {
        let answer = list_entities(
            &router,
            json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "Sort": {"SortBy": sort_by, "SortOrder": "ASCENDING"},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {sort_by:?}: an attribute of the listed type must be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_ids(&answer).len(),
            2,
            "case {sort_by:?}: both products must be served, got {:?}",
            answer.body
        );
    }

    // A field of ANOTHER type orders nothing on this one, so it is refused.
    for sort_by in ["BuyerAccounts", "SolutionId", "StartTime"] {
        let refused = list_entities(
            &router,
            json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "Sort": {"SortBy": sort_by},
            }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {sort_by:?}: a field of another type must be refused, got {} with {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.text("/Message").contains(sort_by),
            "case {sort_by:?}: the refusal must name the attribute asked for, got {:?}",
            refused.body
        );
    }

    // Every type the model declares a sort shape for publishes its own sort fields, and
    // none of them repeats the two every type carries.
    for entity_type in SHAPED_ENTITY_TYPES {
        let fields = entity_type_sort_fields(entity_type);
        assert!(
            !fields.is_empty(),
            "case {entity_type:?}: the type must publish the fields its own sort shape names"
        );
        for common in ENTITY_SORT_BY {
            assert!(
                !fields.contains(common),
                "case {entity_type:?}: {common} is published for every type, got {fields:?}"
            );
        }
    }
}

/// A `EntityTypeFilters` or `EntityTypeSort` member belonging to another entity type
/// selects and orders nothing on the type being listed, so it is refused.
#[tokio::test]
async fn a_per_type_member_must_name_the_type_being_listed() {
    let router = emulator();
    let _ = created_entities(
        &router,
        &[(
            "AmiProduct",
            json!({"ProductTitle": "mismatched", "Visibility": "Limited"}),
        )],
    )
    .await;

    // (case, EntityType, the member and the value that names another type)
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "an offer's filters against a product",
            "AmiProduct",
            "EntityTypeFilters",
            json!({"OfferFilters": {"State": {"ValueList": ["Released"]}}}),
        ),
        (
            "a product's filters against an offer",
            "Offer",
            "EntityTypeFilters",
            json!({"AmiProductFilters": {"Visibility": {"ValueList": ["Public"]}}}),
        ),
        (
            "an offer set's sort against a product",
            "AmiProduct",
            "EntityTypeSort",
            json!({"OfferSetSort": {"SortBy": "Name"}}),
        ),
        (
            "a product's sort against a resale authorization",
            "ResaleAuthorization",
            "EntityTypeSort",
            json!({"AmiProductSort": {"SortBy": "ProductTitle"}}),
        ),
    ];

    for (case, entity_type, member, value) in cases {
        let mut request = json!({"Catalog": CATALOG, "EntityType": entity_type});
        request[member] = value;
        let refused = list_entities(&router, request).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {case:?}: must be refused, got {} with {:?}",
            refused.status,
            refused.body
        );
        let message = refused.text("/Message");
        assert!(
            message.contains(member) && message.contains(entity_type),
            "case {case:?}: the refusal must name the member and the type being listed, \
             got {message:?}"
        );
    }
}

/// `EntitySummary` reports the per-type summary of the listed entity's OWN type, with each
/// member read from the details member of that name — and no summary belonging to any
/// other type.
#[tokio::test]
async fn the_per_type_summary_reports_the_listed_types_own_members() {
    let router = emulator();
    // (entity_type, details, the summary members that must be reported)
    let cases: Vec<(&str, Value, BTreeMap<&str, Value>)> = vec![
        (
            "AmiProduct",
            json!({"ProductTitle": "an ami", "Visibility": "Public"}),
            BTreeMap::from([
                ("ProductTitle", json!("an ami")),
                ("Visibility", json!("Public")),
            ]),
        ),
        (
            "Offer",
            json!({
                "Name": "an offer",
                "ProductId": "prod-7",
                "ResaleAuthorizationId": "resale-7",
                "ReleaseDate": "2026-04-04T00:00:00Z",
                "AvailabilityEndDate": "2027-04-04T00:00:00Z",
                "BuyerAccounts": ["111122223333"],
                "State": "Released",
                "Targeting": ["BuyerAccounts"],
                "OfferSetId": "offerset-abc123",
                "TargetAgreementId": "agreement-7",
                "TargetAgreementIntent": "Renew",
                "CreatedBySource": "Seller",
            }),
            BTreeMap::from([
                ("Name", json!("an offer")),
                ("ProductId", json!("prod-7")),
                ("ResaleAuthorizationId", json!("resale-7")),
                ("ReleaseDate", json!("2026-04-04T00:00:00Z")),
                ("AvailabilityEndDate", json!("2027-04-04T00:00:00Z")),
                ("BuyerAccounts", json!(["111122223333"])),
                ("State", json!("Released")),
                ("Targeting", json!(["BuyerAccounts"])),
                ("OfferSetId", json!("offerset-abc123")),
                ("TargetAgreementId", json!("agreement-7")),
                ("TargetAgreementIntent", json!("Renew")),
                ("CreatedBySource", json!("Seller")),
            ]),
        ),
        (
            "OfferSet",
            json!({
                "Name": "a set",
                "State": "Released",
                "ReleaseDate": "2026-05-05T00:00:00Z",
                "AssociatedOfferIds": ["offer-a", "offer-b"],
                "SolutionId": "solution-7",
            }),
            BTreeMap::from([
                ("Name", json!("a set")),
                ("State", json!("Released")),
                ("ReleaseDate", json!("2026-05-05T00:00:00Z")),
                ("AssociatedOfferIds", json!(["offer-a", "offer-b"])),
                ("SolutionId", json!("solution-7")),
            ]),
        ),
        (
            "ResaleAuthorization",
            json!({
                "Name": "a resale",
                "ProductId": "prod-8",
                "ProductName": "a product",
                "ManufacturerAccountId": "111122223333",
                "ManufacturerLegalName": "A Maker",
                "ResellerAccountID": "444455556666",
                "ResellerLegalName": "A Reseller",
                "Status": "Active",
                "OfferExtendedStatus": "Extended",
                "CreatedDate": "2026-06-06T00:00:00Z",
                "AvailabilityEndDate": "2027-06-06T00:00:00Z",
                "ResellerRole": "ChannelPartner",
                "SourceAuthorization": "resale-source",
                "IssuerAccountId": "777788889999",
            }),
            BTreeMap::from([
                ("Name", json!("a resale")),
                ("ProductId", json!("prod-8")),
                ("ProductName", json!("a product")),
                ("ManufacturerAccountId", json!("111122223333")),
                ("ManufacturerLegalName", json!("A Maker")),
                ("ResellerAccountID", json!("444455556666")),
                ("ResellerLegalName", json!("A Reseller")),
                ("Status", json!("Active")),
                ("OfferExtendedStatus", json!("Extended")),
                ("CreatedDate", json!("2026-06-06T00:00:00Z")),
                ("AvailabilityEndDate", json!("2027-06-06T00:00:00Z")),
                ("ResellerRole", json!("ChannelPartner")),
                ("SourceAuthorization", json!("resale-source")),
                ("IssuerAccountId", json!("777788889999")),
            ]),
        ),
        (
            "ExpressPrivateOfferConfiguration",
            json!({"ProductId": "prod-9", "State": "INACTIVE"}),
            BTreeMap::from([
                ("ProductId", json!("prod-9")),
                ("State", json!("INACTIVE")),
            ]),
        ),
        (
            "ContainerProduct",
            json!({"ProductTitle": "a container"}),
            BTreeMap::from([
                ("ProductTitle", json!("a container")),
                // No change named a visibility, so the entity carries the one a created
                // entity has, which the common member reports too.
                ("Visibility", json!("Limited")),
            ]),
        ),
    ];

    let specs: Vec<(&str, Value)> = cases
        .iter()
        .map(|(entity_type, details, _)| (*entity_type, details.clone()))
        .collect();
    let ids = created_entities(&router, &specs).await;

    for (index, (entity_type, submitted, expected)) in cases.iter().enumerate() {
        let member = SUMMARY_MEMBERS
            .iter()
            .find(|(held, _)| held == entity_type)
            .map(|(_, member)| *member)
            .unwrap_or_else(|| panic!("case {entity_type:?}: the type must have a summary member"));
        let answer = list_entities(
            &router,
            json!({"Catalog": CATALOG, "EntityType": entity_type}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {entity_type:?}: ListEntities must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let summaries = answer.list("/EntitySummaryList");
        let summary = summaries
            .iter()
            .find(|item| item["EntityId"] == json!(ids[index]))
            .unwrap_or_else(|| {
                panic!(
                    "case {entity_type:?}: the created entity must be listed, got {:?}",
                    answer.body
                )
            });

        for (field, wanted) in expected {
            assert_eq!(
                &summary[member][*field],
                wanted,
                "case {entity_type:?}: {member}.{field} must report what the details stored, \
                 got {:?}",
                summary[member]
            );
        }
        let mut reported: Vec<String> = summary[member]
            .as_object()
            .map(|held| held.keys().cloned().collect())
            .unwrap_or_default();
        reported.sort();
        assert_eq!(
            reported,
            expected
                .keys()
                .map(|field| (*field).to_string())
                .collect::<Vec<String>>(),
            "case {entity_type:?}: {member} must report exactly the members the details \
             carry, got {:?}",
            summary[member]
        );
        for (_, other) in SUMMARY_MEMBERS.iter().filter(|(_, held)| *held != member) {
            assert_eq!(
                summary.get(*other),
                None,
                "case {entity_type:?}: {other} belongs to another type, got {summary:?}"
            );
        }

        // The summary is a projection of the persisted entity, so the read of that same
        // entity reports the details it was computed from.
        let described = describe_entity(&router, &ids[index]).await;
        let details: Value = serde_json::from_str(&described.text("/Details"))
            .unwrap_or_else(|error| panic!("case {entity_type:?}: Details must be JSON: {error}"));
        for (field, wanted) in submitted
            .as_object()
            .expect("every fixture's details are an object")
        {
            assert_eq!(
                &details[field], wanted,
                "case {entity_type:?}: the stored details must carry {field}, got {details:?}"
            );
        }
    }
}
