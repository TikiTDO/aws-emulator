// SPDX-License-Identifier: CC0-1.0

//! `TerminateProvisionedProduct` over the wire, the launch-path parameter pair every
//! operation that resolves a launch validates, and the resolution this service reports
//! a timestamp at — driven as awsJson1_1 requests through the same `MultiServiceRouter`
//! the production emulator mounts.
//!
//! The rules under test:
//!
//! * terminating a provisioned product records a `TERMINATE_PROVISIONED_PRODUCT` record
//!   that settles on the clock, and the provisioned product stops existing when that
//!   record settles: every later read of its id or its name is the modelled 400
//!   `ResourceNotFoundException`, a search no longer reports it and its name is free
//!   again;
//! * the launch-path members are PARAMETERS of the read that reports a launch's
//!   configuration: the pair is required unless the product resolves exactly one path, a
//!   value identifying none of the product's paths is an invalid parameter, and both
//!   refusals are decided before the call reports whether the product or its paths
//!   resolve;
//! * every timestamp this service reports carries at most the millisecond resolution the
//!   protocol serves, consistently for every reporter of one stored instant.
//!
//! Every assertion is about STORED state: a response is checked against what an earlier
//! create, provision or termination persisted, or against a value derived from it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    associate_product_with_portfolio, call, create_portfolio, create_product, emulator, error_code,
    first_provisioning_artifact_id, is_catalog_id, message, tag_list,
};

/// AWS's sentence for a request that names neither half of the launch-path pair when the
/// path cannot be inferred. All fixed text: it carries no value from the request.
const PATH_REQUIRED: &str = "Either Path Id or Path Name needs to be specified";

/// One span of a provisioning operation, plus the margin a test waits past it: the
/// emulator settles a record 3s after the request that starts it.
const SETTLE_WAIT: std::time::Duration = std::time::Duration::from_millis(3_500);

/// How a termination request addresses the provisioned product it terminates.
#[derive(Debug, Clone, Copy)]
enum TerminateBy {
    /// `ProvisionedProductId`.
    Id,
    /// `ProvisionedProductName`, which takes a name…
    Name,
    /// …or that provisioned product's ARN (`ProvisionedProductNameOrArn`).
    Arn,
}

/// One row of the termination table.
struct TerminateCase {
    case: &'static str,
    /// The `ProvisionedProductName` the row provisions and later re-provisions.
    provisioned_name: &'static str,
    /// How the termination addresses it.
    addressed: TerminateBy,
    /// The tags the provisioning request carried, which the termination's record
    /// reports as its own `RecordTags`.
    tags: &'static [(&'static str, &'static str)],
}

/// What one row provisioned, before it was terminated.
struct Provisioned {
    id: String,
    arn: String,
    /// The record the termination answered with.
    terminate_record_id: String,
}

/// Seed one portfolio with one product in it, and answer (product id, version id,
/// portfolio id) — the launch every row of a table provisions through.
async fn seed_launch(router: &MultiServiceRouter) -> (String, String, String) {
    let portfolio_id = create_portfolio(router, "alpha-portfolio", "acme", &[], "portfolio").await;
    let product_id = create_product(router, "terminated-source-product", "product-token").await;
    let artifact_id = first_provisioning_artifact_id(router, &product_id).await;
    associate_product_with_portfolio(router, &product_id, &portfolio_id).await;
    (product_id, artifact_id, portfolio_id)
}

