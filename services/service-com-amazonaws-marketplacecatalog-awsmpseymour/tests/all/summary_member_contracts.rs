// SPDX-License-Identifier: CC0-1.0

//! The contract of every details member `ListEntities` projects into an `EntitySummary`,
//! end to end over HTTP.
//!
//! One rule is under test: a details member the catalog READS to report it in an
//! `EntitySummary` member may only hold what the model declares for that member — the
//! values its enum publishes, or the `@length` and `@pattern` of the string shape it
//! targets — so a change carrying a value outside it fails its change set (as a change
//! carrying an unknown `Visibility` already does) and `ListEntities` reports no summary
//! member a client validating against the model cannot read.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
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

/// The `ErrorDetail$ErrorCode` the catalog reports for a details value it cannot read.
const INVALID_VALUE: &str = "INVALID_VALUE";

/// A value no enumerated summary member publishes, used as the "outside the set" case of
/// every such member.
const OUTSIDE_EVERY_SET: &str = "NotAPublishedValue";

/// The five entity types whose summary calls the entity's name `ProductTitle`
/// (`AmiProductSummary$ProductTitle` and the four product summaries beside it); every
/// other type's summary calls it `Name` (`OfferSummary$Name`, `OfferSetSummary$Name`,
/// `ResaleAuthorizationSummary$Name`).
const PRODUCT_TYPES: &[&str] = &[
    "AmiProduct",
    "ContainerProduct",
    "DataProduct",
    "MachineLearningProduct",
    "SaaSProduct",
];

/// The enumerated members of the per-type summaries, with the values the model publishes
/// for each: read off the enums those members target
/// (`smithy_query(query="shape", shape="OfferStateString,OfferTargetingString,
/// OfferTargetAgreementIntentString,OfferCreatedBySourceString,OfferSetStateString,
/// ResaleAuthorizationStatusString,ResaleAuthorizationResellerRoleString,
/// ExpressPrivateOfferConfigurationStateString")`).
///
/// `(entity type, details member, the pointer the summary reports it at, published
/// values, whether the member is a LIST of them)`.
const ENUMERATED_MEMBERS: &[(&str, &str, &str, &[&str], bool)] = &[
    (
        "Offer",
        "State",
        "/OfferSummary/State",
        &["Draft", "Released"],
        false,
    ),
    (
        "Offer",
        "Targeting",
        "/OfferSummary/Targeting",
        &[
            "BuyerAccounts",
            "ParticipatingPrograms",
            "CountryCodes",
            "None",
        ],
        true,
    ),
    (
        "Offer",
        "TargetAgreementIntent",
        "/OfferSummary/TargetAgreementIntent",
        &["Renew"],
        false,
    ),
    (
        "Offer",
        "CreatedBySource",
        "/OfferSummary/CreatedBySource",
        &["Seller", "AwsMarketplace"],
        false,
    ),
    (
        "OfferSet",
        "State",
        "/OfferSetSummary/State",
        &["Draft", "Released"],
        false,
    ),
    (
        "ResaleAuthorization",
        "Status",
        "/ResaleAuthorizationSummary/Status",
        &["Draft", "Active", "Restricted"],
        false,
    ),
    (
        "ResaleAuthorization",
        "ResellerRole",
        "/ResaleAuthorizationSummary/ResellerRole",
        &["ChannelPartner", "Distributor"],
        false,
    ),
    (
        "ExpressPrivateOfferConfiguration",
        "State",
        "/ExpressPrivateOfferConfigurationSummary/State",
        &["ACTIVE", "INACTIVE"],
        false,
    ),
];

/// How long after the start a change set has certainly ended its run: the two spans the
/// implementation publishes, plus a margin for a slow machine.
fn wait_for_the_end_of_the_run() {
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

async fn describe_change_set(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={id}"),
        None,
    )
    .await
}

/// The entities of `entity_type` the catalog holds, as `ListEntities` reports them.
async fn list_entities(router: &MultiServiceRouter, entity_type: &str) -> Vec<Value> {
    let answer = call(
        router,
        "POST",
        "/ListEntities",
        Some(json!({"Catalog": CATALOG, "EntityType": entity_type, "MaxResults": 50})),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "listing {entity_type} must answer a page, got {} with {:?}",
        answer.status, answer.body
    );
    answer.list("/EntitySummaryList")
}

