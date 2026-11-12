// SPDX-License-Identifier: CC0-1.0

//! The attributes a `ListEntities` of one entity type sorts on, end to end over HTTP.
//!
//! One rule is under test: the attributes a type is ordered by are the ones the model
//! declares for it — `Sort$SortBy` publishes `LastModifiedDate` (its default) and
//! `EntityId` for every type and adds "each `EntityType` might support additional
//! fields", which the model names in that type's own `*SortBy` enum. So every value of
//! that enum orders a list of that type, every other attribute name is refused, and the
//! refusal publishes exactly that set.
//!
//! The table below is the model's own answer (`smithy_query(query="shape",
//! shape="AmiProductSortBy,ContainerProductSortBy,DataProductSortBy,
//! ExpressPrivateOfferConfigurationSortBy,MachineLearningProductSortBy,OfferSortBy,
//! OfferSetSortBy,ResaleAuthorizationSortBy,SaaSProductSortBy")`, each enum's values in
//! the order it declares them), spelled out here so the service is judged against the
//! model rather than against its own constants — and extended with the entity types the
//! model declares no sort shape for, which carry the two common attributes alone.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`), which
/// is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Every entity type under test and the attributes the model declares it sorts on.
///
/// The first nine are the types whose own `*SortBy` enum the model declares, each row that
/// enum's values; `Seller` and `Solution` are types this catalog serves
/// (`ListEntitiesRequest$EntityType`'s documented values) that the model declares no sort
/// shape for, so they carry only the two attributes `Sort$SortBy` publishes for every
/// type.
const MODELLED_SORT_BY: &[(&str, &[&str])] = &[
    (
        "AmiProduct",
        &["EntityId", "LastModifiedDate", "ProductTitle", "Visibility"],
    ),
    (
        "ContainerProduct",
        &[
            "EntityId",
            "LastModifiedDate",
            "ProductTitle",
            "Visibility",
            "CompatibleAWSServices",
        ],
    ),
    (
        "DataProduct",
        &["EntityId", "ProductTitle", "Visibility", "LastModifiedDate"],
    ),
    (
        "ExpressPrivateOfferConfiguration",
        &["EntityId", "LastModifiedDate", "ProductId", "State"],
    ),
    (
        "MachineLearningProduct",
        &["EntityId", "LastModifiedDate", "ProductTitle", "Visibility"],
    ),
    (
        "Offer",
        &[
            "EntityId",
            "Name",
            "ProductId",
            "ResaleAuthorizationId",
            "ReleaseDate",
            "AvailabilityEndDate",
            "BuyerAccounts",
            "State",
            "Targeting",
            "LastModifiedDate",
            "OfferSetId",
            "TargetAgreementId",
            "TargetAgreementIntent",
            "CreatedBySource",
        ],
    ),
    (
        "OfferSet",
        &[
            "Name",
            "State",
            "ReleaseDate",
            "SolutionId",
            "EntityId",
            "LastModifiedDate",
        ],
    ),
    (
        "ResaleAuthorization",
        &[
            "EntityId",
            "Name",
            "ProductId",
            "ProductName",
            "ManufacturerAccountId",
            "ManufacturerLegalName",
            "ResellerAccountID",
            "ResellerLegalName",
            "Status",
            "OfferExtendedStatus",
            "CreatedDate",
            "AvailabilityEndDate",
            "LastModifiedDate",
            "ResellerRole",
            "SourceAuthorization",
            "IssuerAccountId",
        ],
    ),
    (
        "SaaSProduct",
        &[
            "EntityId",
            "ProductTitle",
            "Visibility",
            "LastModifiedDate",
            "DeliveryOptionTypes",
        ],
    ),
    ("Seller", &["EntityId", "LastModifiedDate"]),
    ("Solution", &["EntityId", "LastModifiedDate"]),
];

/// An attribute name no `*SortBy` enum of the model declares, and no entity type is
/// therefore ordered by.
const UNDECLARED_ATTRIBUTE: &str = "NoSuchAttribute";

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

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn list_entities(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/ListEntities", request).await
}

/// Every attribute any `*SortBy` enum of the model declares, which is the pool a case
/// draws an attribute belonging to ANOTHER type from.
fn every_modelled_attribute() -> Vec<&'static str> {
    let mut attributes: Vec<&'static str> = MODELLED_SORT_BY
        .iter()
        .flat_map(|(_, values)| values.iter().copied())
        .collect();
    attributes.sort_unstable();
    attributes.dedup();
    attributes
}