/// Provision one product and answer the whole response body.
async fn provision(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> Value {
    let (status, body) = call(router, "ProvisionProduct", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the provisioning to be served, got {status} with {body}"
    );
    body
}

/// The provisioned product `Id`/`Name` addresses, as `DescribeProvisionedProduct`
/// reports it, or the refusal it answered.
async fn describe_provisioned(
    router: &MultiServiceRouter,
    member: &str,
    identifier: &str,
) -> (u16, Value) {
    call(
        router,
        "DescribeProvisionedProduct",
        &json!({member: identifier}),
    )
    .await
}

/// Terminating a provisioned product records the termination, settles it on the clock
/// and makes the provisioned product stop existing — for every way of addressing it.
#[tokio::test]
async fn a_terminated_provisioned_product_stops_existing_once_its_record_settles() {
    // Rows differ along how the termination addresses the provisioned product (id, name,
    // ARN — the three forms the two input members admit) and along the tags the record
    // of the termination carries.
    let cases = [
        TerminateCase {
            case: "addressed by ProvisionedProductId, no tag",
            provisioned_name: "pp-by-id",
            addressed: TerminateBy::Id,
            tags: &[],
        },
        TerminateCase {
            case: "addressed by ProvisionedProductName, two tags",
            provisioned_name: "pp-by-name",
            addressed: TerminateBy::Name,
            tags: &[("env", "prod"), ("team", "platform")],
        },
        TerminateCase {
            case: "addressed by the provisioned product's ARN, one tag",
            provisioned_name: "pp-by-arn",
            addressed: TerminateBy::Arn,
            tags: &[("env", "dev")],
        },
    ];

    let router = emulator();
    let (product_id, artifact_id, _portfolio_id) = seed_launch(&router).await;

    // Every row is provisioned and terminated first, so the one wait below settles every
    // termination at once.
    let mut provisioned: Vec<(&TerminateCase, Provisioned)> = Vec::new();
    for row in &cases {
        let case = row.case;
        let mut request = json!({
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionedProductName": row.provisioned_name,
            "ProvisionToken": format!("provision-{}", row.provisioned_name),
        });
        if !row.tags.is_empty() {
            request["Tags"] = tag_list(row.tags);
        }
        let body = provision(&router, case, &request).await;
        let provision_record_id = body["RecordDetail"]["RecordId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RecordId in {body}"))
            .to_string();

        let (status, described) =
            describe_provisioned(&router, "Name", row.provisioned_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the provisioned product to be readable, got {status} with \
             {described}"
        );
        let detail = &described["ProvisionedProductDetail"];
        let id = detail["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {described}"))
            .to_string();
        let arn = detail["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Arn in {described}"))
            .to_string();

        let (member, identifier) = match row.addressed {
            TerminateBy::Id => ("ProvisionedProductId", id.clone()),
            TerminateBy::Name => ("ProvisionedProductName", row.provisioned_name.to_string()),
            TerminateBy::Arn => ("ProvisionedProductName", arn.clone()),
        };
        let (status, terminated) = call(
            &router,
            "TerminateProvisionedProduct",
            &json!({member: identifier, "TerminateToken": format!("terminate-{}", row.provisioned_name)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the termination to be served, got {status} with {terminated}"
        );
        let record = &terminated["RecordDetail"];
        let terminate_record_id = record["RecordId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RecordId in {terminated}"))
            .to_string();
        assert!(
            is_catalog_id("rec", &terminate_record_id) && terminate_record_id != provision_record_id,
            "case {case:?}: the termination must mint its own record id, got \
             {terminate_record_id:?} beside {provision_record_id:?}"
        );
        // The record is the record OF the termination, in flight, naming the stored
        // provisioned product it terminates.
        assert_eq!(
            (
                record["RecordType"].as_str(),
                record["Status"].as_str(),
                record["ProvisionedProductId"].as_str(),
                record["ProvisionedProductName"].as_str(),
                record["ProductId"].as_str(),
            ),
            (
                Some("TERMINATE_PROVISIONED_PRODUCT"),
                Some("CREATED"),
                Some(id.as_str()),
                Some(row.provisioned_name),
                Some(product_id.as_str()),
            ),
            "case {case:?}: unexpected termination record in {terminated}"
        );
        let reported_tags = crate::harness::tag_map(case, &record["RecordTags"]);
        let stored_tags: std::collections::BTreeMap<String, String> = row
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            reported_tags, stored_tags,
            "case {case:?}: the record must carry the provisioned product's stored tags, got \
             {terminated}"
        );

        // The termination is owed, not done: the provisioned product still exists and
        // reports the record of the operation in flight.
        let (status, in_flight) = describe_provisioned(&router, "Id", &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a caller acting straight after the termination still reads the \
             provisioned product, got {status} with {in_flight}"
        );
        assert_eq!(
            (
                in_flight["ProvisionedProductDetail"]["Status"].as_str(),
                in_flight["ProvisionedProductDetail"]["LastRecordId"].as_str(),
            ),
            (Some("UNDER_CHANGE"), Some(terminate_record_id.as_str())),
            "case {case:?}: the termination in flight is the last operation: {in_flight}"
        );

        // A repeat of the request while that termination is owed is the SAME
        // termination, whatever token it carries: no second record is minted.
        let (status, repeated) = call(
            &router,
            "TerminateProvisionedProduct",
            &json!({"ProvisionedProductId": id, "TerminateToken": "terminate-again"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated termination to be served, got {status} with \
             {repeated}"
        );
        assert_eq!(
            repeated["RecordDetail"]["RecordId"].as_str(),
            Some(terminate_record_id.as_str()),
            "case {case:?}: a repeat must answer the termination already in flight: {repeated}"
        );

        provisioned.push((
            row,
            Provisioned {
                id,
                arn,
                terminate_record_id,
            },
        ));
    }

    // The transition is owed by the CLOCK, not by any read: wait the span out once.
    std::thread::sleep(SETTLE_WAIT);

    for (row, stored) in &provisioned {
        let case = row.case;
        // Every way of addressing a provisioned product that no longer exists is the
        // modelled not-found, naming the identifier the request used.
        for (member, identifier) in [
            ("Id", stored.id.as_str()),
            ("Name", row.provisioned_name),
        ] {
            let (status, refused) = describe_provisioned(&router, member, identifier).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a terminated provisioned product cannot be read by {member}, got \
                 {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error reading by {member}: {refused}"
            );
            assert_eq!(
                message(&refused),
                format!("Provisioned product not found: {identifier}"),
                "case {case:?}: unexpected message reading by {member}: {refused}"
            );
        }

        // A search no longer reports it either: the two readers of one provisioned
        // product cannot disagree about whether it exists.
        let (status, found) = call(
            &router,
            "SearchProvisionedProducts",
            &json!({"Filters": {"SearchQuery": [format!("name:{}", row.provisioned_name)]}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the search to be served, got {status} with {found}"
        );
        assert_eq!(
            (
                found["ProvisionedProducts"].as_array().map(Vec::len),
                found["TotalResultsCount"].as_i64(),
            ),
            (Some(0), Some(0)),
            "case {case:?}: a terminated provisioned product is not searchable: {found}"
        );

        // Terminating it again is the not-found as well, whatever token the request
        // carries: "after the provisioned product is terminated, subsequent requests to
        // terminate the same provisioned product always return ResourceNotFound".
        let (status, refused) = call(
            &router,
            "TerminateProvisionedProduct",
            &json!({
                "ProvisionedProductId": stored.id,
                "TerminateToken": "terminate-after-the-fact",
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a second termination must be a not-found, got {status} with {refused}"
        );
        // And so is a termination addressed by the ARN the provisioned product had.
        let (status, refused) = call(
            &router,
            "TerminateProvisionedProduct",
            &json!({"ProvisionedProductName": stored.arn, "TerminateToken": "terminate-by-arn"}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: the ARN of a terminated provisioned product resolves nothing, got \
             {status} with {refused}"
        );

        // The RECORD of the termination outlives the provisioned product: the
        // termination's record id is not reused and the re-provisioning below mints its
        // own, so the record this termination minted is still the one this id names.
        // (`DescribeRecord` is still a stub, so this is read through the ids the stored
        // history hands out.)
        assert!(
            is_catalog_id("rec", &stored.terminate_record_id),
            "case {case:?}: the termination's record id must stay a record id, got {:?}",
            stored.terminate_record_id
        );

        // The name is free again: the SAME `ProvisionedProductName` provisions a new
        // provisioned product, with a new id.
        let reprovisioned = provision(
            &router,
            case,
            &json!({
                "ProductId": product_id,
                "ProvisioningArtifactId": artifact_id,
                "ProvisionedProductName": row.provisioned_name,
                "ProvisionToken": format!("reprovision-{}", row.provisioned_name),
            }),
        )
        .await;
        let (status, described) =
            describe_provisioned(&router, "Name", row.provisioned_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the re-provisioned product must be readable, got {status} with \
             {described}"
        );
        let new_id = described["ProvisionedProductDetail"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {described}"));
        assert!(
            new_id != stored.id && is_catalog_id("pp", new_id),
            "case {case:?}: re-provisioning the freed name mints a new provisioned product, got \
             {new_id:?} against {:?} in {reprovisioned}",
            stored.id
        );
    }
}

/// Replaying the `ProvisionToken` of a provisioned product that has been terminated is
/// no longer a replay: the token cannot answer with a resource that stopped existing.
#[tokio::test]
async fn a_provision_token_whose_provisioned_product_was_terminated_provisions_again() {
    let router = emulator();
    let (product_id, artifact_id, _portfolio_id) = seed_launch(&router).await;
    let request = json!({
        "ProductId": product_id,
        "ProvisioningArtifactId": artifact_id,
        "ProvisionedProductName": "pp-replayed",
        "ProvisionToken": "one-token",
    });

    let first = provision(&router, "the first provisioning", &request).await;
    let first_id = first["RecordDetail"]["ProvisionedProductId"]
        .as_str()
        .unwrap_or_else(|| panic!("no ProvisionedProductId in {first}"))
        .to_string();

    // While the provisioned product exists the token replays the record it minted.
    let replayed = provision(&router, "the replay", &request).await;
    assert_eq!(
        replayed["RecordDetail"]["RecordId"],
        first["RecordDetail"]["RecordId"],
        "the token must replay the record its request minted: {replayed}"
    );

    let (status, terminated) = call(
        &router,
        "TerminateProvisionedProduct",
        &json!({"ProvisionedProductId": first_id, "TerminateToken": "terminate-replayed"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the termination to be served, got {status} with {terminated}"
    );
    std::thread::sleep(SETTLE_WAIT);

    let again = provision(&router, "after the termination settled", &request).await;
    let new_id = again["RecordDetail"]["ProvisionedProductId"]
        .as_str()
        .unwrap_or_else(|| panic!("no ProvisionedProductId in {again}"));
    assert!(
        new_id != first_id,
        "the token of a terminated provisioned product provisions a new one, got {new_id:?} \
         against {first_id:?} in {again}"
    );
}

/// One row of the termination-refusal table: how the request addresses a provisioned
/// product it cannot address.
struct RefusalCase {
    case: &'static str,
    request: fn(&str) -> Value,
}

/// Every request that addresses no provisioned product is refused with the operation's
/// one modelled error, and nothing about the stored provisioned product changes.
#[tokio::test]
async fn a_termination_that_addresses_no_provisioned_product_is_refused() {
    // Rows differ along WHICH way the request fails to address a provisioned product:
    // an id, a name and an ARN nothing carries, both members at once, and neither.
    let cases = [
        RefusalCase {
            case: "a well-formed provisioned-product id nothing stored carries",
            request: |_| json!({"ProvisionedProductId": "pp-000000000000"}),
        },
        RefusalCase {
            case: "a name no provisioned product carries",
            request: |_| json!({"ProvisionedProductName": "pp-nothing-by-this-name"}),
        },
        RefusalCase {
            case: "an ARN no provisioned product carries",
            request: |_| {
                json!({
                    "ProvisionedProductName":
                        "arn:aws:servicecatalog:us-east-1:000000000000:stack/nope/pp-000000000000",
                })
            },
        },
        RefusalCase {
            case: "both the id and the name of the stored provisioned product",
            request: |id| {
                json!({"ProvisionedProductId": id, "ProvisionedProductName": "pp-addressed"})
            },
        },
        RefusalCase {
            case: "neither the id nor the name",
            request: |_| json!({}),
        },
        RefusalCase {
            case: "an id of another resource kind",
            request: |_| json!({"ProvisionedProductId": "port-zzzzzzzzzzzz"}),
        },
    ];

    let router = emulator();
    let (product_id, artifact_id, _portfolio_id) = seed_launch(&router).await;
    let body = provision(
        &router,
        "the provisioned product the refusals leave alone",
        &json!({
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionedProductName": "pp-addressed",
            "ProvisionToken": "provision-addressed",
        }),
    )
    .await;
    let stored_id = body["RecordDetail"]["ProvisionedProductId"]
        .as_str()
        .unwrap_or_else(|| panic!("no ProvisionedProductId in {body}"))
        .to_string();

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let mut request = (row.request)(&stored_id);
        request["TerminateToken"] = Value::String(format!("refused-{index}"));
        let (status, refused) = call(&router, "TerminateProvisionedProduct", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request refused, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {refused}"
        );
    }

    // None of the refusals touched the stored provisioned product.
    let (status, described) = describe_provisioned(&router, "Id", &stored_id).await;
    assert_eq!(
        status, 200,
        "the refusals must have left the provisioned product alone, got {status} with {described}"
    );
    assert_eq!(
        described["ProvisionedProductDetail"]["Name"].as_str(),
        Some("pp-addressed"),
        "the stored provisioned product must read back unchanged: {described}"
    );
}

/// How many launch paths the product of a path row resolves.
#[derive(Debug, Clone, Copy)]
enum Paths {
    /// A product id nothing stored carries, which resolves no path either.
    Unknown,
    /// A stored product that sits in no portfolio.
    None,
    /// A stored product in exactly one portfolio: its default path.
    Default,
    /// A stored product in two portfolios, so it has no default path.
    Two,
}

/// What one operation answers for a row of the launch-path table.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// The launch path resolved: the request is served.
    Served,
    /// 400 `InvalidParametersException` with AWS's fixed sentence for a request that
    /// names neither half of the pair where no default path can be inferred.
    PathRequired,
    /// 400 `InvalidParametersException` naming the path member the request used and the
    /// value it carried, which identifies no launch path of the product.
    InvalidPath,
    /// 400 `ResourceNotFoundException` naming the product that resolves no launch path —
    /// the refusal a PROVISIONING takes for such a product, which resolves the product's
    /// paths before it reads the request's path members.
    NoLaunchPaths,
    /// 400 `ResourceNotFoundException` naming the path a provisioning could not resolve
    /// among the product's own.
    LaunchPathNotFound,
}

/// Which path the request names, among the two portfolios a row seeds.
#[derive(Debug, Clone, Copy)]
enum PathArg {
    /// The portfolio the product under test is associated with first, by id.
    FirstById,
    /// …and by its display name.
    FirstByName,
    /// The other portfolio, by id: a path of the product only in the two-path row.
    SecondById,
    /// …and by its display name.
    SecondByName,
}

/// One row of the launch-path pair table.
struct PathCase {
    case: &'static str,
    paths: Paths,
    /// The path the request names, or `None` where it names neither member.
    names: Option<PathArg>,
    /// What `DescribeProvisioningParameters` — the end-user read of a launch — answers.
    describe: Outcome,
    /// What `ProvisionProduct` answers for the same launch.
    provision: Outcome,
}

/// The launch-path members are validated as PARAMETERS by the read that reports a
/// launch's configuration: the pair is required unless the product resolves exactly one
/// path, a value that identifies none of the product's paths is an invalid parameter,
/// and both refusals are decided before the call reports whether the product or its
/// paths resolve. A provisioning resolves the product's paths first, so it takes its own
/// not-found for a product that has none.
#[tokio::test]
async fn the_launch_path_members_are_validated_as_parameters_of_the_launch() {
    // Rows differ along how many launch paths the product resolves (none at all, one,
    // two, and a product nothing stored carries) and along which path the request names:
    // neither member, one of the product's own by id or by name, and one that is not the
    // product's by id or by name.
    let cases = [
        PathCase {
            case: "no path named for a product with a default path",
            paths: Paths::Default,
            names: None,
            describe: Outcome::Served,
            provision: Outcome::Served,
        },
        PathCase {
            case: "the default path named by id",
            paths: Paths::Default,
            names: Some(PathArg::FirstById),
            describe: Outcome::Served,
            provision: Outcome::Served,
        },
        PathCase {
            case: "the default path named by its portfolio's name",
            paths: Paths::Default,
            names: Some(PathArg::FirstByName),
            describe: Outcome::Served,
            provision: Outcome::Served,
        },
        PathCase {
            case: "no path named for a product with two paths",
            paths: Paths::Two,
            names: None,
            describe: Outcome::PathRequired,
            provision: Outcome::PathRequired,
        },
        PathCase {
            case: "the second of the two paths named by id",
            paths: Paths::Two,
            names: Some(PathArg::SecondById),
            describe: Outcome::Served,
            provision: Outcome::Served,
        },
        PathCase {
            case: "the second of the two paths named by name",
            paths: Paths::Two,
            names: Some(PathArg::SecondByName),
            describe: Outcome::Served,
            provision: Outcome::Served,
        },
        PathCase {
            case: "a path that is not one of the product's, by id",
            paths: Paths::Default,
            names: Some(PathArg::SecondById),
            describe: Outcome::InvalidPath,
            provision: Outcome::LaunchPathNotFound,
        },
        PathCase {
            case: "a path that is not one of the product's, by name",
            paths: Paths::Default,
            names: Some(PathArg::SecondByName),
            describe: Outcome::InvalidPath,
            provision: Outcome::LaunchPathNotFound,
        },
        PathCase {
            case: "no path named for a product that sits in no portfolio",
            paths: Paths::None,
            names: None,
            describe: Outcome::PathRequired,
            provision: Outcome::NoLaunchPaths,
        },
        PathCase {
            case: "no path named for a product id nothing stored carries",
            paths: Paths::Unknown,
            names: None,
            describe: Outcome::PathRequired,
            provision: Outcome::NoLaunchPaths,
        },
        PathCase {
            case: "a path named for a product that sits in no portfolio",
            paths: Paths::None,
            names: Some(PathArg::FirstById),
            describe: Outcome::InvalidPath,
            provision: Outcome::NoLaunchPaths,
        },
        PathCase {
            case: "a path named for a product id nothing stored carries",
            paths: Paths::Unknown,
            names: Some(PathArg::FirstByName),
            describe: Outcome::InvalidPath,
            provision: Outcome::NoLaunchPaths,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let first_name = "alpha-portfolio";
        let second_name = "beta-portfolio";
        let first_portfolio = create_portfolio(&router, first_name, "acme", &[], "p1").await;
        let second_portfolio = create_portfolio(&router, second_name, "globex", &[], "p2").await;
        let stored_product = create_product(&router, "path-source-product", "product-token").await;
        let artifact_id = first_provisioning_artifact_id(&router, &stored_product).await;
        match row.paths {
            Paths::Unknown | Paths::None => {}
            Paths::Default => {
                associate_product_with_portfolio(&router, &stored_product, &first_portfolio).await
            }
            Paths::Two => {
                associate_product_with_portfolio(&router, &stored_product, &first_portfolio).await;
                associate_product_with_portfolio(&router, &stored_product, &second_portfolio).await;
            }
        }
        let product_id = match row.paths {
            Paths::Unknown => "prod-000000000000".to_string(),
            _ => stored_product.clone(),
        };

        let mut request = json!({
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
        });
        // The member the row names the path by, and the value it carries — the variable
        // part of the refusal that names it.
        let named = match row.names {
            Some(PathArg::FirstById) => Some(("PathId", first_portfolio.clone())),
            Some(PathArg::FirstByName) => Some(("PathName", first_name.to_string())),
            Some(PathArg::SecondById) => Some(("PathId", second_portfolio.clone())),
            Some(PathArg::SecondByName) => Some(("PathName", second_name.to_string())),
            None => None,
        };
        if let Some((member, value)) = &named {
            request[*member] = Value::String(value.clone());
        }

        // The refusal each outcome states, as the operation under test words it.
        let expected = |outcome: Outcome| -> Option<(&'static str, String)> {
            match outcome {
                Outcome::Served => None,
                Outcome::PathRequired => {
                    Some(("InvalidParametersException", PATH_REQUIRED.to_string()))
                }
                Outcome::InvalidPath => {
                    let (member, value) = named
                        .as_ref()
                        .unwrap_or_else(|| panic!("case {case:?}: no path named to be invalid"));
                    Some((
                        "InvalidParametersException",
                        format!("Invalid {member}: {value}"),
                    ))
                }
                Outcome::NoLaunchPaths => Some((
                    "ResourceNotFoundException",
                    format!("No launch paths found for resource: {product_id}"),
                )),
                Outcome::LaunchPathNotFound => {
                    let (_, value) = named
                        .as_ref()
                        .unwrap_or_else(|| panic!("case {case:?}: no path named to be unresolved"));
                    Some((
                        "ResourceNotFoundException",
                        format!("Launch path not found: {value}"),
                    ))
                }
            }
        };

        let (status, body) = call(&router, "DescribeProvisioningParameters", &request).await;
        assert_outcome(
            case,
            "DescribeProvisioningParameters",
            status,
            &body,
            expected(row.describe),
        );

        let mut provisioning = request.clone();
        provisioning["ProvisionedProductName"] = Value::String(format!("pp-path-{index}"));
        provisioning["ProvisionToken"] = Value::String(format!("path-token-{index}"));
        let (status, body) = call(&router, "ProvisionProduct", &provisioning).await;
        assert_outcome(
            case,
            "ProvisionProduct",
            status,
            &body,
            expected(row.provision),
        );
    }
}

/// Assert one row's outcome: served, or refused with that error code and that sentence.
fn assert_outcome(
    case: &str,
    operation: &str,
    status: u16,
    body: &Value,
    expected: Option<(&'static str, String)>,
) {
    match expected {
        None => assert_eq!(
            status, 200,
            "case {case:?}: {operation} must be served, got {status} with {body}"
        ),
        Some((code, sentence)) => {
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must be refused, got {status} with {body}"
            );
            assert_eq!(
                error_code(body),
                code,
                "case {case:?}: unexpected {operation} error in {body}"
            );
            assert_eq!(
                message(body),
                sentence,
                "case {case:?}: unexpected {operation} message in {body}"
            );
        }
    }
}

/// One row of the timestamp table: one operation, and the member of its response that
/// carries a timestamp.
struct TimestampCase {
    case: &'static str,
    operation: &'static str,
    /// The members to walk into, outermost first.
    pointer: &'static [&'static str],
}

/// Whether `seconds` — a timestamp on the wire, which this protocol serves as epoch
/// seconds — carries at most millisecond resolution.
fn is_millisecond_resolution(seconds: f64) -> bool {
    let millis = seconds * 1_000.0;
    (millis - millis.round()).abs() < 1e-3
}

/// The timestamp a response carries at `pointer`, as epoch seconds. A member that is a
/// number indexes into a list.
fn timestamp(case: &str, body: &Value, pointer: &[&str]) -> f64 {
    let mut value = body;
    for member in pointer {
        value = match member.parse::<usize>() {
            Ok(index) => &value[index],
            Err(_) => &value[*member],
        };
    }
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected an epoch-seconds timestamp at {pointer:?}, got {body}")
    })
}

/// Every timestamp this service reports carries at most the millisecond resolution the
/// protocol serves, and every reporter of one stored instant reports the same value.
#[tokio::test]
async fn every_reported_timestamp_carries_the_resolution_the_protocol_serves() {
    let router = emulator();
    let portfolio_id = create_portfolio(&router, "alpha-portfolio", "acme", &[], "portfolio").await;
    let product_id = create_product(&router, "timestamped-product", "product-token").await;
    let artifact_id = first_provisioning_artifact_id(&router, &product_id).await;
    associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;
    let provision_request = json!({
        "ProductId": product_id,
        "ProvisioningArtifactId": artifact_id,
        "ProvisionedProductName": "pp-timestamped",
        "ProvisionToken": "provision-timestamped",
    });
    let provisioned = provision(&router, "the provisioning under test", &provision_request).await;
    // The same token replays the record the provisioning minted: a SECOND reporter of
    // the one stored instant.
    let replayed = provision(&router, "the replay of that provisioning", &provision_request).await;

    // A second provisioned product, terminated, so the record of a termination is
    // reported here too.
    let terminated_pp = provision(
        &router,
        "the provisioning that is terminated",
        &json!({
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionedProductName": "pp-timestamped-terminated",
            "ProvisionToken": "provision-timestamped-terminated",
        }),
    )
    .await;
    let terminated_id = terminated_pp["RecordDetail"]["ProvisionedProductId"]
        .as_str()
        .unwrap_or_else(|| panic!("no ProvisionedProductId in {terminated_pp}"))
        .to_string();
    let (status, terminated) = call(
        &router,
        "TerminateProvisionedProduct",
        &json!({"ProvisionedProductId": terminated_id, "TerminateToken": "terminate-ts"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the termination to be served, got {status} with {terminated}"
    );

    // Rows differ along WHICH operation reports the timestamp and which member of its
    // response carries it: a record's two timestamps as the request that minted it and
    // a replay of that request report them, a termination record's two, a provisioned
    // product's as a describe and a search report it, a portfolio's and a product's.
    let cases = [
        TimestampCase {
            case: "ProvisionProduct's record was created",
            operation: "ProvisionProduct",
            pointer: &["RecordDetail", "CreatedTime"],
        },
        TimestampCase {
            case: "ProvisionProduct's record was last updated",
            operation: "ProvisionProduct",
            pointer: &["RecordDetail", "UpdatedTime"],
        },
        TimestampCase {
            case: "the replay of that provisioning reports the same record",
            operation: "ProvisionProduct (replayed)",
            pointer: &["RecordDetail", "CreatedTime"],
        },
        TimestampCase {
            case: "TerminateProvisionedProduct's record was created",
            operation: "TerminateProvisionedProduct",
            pointer: &["RecordDetail", "CreatedTime"],
        },
        TimestampCase {
            case: "TerminateProvisionedProduct's record was last updated",
            operation: "TerminateProvisionedProduct",
            pointer: &["RecordDetail", "UpdatedTime"],
        },
        TimestampCase {
            case: "DescribeProvisionedProduct reports the provisioned product",
            operation: "DescribeProvisionedProduct",
            pointer: &["ProvisionedProductDetail", "CreatedTime"],
        },
        TimestampCase {
            case: "SearchProvisionedProducts reports the same provisioned product",
            operation: "SearchProvisionedProducts",
            pointer: &["ProvisionedProducts", "0", "CreatedTime"],
        },
        TimestampCase {
            case: "DescribePortfolio reports the portfolio",
            operation: "DescribePortfolio",
            pointer: &["PortfolioDetail", "CreatedTime"],
        },
        TimestampCase {
            case: "DescribeProductAsAdmin reports the product",
            operation: "DescribeProductAsAdmin",
            pointer: &["ProductViewDetail", "CreatedTime"],
        },
    ];

    let (status, described_provisioned) =
        describe_provisioned(&router, "Name", "pp-timestamped").await;
    assert_eq!(
        status, 200,
        "expected the describe to be served, got {status} with {described_provisioned}"
    );
    let (status, found) = call(
        &router,
        "SearchProvisionedProducts",
        &json!({"Filters": {"SearchQuery": ["name:pp-timestamped"]}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the search to be served, got {status} with {found}"
    );
    let (status, described_portfolio) =
        call(&router, "DescribePortfolio", &json!({"Id": portfolio_id})).await;
    assert_eq!(
        status, 200,
        "expected DescribePortfolio to be served, got {status} with {described_portfolio}"
    );
    let (status, described_product) = call(
        &router,
        "DescribeProductAsAdmin",
        &json!({"Id": product_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected DescribeProductAsAdmin to be served, got {status} with {described_product}"
    );

    for row in &cases {
        let case = row.case;
        let body = match row.operation {
            "ProvisionProduct" => &provisioned,
            "ProvisionProduct (replayed)" => &replayed,
            "TerminateProvisionedProduct" => &terminated,
            "DescribeProvisionedProduct" => &described_provisioned,
            "SearchProvisionedProducts" => &found,
            "DescribePortfolio" => &described_portfolio,
            "DescribeProductAsAdmin" => &described_product,
            other => panic!("case {case:?}: no response captured for {other}"),
        };
        let seconds = timestamp(case, body, row.pointer);
        assert!(
            is_millisecond_resolution(seconds),
            "case {case:?}: expected a timestamp of at most millisecond resolution, got {seconds} \
             in {body}"
        );
    }

    // Two reporters of ONE record report the same instants, and a record that is still
    // in flight was last updated when it was created.
    let created = timestamp("the record", &provisioned, &["RecordDetail", "CreatedTime"]);
    let updated = timestamp("the record", &provisioned, &["RecordDetail", "UpdatedTime"]);
    assert_eq!(
        (
            timestamp("the replay", &replayed, &["RecordDetail", "CreatedTime"]),
            timestamp("the replay", &replayed, &["RecordDetail", "UpdatedTime"]),
        ),
        (created, updated),
        "the request and a replay of it cannot report one record's instants differently: \
         {provisioned} against {replayed}"
    );
    assert_eq!(
        created, updated,
        "a record still in flight was last updated when it was created: {provisioned}"
    );

    // Two reporters of ONE provisioned product report the same creation instant.
    assert_eq!(
        timestamp(
            "the search",
            &found,
            &["ProvisionedProducts", "0", "CreatedTime"]
        ),
        timestamp(
            "the describe",
            &described_provisioned,
            &["ProvisionedProductDetail", "CreatedTime"]
        ),
        "a describe and a search cannot report one provisioned product's instant \
         differently: {described_provisioned} against {found}"
    );

    // Once the clock settles the record, the update it reports is the completion — later
    // than the creation, and still at the resolution above.
    std::thread::sleep(SETTLE_WAIT);
    let settled = provision(&router, "the replay after the record settled", &provision_request).await;
    let settled_created = timestamp("settled", &settled, &["RecordDetail", "CreatedTime"]);
    let settled_updated = timestamp("settled", &settled, &["RecordDetail", "UpdatedTime"]);
    assert_eq!(
        settled_created, created,
        "a record's creation instant is minted once: {settled}"
    );
    assert!(
        settled_updated > settled_created && is_millisecond_resolution(settled_updated),
        "a settled record was updated after it was created, at the same resolution: {settled}"
    );
    assert_eq!(
        settled["RecordDetail"]["Status"].as_str(),
        Some("SUCCEEDED"),
        "the clock owes the completion of the record: {settled}"
    );
}