/// The details member an entity of `entity_type` carries its name in.
fn name_member(entity_type: &str) -> &'static str {
    if PRODUCT_TYPES.contains(&entity_type) {
        "ProductTitle"
    } else {
        "Name"
    }
}

/// The change type that CREATES an entity of `entity_type`, as the catalog's change-type
/// catalogue names it.
fn create_change_type(entity_type: &str) -> &'static str {
    match entity_type {
        "Offer" => "CreateOffer",
        "ResaleAuthorization" => "CreateResaleAuthorization",
        _ => "CreateProduct",
    }
}

/// `details` with `member` set to `value`.
fn with(details: &Value, member: &str, value: Value) -> Value {
    let mut members: Map<String, Value> = details
        .as_object()
        .cloned()
        .unwrap_or_default();
    members.insert(member.to_string(), value);
    Value::Object(members)
}

/// One case: the case name, the entity type its change creates, and the details that
/// change carries.
type Row = (String, &'static str, Value);

/// One case of [`a_details_value_outside_its_summary_members_contract_fails_the_change_set`]:
/// a [`Row`] plus the member the reported defect names.
type RefusedRow = (String, &'static str, Value, &'static str);

/// One case of [`a_details_value_inside_its_summary_members_contract_is_reported`]: a
/// [`Row`] plus the `(pointer, value)` pairs the served summary has to report.
type ReportedRow = (String, &'static str, Value, Vec<(&'static str, Value)>);

/// Start one change set per row, each creating a new entity of its own type, and wait
/// once for the clock to end every run. Answers each row's change set id, in order.
async fn run_rows(router: &MultiServiceRouter, rows: &[Row]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::with_capacity(rows.len());
    for (case, entity_type, details) in rows {
        let started = call(
            router,
            "POST",
            "/StartChangeSet",
            Some(json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": create_change_type(entity_type),
                    "Entity": {"Type": format!("{entity_type}@1.0")},
                    "DetailsDocument": details,
                }],
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: content only applying can judge must be accepted, got {} \
             with {:?}",
            started.status, started.body
        );
        ids.push(started.text("/ChangeSetId"));
    }
    wait_for_the_end_of_the_run();
    ids
}