/// Each entity type is ordered by exactly the attributes the model declares for it: every
/// one of them is served, and every other attribute name is refused with the one sentence
/// the service states a sort attribute it does not order by in — which publishes no set,
/// because `Sort$SortBy` declares none.
#[tokio::test]
async fn a_list_sorts_on_the_attributes_the_model_declares_for_its_type() {
    let router = emulator();
    let pool = every_modelled_attribute();

    for (entity_type, declared) in MODELLED_SORT_BY {
        for sort_by in *declared {
            let case = format!("{entity_type} by {sort_by}");
            let answer = list_entities(
                &router,
                json!({
                    "Catalog": CATALOG,
                    "EntityType": entity_type,
                    "Sort": {"SortBy": sort_by},
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: an attribute the model declares for this type must order \
                 the list, got {} with {:?}",
                answer.status, answer.body
            );
        }

        // Every attribute the model declares for some OTHER type and not for this one,
        // plus a name no enum of the model declares at all.
        let refusable: Vec<&str> = pool
            .iter()
            .copied()
            .chain([UNDECLARED_ATTRIBUTE])
            .filter(|attribute| !declared.contains(attribute))
            .collect();
        assert!(
            !refusable.is_empty(),
            "case {entity_type:?}: the table must offer an attribute this type is not \
             ordered by"
        );

        for sort_by in refusable {
            let case = format!("{entity_type} by {sort_by}");
            let refused = list_entities(
                &router,
                json!({
                    "Catalog": CATALOG,
                    "EntityType": entity_type,
                    "Sort": {"SortBy": sort_by},
                }),
            )
            .await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (422, "ValidationException"),
                "case {case:?}: an attribute the model does not declare for this type must \
                 be refused, got {} with {:?}",
                refused.status,
                refused.body
            );
            let message = refused.text("/Message");
            assert_eq!(
                message,
                format!("[Requested sort by '{sort_by}' is invalid. It should be one of '[]'.]"),
                "case {case:?}: the refusal must name the attribute asked for and render the \
                 unpublished allowed-value set as an empty list, got {message:?}"
            );
        }
    }
}

/// An attribute the model declares for a type does not merely pass the check: it orders
/// the page the catalog serves, over two types whose own sort enums name different
/// attributes.
#[tokio::test]
async fn a_declared_attribute_orders_the_page_it_is_asked_for() {
    let router = emulator();
    // (entity_type, the details of each entity, the attribute to order by, the member of
    // the per-type summary that reports it)
    let cases: Vec<(&str, Vec<Value>, &str, &str)> = vec![
        (
            "SaaSProduct",
            vec![
                json!({"ProductTitle": "charlie saas", "Visibility": "Limited"}),
                json!({"ProductTitle": "alpha saas", "Visibility": "Public"}),
                json!({"ProductTitle": "bravo saas", "Visibility": "Restricted"}),
            ],
            "ProductTitle",
            "/SaaSProductSummary/ProductTitle",
        ),
        (
            "ResaleAuthorization",
            vec![
                json!({"Name": "charlie resale", "Status": "Active"}),
                json!({"Name": "alpha resale", "Status": "Draft"}),
                json!({"Name": "bravo resale", "Status": "Restricted"}),
            ],
            "Name",
            "/ResaleAuthorizationSummary/Name",
        ),
    ];

    for (entity_type, entities, _, _) in &cases {
        for details in entities {
            let started = call(
                &router,
                "/StartChangeSet",
                json!({
                    "Catalog": CATALOG,
                    "ChangeSet": [{
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": format!("{entity_type}@1.0")},
                        "DetailsDocument": details,
                    }],
                }),
            )
            .await;
            assert_eq!(
                started.status, 200,
                "case {entity_type:?}: the fixture must start a change set, got {} with {:?}",
                started.status, started.body
            );
        }
    }
    wait_for_success();

    for (entity_type, entities, sort_by, pointer) in &cases {
        let case = format!("{entity_type} by {sort_by}");
        let answer = list_entities(
            &router,
            json!({
                "Catalog": CATALOG,
                "EntityType": entity_type,
                "Sort": {"SortBy": sort_by, "SortOrder": "ASCENDING"},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the list must be served, got {} with {:?}",
            answer.status, answer.body
        );
        let served: Vec<String> = answer
            .list("/EntitySummaryList")
            .iter()
            .map(|item| {
                item.pointer(pointer)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let mut expected: Vec<String> = entities
            .iter()
            .map(|details| {
                details[*sort_by]
                    .as_str()
                    .expect("each fixture carries the attribute it is ordered by")
                    .to_string()
            })
            .collect();
        expected.sort();
        assert_eq!(
            served, expected,
            "case {case:?}: the page must read {sort_by} ascending, got {served:?}"
        );
    }
}