/// Assert that change set `id` failed on the one change it carried, with one
/// `INVALID_VALUE` error naming `member`.
async fn assert_refused(router: &MultiServiceRouter, case: &str, id: &str, member: &str) {
    let described = describe_change_set(router, id).await;
    assert_eq!(
        described.text("/Status"),
        "FAILED",
        "case {case:?}: a value the summary member cannot hold must fail the change set, \
         got {:?}",
        described.body
    );
    assert_eq!(
        described.text("/FailureCode"),
        "CLIENT_ERROR",
        "case {case:?}: the defect is in the request, got {:?}",
        described.body
    );
    let changes = described.list("/ChangeSet");
    let errors = changes
        .first()
        .map(|change| {
            change["ErrorDetailList"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .unwrap_or_default();
    assert_eq!(
        errors.len(),
        1,
        "case {case:?}: the change must report the one defect it carries, got {:?}",
        described.body
    );
    let code = errors[0]["ErrorCode"].as_str().unwrap_or_default();
    let message = errors[0]["ErrorMessage"].as_str().unwrap_or_default();
    assert_eq!(
        code, INVALID_VALUE,
        "case {case:?}: a value outside a member's contract is an {INVALID_VALUE}, got \
         {:?}",
        errors[0]
    );
    assert!(
        message.starts_with(&format!("Details.{member} accepts ")),
        "case {case:?}: the error must name the member it is about and what that member \
         accepts, got {:?}",
        errors[0]
    );
}

/// A change carrying a value one of the summary members cannot hold fails its change set
/// with that member's `INVALID_VALUE`, and the catalog holds no entity from it — so no
/// `ListEntities` answer reports the value.
///
/// The rows span the forms the model declares for those members: below and past each
/// published length bound, outside a pattern, the wrong JSON type, a list where the member
/// is one value and one value where the member is a list, and a list longer than its own
/// bound.
#[tokio::test]
async fn a_details_value_outside_its_summary_members_contract_fails_the_change_set() {
    // `OfferBuyerAccountsList` is `@length(min: 0, max: 26)`, so 27 account ids is one
    // past its bound; `OfferTargetingList` is `@length(min: 0, max: 4)`.
    let twenty_seven_accounts: Vec<Value> = (0..27)
        .map(|index| json!(format!("{:012}", 111_122_223_333u64 + index)))
        .collect();
    let five_targetings = json!([
        "BuyerAccounts",
        "ParticipatingPrograms",
        "CountryCodes",
        "None",
        "BuyerAccounts",
    ]);
    // (case, entity type, the details one change carries, the member the error names)
    let rows: Vec<RefusedRow> = vec![
        (
            "an empty product title, below EntityNameString's @length(min: 1)".to_string(),
            "AmiProduct",
            json!({"ProductTitle": ""}),
            "ProductTitle",
        ),
        (
            "a product title one character past @length(max: 255)".to_string(),
            "AmiProduct",
            json!({"ProductTitle": "a".repeat(256)}),
            "ProductTitle",
        ),
        (
            "a product title whose first character is whitespace".to_string(),
            "SaaSProduct",
            json!({"ProductTitle": " spaced"}),
            "ProductTitle",
        ),
        (
            "a product title carrying a line break".to_string(),
            "DataProduct",
            json!({"ProductTitle": "two\nlines"}),
            "ProductTitle",
        ),
        (
            "a product title that is not a string".to_string(),
            "ContainerProduct",
            json!({"ProductTitle": 7}),
            "ProductTitle",
        ),
        (
            "an offer name past OfferNameString's @length(max: 150)".to_string(),
            "Offer",
            json!({"Name": "o".repeat(151)}),
            "Name",
        ),
        (
            "an empty offer set name".to_string(),
            "OfferSet",
            json!({"Name": ""}),
            "Name",
        ),
        (
            "a resale authorization name past @length(max: 255)".to_string(),
            "ResaleAuthorization",
            json!({"Name": "r".repeat(256)}),
            "Name",
        ),
        (
            "a release date that is not an instant at all".to_string(),
            "Offer",
            json!({"Name": "an offer", "ReleaseDate": "tomorrow"}),
            "ReleaseDate",
        ),
        (
            "a release date one character below DateTimeISO8601's @length(20, 20)".to_string(),
            "Offer",
            json!({"ReleaseDate": "2026-01-01T00:00:00"}),
            "ReleaseDate",
        ),
        (
            "a release date one character past that length".to_string(),
            "Offer",
            json!({"ReleaseDate": "2026-01-01T00:00:00Z0"}),
            "ReleaseDate",
        ),
        (
            "a release date of the right length with a month outside the pattern".to_string(),
            "Offer",
            json!({"ReleaseDate": "2026-13-01T00:00:00Z"}),
            "ReleaseDate",
        ),
        (
            "a created date of the right length with an hour outside the pattern".to_string(),
            "ResaleAuthorization",
            json!({"CreatedDate": "2026-06-06T24:00:00Z"}),
            "CreatedDate",
        ),
        (
            "an offer set id without the prefix OfferSetIdString requires".to_string(),
            "Offer",
            json!({"OfferSetId": "abc123"}),
            "OfferSetId",
        ),
        (
            "an offer set id one character past @length(max: 50)".to_string(),
            "Offer",
            json!({"OfferSetId": format!("offerset-{}", "a".repeat(42))}),
            "OfferSetId",
        ),
        (
            "a solution id too short for the id pattern".to_string(),
            "OfferSet",
            json!({"SolutionId": "s"}),
            "SolutionId",
        ),
        (
            "an express private offer configuration product id carrying a space".to_string(),
            "ExpressPrivateOfferConfiguration",
            json!({"ProductId": "prod 9"}),
            "ProductId",
        ),
        (
            "a manufacturer account id of 11 digits".to_string(),
            "ResaleAuthorization",
            json!({"ManufacturerAccountId": "11112222333"}),
            "ManufacturerAccountId",
        ),
        (
            "a reseller account id of 13 digits".to_string(),
            "ResaleAuthorization",
            json!({"ResellerAccountID": "4444555566667"}),
            "ResellerAccountID",
        ),
        (
            "an issuer account id that is not all digits".to_string(),
            "ResaleAuthorization",
            json!({"IssuerAccountId": "77778888999a"}),
            "IssuerAccountId",
        ),
        (
            "a buyer account outside the 12-digit form".to_string(),
            "Offer",
            json!({"BuyerAccounts": ["12345"]}),
            "BuyerAccounts",
        ),
        (
            "a buyer account that is not a string".to_string(),
            "Offer",
            json!({"BuyerAccounts": [111122223333u64]}),
            "BuyerAccounts",
        ),
        (
            "one buyer account past the list's @length(max: 26)".to_string(),
            "Offer",
            json!({"BuyerAccounts": twenty_seven_accounts}),
            "BuyerAccounts",
        ),
        (
            "one targeting value past the list's @length(max: 4)".to_string(),
            "Offer",
            json!({"Targeting": five_targetings}),
            "Targeting",
        ),
        (
            "an associated offer id too short for the id pattern".to_string(),
            "OfferSet",
            json!({"AssociatedOfferIds": ["o"]}),
            "AssociatedOfferIds",
        ),
        (
            "one value where the summary member is a list of them".to_string(),
            "Offer",
            json!({"Targeting": "BuyerAccounts"}),
            "Targeting",
        ),
        (
            "a list where the summary member is one value".to_string(),
            "Offer",
            json!({"State": ["Draft"]}),
            "State",
        ),
        (
            "an empty offer product id".to_string(),
            "Offer",
            json!({"ProductId": ""}),
            "ProductId",
        ),
        (
            "a target agreement id one character past @length(max: 64)".to_string(),
            "Offer",
            json!({"TargetAgreementId": "t".repeat(65)}),
            "TargetAgreementId",
        ),
    ];

    let router = emulator();
    let started: Vec<Row> = rows
        .iter()
        .map(|(case, entity_type, details, _)| (case.clone(), *entity_type, details.clone()))
        .collect();
    let ids = run_rows(&router, &started).await;

    for ((case, entity_type, _, member), id) in rows.iter().zip(&ids) {
        assert_refused(&router, case, id, member).await;
        let listed = list_entities(&router, entity_type).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a change set that failed applies nothing, so no {entity_type} \
             is listed, got {listed:?}"
        );
    }
}

/// The same rule's other side: a value the summary member CAN hold is applied and reported
/// in that member, including at each published bound.
///
/// The rows differ along the entity type, the member, and where in the contract the value
/// sits (the shortest and the longest admitted string, a list at its own bound, an instant,
/// an account id, a prefixed id).
#[tokio::test]
async fn a_details_value_inside_its_summary_members_contract_is_reported() {
    let longest_title = "a".repeat(255);
    let longest_offer_name = "o".repeat(150);
    let longest_agreement_id = "t".repeat(64);
    // `OfferSetIdString` is `@length(min: 1, max: 50)` behind the `offerset-` prefix.
    let longest_offer_set_id = format!("offerset-{}", "a".repeat(41));
    // `OfferBuyerAccountsList` holds at most 26 account ids, `OfferSetAssociatedOfferIdsList`
    // at most 10 offer ids.
    let twenty_six_accounts: Vec<Value> = (0..26)
        .map(|index| json!(format!("{:012}", 111_122_223_333u64 + index)))
        .collect();
    let ten_offer_ids: Vec<Value> = (0..10)
        .map(|index| json!(format!("offer-{index:03}")))
        .collect();

    // (case, entity type, the details one change carries, the members the summary reports)
    let rows: Vec<ReportedRow> = vec![
        (
            "a product title of one character, the shortest admitted".to_string(),
            "MachineLearningProduct",
            json!({"ProductTitle": "x", "Visibility": "Draft"}),
            vec![
                ("/Name", json!("x")),
                ("/MachineLearningProductSummary/ProductTitle", json!("x")),
            ],
        ),
        (
            "a product title of 255 characters, exactly on @length(max)".to_string(),
            "AmiProduct",
            json!({"ProductTitle": longest_title.clone()}),
            vec![
                ("/Name", json!(longest_title.clone())),
                (
                    "/AmiProductSummary/ProductTitle",
                    json!(longest_title.clone()),
                ),
            ],
        ),
        (
            "an offer name of 150 characters, exactly on OfferNameString's max".to_string(),
            "Offer",
            json!({"Name": longest_offer_name.clone()}),
            vec![
                ("/Name", json!(longest_offer_name.clone())),
                ("/OfferSummary/Name", json!(longest_offer_name.clone())),
            ],
        ),
        (
            "every member of an offer's summary, each at a bound of its own".to_string(),
            "Offer",
            json!({
                "Name": "an offer at the bounds",
                "ProductId": "p",
                "ResaleAuthorizationId": "resale-7",
                "ReleaseDate": "2026-04-04T00:00:00Z",
                "AvailabilityEndDate": "2027-12-31T23:59:59Z",
                "BuyerAccounts": twenty_six_accounts.clone(),
                "State": "Draft",
                "Targeting": ["BuyerAccounts", "ParticipatingPrograms", "CountryCodes", "None"],
                "OfferSetId": longest_offer_set_id.clone(),
                "TargetAgreementId": longest_agreement_id.clone(),
                "TargetAgreementIntent": "Renew",
                "CreatedBySource": "AwsMarketplace",
            }),
            vec![
                ("/OfferSummary/Name", json!("an offer at the bounds")),
                ("/OfferSummary/ProductId", json!("p")),
                ("/OfferSummary/ResaleAuthorizationId", json!("resale-7")),
                ("/OfferSummary/ReleaseDate", json!("2026-04-04T00:00:00Z")),
                (
                    "/OfferSummary/AvailabilityEndDate",
                    json!("2027-12-31T23:59:59Z"),
                ),
                (
                    "/OfferSummary/BuyerAccounts",
                    Value::Array(twenty_six_accounts.clone()),
                ),
                ("/OfferSummary/State", json!("Draft")),
                (
                    "/OfferSummary/Targeting",
                    json!(["BuyerAccounts", "ParticipatingPrograms", "CountryCodes", "None"]),
                ),
                (
                    "/OfferSummary/OfferSetId",
                    json!(longest_offer_set_id.clone()),
                ),
                (
                    "/OfferSummary/TargetAgreementId",
                    json!(longest_agreement_id.clone()),
                ),
                ("/OfferSummary/TargetAgreementIntent", json!("Renew")),
                ("/OfferSummary/CreatedBySource", json!("AwsMarketplace")),
            ],
        ),
        (
            "every member of an offer set's summary".to_string(),
            "OfferSet",
            json!({
                "Name": "a set at the bounds",
                "State": "Released",
                "ReleaseDate": "2026-05-05T00:00:00Z",
                "AssociatedOfferIds": ten_offer_ids.clone(),
                "SolutionId": "s-1",
            }),
            vec![
                ("/OfferSetSummary/Name", json!("a set at the bounds")),
                ("/OfferSetSummary/State", json!("Released")),
                (
                    "/OfferSetSummary/ReleaseDate",
                    json!("2026-05-05T00:00:00Z"),
                ),
                (
                    "/OfferSetSummary/AssociatedOfferIds",
                    Value::Array(ten_offer_ids.clone()),
                ),
                ("/OfferSetSummary/SolutionId", json!("s-1")),
            ],
        ),
        (
            "every member of a resale authorization's summary".to_string(),
            "ResaleAuthorization",
            json!({
                "Name": "a resale at the bounds",
                "ProductId": "prod-8",
                "ProductName": "a product",
                "ManufacturerAccountId": "111122223333",
                "ManufacturerLegalName": "A Maker",
                "ResellerAccountID": "444455556666",
                "ResellerLegalName": "A Reseller",
                "Status": "Restricted",
                "OfferExtendedStatus": "Extended",
                "CreatedDate": "2026-06-06T00:00:00Z",
                "AvailabilityEndDate": "2027-06-06T00:00:00Z",
                "ResellerRole": "Distributor",
                "SourceAuthorization": "resale-source",
                "IssuerAccountId": "777788889999",
            }),
            vec![
                (
                    "/ResaleAuthorizationSummary/Name",
                    json!("a resale at the bounds"),
                ),
                ("/ResaleAuthorizationSummary/ProductId", json!("prod-8")),
                (
                    "/ResaleAuthorizationSummary/ManufacturerAccountId",
                    json!("111122223333"),
                ),
                (
                    "/ResaleAuthorizationSummary/ResellerAccountID",
                    json!("444455556666"),
                ),
                ("/ResaleAuthorizationSummary/Status", json!("Restricted")),
                (
                    "/ResaleAuthorizationSummary/CreatedDate",
                    json!("2026-06-06T00:00:00Z"),
                ),
                (
                    "/ResaleAuthorizationSummary/ResellerRole",
                    json!("Distributor"),
                ),
                (
                    "/ResaleAuthorizationSummary/IssuerAccountId",
                    json!("777788889999"),
                ),
            ],
        ),
        (
            "an express private offer configuration's own members".to_string(),
            "ExpressPrivateOfferConfiguration",
            json!({"Name": "an express configuration", "ProductId": "prod-9", "State": "INACTIVE"}),
            vec![
                (
                    "/ExpressPrivateOfferConfigurationSummary/ProductId",
                    json!("prod-9"),
                ),
                (
                    "/ExpressPrivateOfferConfigurationSummary/State",
                    json!("INACTIVE"),
                ),
            ],
        ),
    ];

    let router = emulator();
    let started: Vec<Row> = rows
        .iter()
        .map(|(case, entity_type, details, _)| (case.clone(), *entity_type, details.clone()))
        .collect();
    let ids = run_rows(&router, &started).await;

    for ((case, entity_type, details, expected), id) in rows.iter().zip(&ids) {
        let described = describe_change_set(&router, id).await;
        assert_eq!(
            described.text("/Status"),
            "SUCCEEDED",
            "case {case:?}: values inside their members' contracts must be applied, got \
             {:?}",
            described.body
        );
        let summary = listed_by_name(&router, case, entity_type, details).await;
        for (pointer, value) in expected {
            assert_eq!(
                summary.pointer(pointer),
                Some(value),
                "case {case:?}: the summary must report the applied value at {pointer}, \
                 got {summary:?}"
            );
        }
    }
}

/// Every value the model publishes for an enumerated summary member is applied and
/// reported in that member, and a value outside the published set fails the change set.
///
/// The rows are every enumerated member of every per-type summary
/// ([`ENUMERATED_MEMBERS`]), so no member of the catalogue is judged by a rule of its own.
#[tokio::test]
async fn every_published_value_of_an_enumerated_summary_member_is_accepted_and_no_other_is() {
    let mut accepted: Vec<(String, &'static str, Value, &'static str, Value)> = Vec::new();
    let mut refused: Vec<(String, &'static str, Value, &'static str)> = Vec::new();
    for (entity_type, member, pointer, published, is_list) in ENUMERATED_MEMBERS {
        assert!(
            !published.contains(&OUTSIDE_EVERY_SET),
            "{entity_type}${member} must not publish the value this test holds outside \
             every set, got {published:?}"
        );
        for value in *published {
            let held = if *is_list {
                json!([value])
            } else {
                json!(value)
            };
            let named = with(
                &json!({}),
                name_member(entity_type),
                json!(format!("{entity_type}-{member}-{value}")),
            );
            accepted.push((
                format!("{entity_type}${member} = {value:?}"),
                entity_type,
                with(&named, member, held.clone()),
                pointer,
                held,
            ));
        }
        let outside = if *is_list {
            json!([OUTSIDE_EVERY_SET])
        } else {
            json!(OUTSIDE_EVERY_SET)
        };
        let named = with(
            &json!({}),
            name_member(entity_type),
            json!(format!("{entity_type}-{member}-outside")),
        );
        refused.push((
            format!("{entity_type}${member} outside the published set"),
            entity_type,
            with(&named, member, outside),
            member,
        ));
    }

    let router = emulator();
    let mut rows: Vec<Row> = accepted
        .iter()
        .map(|(case, entity_type, details, _, _)| (case.clone(), *entity_type, details.clone()))
        .collect();
    rows.extend(
        refused
            .iter()
            .map(|(case, entity_type, details, _)| (case.clone(), *entity_type, details.clone())),
    );
    let ids = run_rows(&router, &rows).await;

    for ((case, entity_type, details, pointer, value), id) in accepted.iter().zip(&ids) {
        let described = describe_change_set(&router, id).await;
        assert_eq!(
            described.text("/Status"),
            "SUCCEEDED",
            "case {case:?}: a published value must be applied, got {:?}",
            described.body
        );
        let summary = listed_by_name(&router, case, entity_type, details).await;
        assert_eq!(
            summary.pointer(pointer),
            Some(value),
            "case {case:?}: the summary must report the applied value at {pointer}, got \
             {summary:?}"
        );
    }
    for ((case, _, _, member), id) in refused.iter().zip(ids.iter().skip(accepted.len())) {
        assert_refused(&router, case, id, member).await;
    }
}

/// The `EntitySummary` of the entity the row's details name, found by the name those
/// details gave it.
async fn listed_by_name(
    router: &MultiServiceRouter,
    case: &str,
    entity_type: &str,
    details: &Value,
) -> Value {
    let name = details
        .pointer(&format!("/{}", name_member(entity_type)))
        .cloned()
        .unwrap_or(Value::Null);
    let listed = list_entities(router, entity_type).await;
    listed
        .iter()
        .find(|summary| summary.get("Name") == Some(&name))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the applied entity named {name:?} must be listed, got {listed:?}")
        })
}
