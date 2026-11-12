// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListResolverQueryLogConfigs`.
//!
//! The rule: the operation reports the query logging configurations the account
//! has created, computed from the store and projected exactly as
//! `GetResolverQueryLogConfig` projects one; it narrows that collection to the
//! records whose named member matches one of a filter's values, orders it by the
//! element `SortBy` names in the direction `SortOrder` asks for, cuts it into
//! pages a cursor resumes, and reports the size of the whole collection
//! (`TotalCount`) and of the selected one (`TotalFilteredCount`) beside the page.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use super::harness::{
    call, emulator, expect_error, ok, parse_time, string_member, without_status, Emulator, ACCOUNT,
};

/// One seeded query logging configuration: what was asked for, and the `Id` the
/// create minted for it.
struct Seeded {
    name: &'static str,
    id: String,
}

/// The configurations the suite seeds, in creation order.
///
/// They span what a query selects and orders on: every destination kind
/// `CreateResolverQueryLogConfig$DestinationArn` documents (an S3 bucket, a
/// CloudWatch Logs log group — named with the `:*` wildcard, so the stored
/// destination is the group — and a Kinesis Data Firehose delivery stream under
/// both the `kinesis` and the `firehose` service), names whose alphabetical
/// order is NOT the creation order, and association counts of none, one, two and
/// three.
const SEEDS: [(&str, &str, usize); 5] = [
    (
        "echo-logs",
        "arn:aws:logs:us-west-2:123456789012:log-group:/echo:*",
        2,
    ),
    ("charlie-s3", "arn:aws:s3:::amzn-s3-demo-bucket-charlie", 0),
    (
        "alpha-kinesis",
        "arn:aws:kinesis:us-east-2:012345678901:stream/alpha",
        1,
    ),
    (
        "delta-firehose",
        "arn:aws:firehose:eu-west-1:012345678901:deliverystream/delta",
        3,
    ),
    ("bravo-s3", "arn:aws:s3:::amzn-s3-demo-bucket-bravo", 0),
];

/// Create every configuration of [`SEEDS`], associate each with as many VPCs as
/// its row asks for, and answer them in creation order.
async fn seed_configs(router: &Emulator) -> Vec<Seeded> {
    let mut seeded = Vec::new();
    for (index, (name, destination, associations)) in SEEDS.iter().enumerate() {
        // Each configuration delivers to a destination the account holds, which
        // the create resolves against the owning service's store.
        router.hold_query_log_destinations([destination]);
        let created = ok(
            router,
            "CreateResolverQueryLogConfig",
            json!({
                "CreatorRequestId": format!("req-listing-{index}"),
                "Name": name,
                "DestinationArn": destination,
            }),
        )
        .await;
        let id = string_member(
            created
                .get("ResolverQueryLogConfig")
                .unwrap_or_else(|| panic!("the create answers a configuration, got {created}")),
            "Id",
        );
        for vpc in 0..*associations {
            let resource_id = format!("vpc-0000000000000{index}{vpc}0");
            // Each association is made with a VPC of the account's own, which
            // the associate resolves against the Amazon EC2 store.
            router.hold_vpcs([&resource_id]);
            ok(
                router,
                "AssociateResolverQueryLogConfig",
                json!({
                    "ResolverQueryLogConfigId": id.clone(),
                    "ResourceId": resource_id,
                }),
            )
            .await;
        }
        seeded.push(Seeded { name, id });
    }
    seeded
}

/// The configurations one response carries.
fn listed(body: &Value) -> Vec<Value> {
    body.get("ResolverQueryLogConfigs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id`s one response carries, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|config| string_member(config, "Id"))
        .collect()
}

/// A count member of one response (`TotalCount`, `TotalFilteredCount`), which
/// the model marks required and defaults to zero.
fn count(body: &Value, member: &str) -> i64 {
    body.get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("the response carries a {member}, got {body}"))
}

/// The id of the seeded configuration named `name`.
fn id_of(seeded: &[Seeded], name: &str) -> String {
    seeded
        .iter()
        .find(|config| config.name == name)
        .map(|config| config.id.clone())
        .unwrap_or_else(|| panic!("{name} is one of the seeded configurations"))
}

/// The value a listed configuration presents for the member a query sorts by, as
/// something orderable: a count and a timestamp compare as NUMBERS (so 10 comes
/// after 9, and an earlier instant before a later one whatever digits either
/// timestamp was rendered with), every other sortable member as text.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ordered {
    Number(i128),
    Text(String),
}

fn ordered_value(case: &str, config: &Value, member: &str) -> Ordered {
    match member {
        "AssociationCount" => Ordered::Number(i128::from(count(config, member))),
        "CreationTime" => {
            let at = parse_time(case, member, &string_member(config, member));
            Ordered::Number(i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos()))
        }
        _ => Ordered::Text(string_member(config, member)),
    }
}

/// Every stored configuration is listed in creation order, projected as its own
/// reader projects it, with the two counts the model requires.
#[tokio::test]
async fn configs_are_listed_from_state_and_projected_as_the_reader_projects_them() {
    let router = emulator();
    let seeded = seed_configs(&router).await;
    let expected: Vec<String> = seeded.iter().map(|config| config.id.clone()).collect();

    let page = ok(&router, "ListResolverQueryLogConfigs", json!({})).await;
    assert_eq!(
        listed_ids(&page),
        expected,
        "case \"no filters\": every stored configuration must be listed in creation order, got \
         {page}"
    );
    assert_eq!(
        (
            count(&page, "TotalCount"),
            count(&page, "TotalFilteredCount")
        ),
        (
            i64::try_from(expected.len()).unwrap_or_default(),
            i64::try_from(expected.len()).unwrap_or_default()
        ),
        "case \"no filters\": both counts must be the size of the store, got {page}"
    );

    // Each entry is the same projection the single-record reader answers. The
    // status is compared apart, because it is the member the clock is allowed to
    // move between the two calls.
    for (index, id) in expected.iter().enumerate() {
        let fetched = ok(
            &router,
            "GetResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": id.clone() }),
        )
        .await;
        let from_list = listed(&page)
            .get(index)
            .cloned()
            .unwrap_or_else(|| panic!("the page carries {id}, got {page}"));
        assert_eq!(
            without_status(&from_list),
            without_status(
                fetched
                    .get("ResolverQueryLogConfig")
                    .unwrap_or(&Value::Null)
            ),
            "case {id:?}: the listed configuration must be the stored one, got {from_list}"
        );
        let status = string_member(&from_list, "Status");
        assert!(
            status == "CREATING" || status == "CREATED",
            "case {id:?}: a listed configuration reports a status of its own slice, got {status:?}"
        );
    }

    // An account that has created nothing is reported as an empty page with both
    // counts at zero — never as an absent list.
    let empty = ok(&emulator(), "ListResolverQueryLogConfigs", json!({})).await;
    assert_eq!(
        (
            listed(&empty).len(),
            count(&empty, "TotalCount"),
            count(&empty, "TotalFilteredCount")
        ),
        (0, 0, 0),
        "case \"an empty store\": the page must be an empty list with zero counts, got {empty}"
    );
    assert!(
        empty.get("ResolverQueryLogConfigs").is_some(),
        "case \"an empty store\": the page must still carry the list member, got {empty}"
    );
}

/// A filter restricts the page to the records whose named member matches one of
/// `Values`, and `TotalFilteredCount` counts that selection while `TotalCount`
/// keeps reporting the whole store.
///
/// The filter names are exactly the catalog the model documents for this
/// operation (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverQueryLogConfigs**): `Arn`, `AssociationCount`, `CreationTime`,
/// `CreatorRequestId`, `Destination`, `DestinationArn`, `Id`, `Name`, `OwnerId`,
/// `ShareStatus`, `Status` — each exercised here, in the documented spelling and
/// in the early upper-snake one the same doc says is "still supported".
#[tokio::test]
async fn configs_are_filtered_on_every_documented_member() {
    let router = emulator();
    let seeded = seed_configs(&router).await;
    let all: Vec<String> = seeded.iter().map(|config| config.id.clone()).collect();
    let store_size = i64::try_from(all.len()).unwrap_or_default();

    let unfiltered = ok(&router, "ListResolverQueryLogConfigs", json!({})).await;
    let first = listed(&unfiltered)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the page carries the first configuration, got {unfiltered}"));
    let first_arn = string_member(&first, "Arn");
    let first_created = string_member(&first, "CreationTime");
    let status = string_member(&first, "Status");
    // Timestamps are rendered to a fixed resolution, so two configurations
    // created in the same instant share one: the expected rows for a
    // `CreationTime` filter are read off the unfiltered page rather than assumed.
    let created_together: Vec<String> = listed(&unfiltered)
        .iter()
        .filter(|config| string_member(config, "CreationTime") == first_created)
        .map(|config| string_member(config, "Id"))
        .collect();
    // The Unix-time spelling `Filter$Name` documents for `CreationTime` names a
    // whole second, so it selects every configuration created inside that
    // second — read off the unfiltered page for the same reason as the set
    // above.
    let unix_second = |stamp: &str| -> String {
        parse_time("CreationTime", "CreationTime", stamp)
            .secs()
            .to_string()
    };
    let first_created_unix = unix_second(&first_created);
    let created_in_the_same_second: Vec<String> = listed(&unfiltered)
        .iter()
        .filter(|config| unix_second(&string_member(config, "CreationTime")) == first_created_unix)
        .map(|config| string_member(config, "Id"))
        .collect();

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "Id",
            json!([{ "Name": "Id", "Values": [all[2].clone()] }]),
            vec![all[2].clone()],
        ),
        (
            "ID, the early spelling, two ids",
            json!([{ "Name": "ID", "Values": [all[0].clone(), all[4].clone()] }]),
            vec![all[0].clone(), all[4].clone()],
        ),
        (
            "Arn",
            json!([{ "Name": "Arn", "Values": [first_arn.clone()] }]),
            vec![all[0].clone()],
        ),
        (
            "Arn, one nothing minted",
            json!([{ "Name": "ARN", "Values": [format!("{first_arn}-not-mine")] }]),
            vec![],
        ),
        (
            "Name",
            json!([{ "Name": "Name", "Values": ["alpha-kinesis"] }]),
            vec![id_of(&seeded, "alpha-kinesis")],
        ),
        (
            "NAME, the early spelling",
            json!([{ "Name": "NAME", "Values": ["bravo-s3", "charlie-s3"] }]),
            vec![
                id_of(&seeded, "charlie-s3"),
                id_of(&seeded, "bravo-s3"),
            ],
        ),
        (
            "CreatorRequestId",
            json!([{ "Name": "CreatorRequestId", "Values": ["req-listing-3"] }]),
            vec![all[3].clone()],
        ),
        (
            "CREATOR_REQUEST_ID, the early spelling",
            json!([{ "Name": "CREATOR_REQUEST_ID", "Values": ["req-listing-0"] }]),
            vec![all[0].clone()],
        ),
        (
            "AssociationCount, the configurations with no VPC",
            json!([{ "Name": "AssociationCount", "Values": ["0"] }]),
            vec![
                id_of(&seeded, "charlie-s3"),
                id_of(&seeded, "bravo-s3"),
            ],
        ),
        (
            "ASSOCIATION_COUNT, the early spelling, two counts",
            json!([{ "Name": "ASSOCIATION_COUNT", "Values": ["1", "3"] }]),
            vec![
                id_of(&seeded, "alpha-kinesis"),
                id_of(&seeded, "delta-firehose"),
            ],
        ),
        (
            "AssociationCount, a count none reports",
            json!([{ "Name": "AssociationCount", "Values": ["7"] }]),
            vec![],
        ),
        (
            "Destination, S3",
            json!([{ "Name": "Destination", "Values": ["S3"] }]),
            vec![
                id_of(&seeded, "charlie-s3"),
                id_of(&seeded, "bravo-s3"),
            ],
        ),
        (
            "Destination, CloudWatchLogs",
            json!([{ "Name": "Destination", "Values": ["CloudWatchLogs"] }]),
            vec![id_of(&seeded, "echo-logs")],
        ),
        (
            "Destination, KinesisFirehose, both services that name one",
            json!([{ "Name": "DESTINATION", "Values": ["KinesisFirehose"] }]),
            vec![
                id_of(&seeded, "alpha-kinesis"),
                id_of(&seeded, "delta-firehose"),
            ],
        ),
        (
            "Destination, a kind that is no documented value",
            json!([{ "Name": "Destination", "Values": ["DynamoDB"] }]),
            vec![],
        ),
        (
            "DestinationArn, the canonical log group the create stored",
            json!([{
                "Name": "DestinationArn",
                "Values": ["arn:aws:logs:us-west-2:123456789012:log-group:/echo"],
            }]),
            vec![id_of(&seeded, "echo-logs")],
        ),
        (
            "DestinationArn, the wildcard form the request carried",
            json!([{
                "Name": "DESTINATION_ARN",
                "Values": ["arn:aws:logs:us-west-2:123456789012:log-group:/echo:*"],
            }]),
            vec![],
        ),
        (
            "OwnerId, the calling account",
            json!([{ "Name": "OwnerId", "Values": [ACCOUNT] }]),
            all.clone(),
        ),
        (
            "OWNER_ID, another account",
            json!([{ "Name": "OWNER_ID", "Values": ["123456789012"] }]),
            vec![],
        ),
        (
            "ShareStatus, shared with nobody",
            json!([{ "Name": "ShareStatus", "Values": ["NOT_SHARED"] }]),
            all.clone(),
        ),
        (
            "SHARE_STATUS, shared by this account",
            json!([{ "Name": "SHARE_STATUS", "Values": ["SHARED_BY_ME"] }]),
            vec![],
        ),
        (
            "Status, the status the records report",
            json!([{ "Name": "Status", "Values": [status.clone(), "CREATED"] }]),
            all.clone(),
        ),
        (
            "Status, one none reports",
            json!([{ "Name": "STATUS", "Values": ["FAILED"] }]),
            vec![],
        ),
        (
            "CreationTime",
            json!([{ "Name": "CreationTime", "Values": [first_created.clone()] }]),
            created_together.clone(),
        ),
        (
            "CreationTime, the documented Unix-time spelling",
            json!([{ "Name": "CreationTime", "Values": [first_created_unix.clone()] }]),
            created_in_the_same_second.clone(),
        ),
        (
            "CREATION_TIME, an instant no configuration was created at",
            json!([{ "Name": "CREATION_TIME", "Values": ["1999-12-31T23:59:59Z"] }]),
            vec![],
        ),
        (
            "CreationTime, a Unix second no configuration was created in",
            json!([{ "Name": "CreationTime", "Values": ["946684799"] }]),
            vec![],
        ),
        (
            "two filters combine with AND, both matching",
            json!([
                { "Name": "Destination", "Values": ["S3"] },
                { "Name": "AssociationCount", "Values": ["0"] },
            ]),
            vec![
                id_of(&seeded, "charlie-s3"),
                id_of(&seeded, "bravo-s3"),
            ],
        ),
        (
            "two filters combine with AND, disagreeing",
            json!([
                { "Name": "Destination", "Values": ["S3"] },
                { "Name": "AssociationCount", "Values": ["3"] },
            ]),
            vec![],
        ),
        (
            "an empty value list matches nothing",
            json!([{ "Name": "Name", "Values": [] }]),
            vec![],
        ),
        ("an empty filter list restricts nothing", json!([]), all.clone()),
    ];

    for (case, filters, expected) in cases {
        let page = ok(
            &router,
            "ListResolverQueryLogConfigs",
            json!({ "Filters": filters }),
        )
        .await;
        assert_eq!(
            listed_ids(&page),
            expected,
            "case {case:?}: the filtered page is wrong, got {page}"
        );
        assert_eq!(
            count(&page, "TotalFilteredCount"),
            i64::try_from(expected.len()).unwrap_or_default(),
            "case {case:?}: TotalFilteredCount must count the selection, got {page}"
        );
        assert_eq!(
            count(&page, "TotalCount"),
            store_size,
            "case {case:?}: TotalCount must stay the size of the store, got {page}"
        );
    }

    // A name outside THIS operation's catalog is refused, not ignored —
    // including the ones that are valid for the endpoint, rule and rule
    // association lists, and the association list's own `ResourceId`.
    for case in [
        "Direction",
        "HostVPCId",
        "IpAddressCount",
        "SecurityGroupIds",
        "DomainName",
        "Type",
        "ResolverEndpointId",
        "ResolverRuleId",
        "VPCId",
        "ResourceId",
        "Error",
    ] {
        expect_error(
            &router,
            "ListResolverQueryLogConfigs",
            json!({ "Filters": [{ "Name": case, "Values": ["anything"] }] }),
            case,
            "InvalidParameterException",
        )
        .await;
    }
}

/// `SortBy` orders the page by the element it names, in the direction
/// `SortOrder` asks for, and reports the same set of configurations whichever
/// element and direction are chosen.
///
/// The elements are exactly the catalog that operation's `SortBy` documents
/// (`smithy_query(query="doc",shape="ListResolverQueryLogConfigsRequest",member="SortBy")`):
/// `Arn`, `AssociationCount`, `CreationTime`, `CreatorRequestId`,
/// `DestinationArn`, `Id`, `Name`, `OwnerId`, `ShareStatus`, `Status`.
#[tokio::test]
async fn sorting_orders_the_page_by_the_element_sortby_names() {
    let router = emulator();
    let seeded = seed_configs(&router).await;
    let mut all: Vec<String> = seeded.iter().map(|config| config.id.clone()).collect();
    all.sort();

    for member in [
        "Arn",
        "AssociationCount",
        "CreationTime",
        "CreatorRequestId",
        "DestinationArn",
        "Id",
        "Name",
        "OwnerId",
        "ShareStatus",
        "Status",
    ] {
        for (order, ascending) in [
            (None, true),
            (Some("ASCENDING"), true),
            (Some("DESCENDING"), false),
        ] {
            let case = format!("SortBy {member} {}", order.unwrap_or("with no SortOrder"));
            let mut request = json!({ "SortBy": member });
            if let Some(order) = order {
                request["SortOrder"] = json!(order);
            }
            let page = ok(&router, "ListResolverQueryLogConfigs", request).await;

            let values: Vec<Ordered> = listed(&page)
                .iter()
                .map(|config| ordered_value(&case, config, member))
                .collect();
            let ordered = values.windows(2).all(|pair| {
                if ascending {
                    pair[0] <= pair[1]
                } else {
                    pair[0] >= pair[1]
                }
            });
            assert!(
                ordered,
                "case {case:?}: the page must be ordered by {member}, got {values:?}"
            );

            let mut returned = listed_ids(&page);
            returned.sort();
            assert_eq!(
                returned, all,
                "case {case:?}: sorting must report the same configurations, got {page}"
            );
            assert_eq!(
                count(&page, "TotalCount"),
                i64::try_from(all.len()).unwrap_or_default(),
                "case {case:?}: TotalCount must stay the size of the store, got {page}"
            );
        }
    }

    // Ordering by a member with a known order is checked against the names the
    // seeds carry, so a sort that only reported SOME order would not pass.
    let by_name = ok(
        &router,
        "ListResolverQueryLogConfigs",
        json!({ "SortBy": "Name", "SortOrder": "ASCENDING" }),
    )
    .await;
    assert_eq!(
        listed(&by_name)
            .iter()
            .map(|config| string_member(config, "Name"))
            .collect::<Vec<String>>(),
        vec![
            "alpha-kinesis",
            "bravo-s3",
            "charlie-s3",
            "delta-firehose",
            "echo-logs"
        ],
        "case \"SortBy Name ASCENDING\": the names must come back in alphabetical order, got \
         {by_name}"
    );
    let by_count = ok(
        &router,
        "ListResolverQueryLogConfigs",
        json!({ "SortBy": "AssociationCount", "SortOrder": "DESCENDING" }),
    )
    .await;
    assert_eq!(
        listed(&by_count)
            .iter()
            .map(|config| count(config, "AssociationCount"))
            .collect::<Vec<i64>>(),
        vec![3, 2, 1, 0, 0],
        "case \"SortBy AssociationCount DESCENDING\": the counts must fall, got {by_count}"
    );

    // Sorting is a pure read: the unsorted query still reports creation order.
    let again = ok(&router, "ListResolverQueryLogConfigs", json!({})).await;
    assert_eq!(
        listed_ids(&again),
        seeded
            .iter()
            .map(|config| config.id.clone())
            .collect::<Vec<String>>(),
        "case \"after sorting\": an unsorted query must still report creation order, got {again}"
    );

    // An element outside the documented catalog is refused, not ignored — among
    // them `Destination`, which this operation can be FILTERED on but not
    // sorted by, and a sort order outside the modelled enum.
    for (case, request) in [
        (
            "SortBy Destination, a filter-only member",
            json!({ "SortBy": "Destination" }),
        ),
        (
            "SortBy a member of no catalog",
            json!({ "SortBy": "DomainName" }),
        ),
        (
            "SortBy the list member itself",
            json!({ "SortBy": "ResolverQueryLogConfigs" }),
        ),
    ] {
        expect_error(
            &router,
            "ListResolverQueryLogConfigs",
            request,
            case,
            "InvalidParameterException",
        )
        .await;
    }

    // `SortOrder` is a modelled enum and `Filter$Name` is length 1..64, so a
    // value outside either is refused by the boundary before the handler runs —
    // with HTTP 400, never with a page ordered some other way.
    for (case, request) in [
        (
            "SortOrder outside the modelled enum",
            json!({ "SortBy": "Name", "SortOrder": "ascending" }),
        ),
        (
            "SortOrder of a sort nothing asked for",
            json!({ "SortOrder": "REVERSED" }),
        ),
        (
            "an empty filter name",
            json!({ "Filters": [{ "Name": "", "Values": ["anything"] }] }),
        ),
        (
            "an empty SortBy",
            json!({ "SortBy": "" }),
        ),
    ] {
        let (status, body) = call(&router, "ListResolverQueryLogConfigs", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside the model must be refused, got {status}: {body}"
        );
    }
}

/// The operation pages the collection the query selects: `MaxResults` bounds the
/// page, `NextToken` resumes where the previous page stopped, the last page
/// carries none, the counts describe the whole collection rather than the page,
/// and a cursor only means anything for the query that minted it.
#[tokio::test]
async fn paging_walks_every_config_exactly_once() {
    let router = emulator();
    let seeded = seed_configs(&router).await;
    let expected: Vec<String> = seeded.iter().map(|config| config.id.clone()).collect();
    let store_size = i64::try_from(expected.len()).unwrap_or_default();

    for (case, page_size) in [
        ("one configuration per page", 1),
        ("two configurations per page", 2),
        ("a page larger than the collection", 100),
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "ListResolverQueryLogConfigs", request).await;
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case:?}: a page must hold at most MaxResults configurations, got {page}"
            );
            assert_eq!(
                (
                    count(&page, "TotalCount"),
                    count(&page, "TotalFilteredCount")
                ),
                (store_size, store_size),
                "case {case:?}: the counts must describe the collection, not the page, got {page}"
            );
            walked.extend(ids);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: paging must walk every configuration exactly once, in creation order"
        );
    }

    // Paging a SORTED query walks that query's order exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": 2, "SortBy": "Name", "SortOrder": "DESCENDING" });
        if let Some(token) = token.as_ref() {
            request["NextToken"] = json!(token);
        }
        let page = ok(&router, "ListResolverQueryLogConfigs", request).await;
        walked.extend(
            listed(&page)
                .iter()
                .map(|config| string_member(config, "Name")),
        );
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked,
        vec![
            "echo-logs",
            "delta-firehose",
            "charlie-s3",
            "bravo-s3",
            "alpha-kinesis"
        ],
        "case \"paging a sorted query\": the pages must walk the sorted order once"
    );

    // A cursor is scoped to the query that minted it: the collection it indexes
    // is the filtered, ordered one, so resuming with a different `Filters`,
    // `SortBy` or `SortOrder` is refused rather than cut at the wrong position.
    let first = ok(
        &router,
        "ListResolverQueryLogConfigs",
        json!({
            "MaxResults": 1,
            "Filters": [{ "Name": "Destination", "Values": ["S3"] }],
            "SortBy": "Name",
            "SortOrder": "ASCENDING",
        }),
    )
    .await;
    let token = string_member(&first, "NextToken");
    assert!(
        !token.is_empty(),
        "the filtered query must owe a cursor, got {first}"
    );
    for (case, request) in [
        (
            "a cursor minted under a filter, replayed with none",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "SortBy": "Name",
                "SortOrder": "ASCENDING",
            }),
        ),
        (
            "a cursor minted under a filter, replayed under another",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{ "Name": "Destination", "Values": ["CloudWatchLogs"] }],
                "SortBy": "Name",
                "SortOrder": "ASCENDING",
            }),
        ),
        (
            "a cursor minted under one SortBy, replayed under another",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{ "Name": "Destination", "Values": ["S3"] }],
                "SortBy": "Id",
                "SortOrder": "ASCENDING",
            }),
        ),
        (
            "a cursor minted ascending, replayed descending",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{ "Name": "Destination", "Values": ["S3"] }],
                "SortBy": "Name",
                "SortOrder": "DESCENDING",
            }),
        ),
        (
            "a cursor this service never minted",
            json!({ "NextToken": "6e6f742d612d746f6b656e" }),
        ),
    ] {
        expect_error(
            &router,
            "ListResolverQueryLogConfigs",
            request,
            case,
            "InvalidNextTokenException",
        )
        .await;
    }

    // The same cursor under the query that minted it resumes that query.
    let resumed = ok(
        &router,
        "ListResolverQueryLogConfigs",
        json!({
            "MaxResults": 1,
            "NextToken": token,
            "Filters": [{ "Name": "Destination", "Values": ["S3"] }],
            "SortBy": "Name",
            "SortOrder": "ASCENDING",
        }),
    )
    .await;
    assert_eq!(
        listed_ids(&resumed),
        vec![id_of(&seeded, "charlie-s3")],
        "the cursor must resume the query that minted it, got {resumed}"
    );
}

/// A list is a pure read that reports the store as it stands: two lists of an
/// unchanged store agree, and a configuration created afterwards appears in the
/// next one with the counts moved with it.
#[tokio::test]
async fn listing_reports_the_store_and_changes_nothing() {
    let router = emulator();
    let seeded = seed_configs(&router).await;
    let expected: Vec<String> = seeded.iter().map(|config| config.id.clone()).collect();

    let before = ok(&router, "ListResolverQueryLogConfigs", json!({})).await;
    let again = ok(&router, "ListResolverQueryLogConfigs", json!({})).await;
    assert_eq!(
        listed_ids(&before),
        listed_ids(&again),
        "two lists of an unchanged store must agree, got {again}"
    );
    assert_eq!(
        listed_ids(&again),
        expected,
        "a list must not disturb the records it reports, got {again}"
    );

    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket-appended"]);
    let created = ok(
        &router,
        "CreateResolverQueryLogConfig",
        json!({
            "CreatorRequestId": "req-listing-appended",
            "Name": "appended",
            "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket-appended",
        }),
    )
    .await;
    let appended = string_member(
        created
            .get("ResolverQueryLogConfig")
            .unwrap_or_else(|| panic!("the create answers a configuration, got {created}")),
        "Id",
    );
    let after = ok(&router, "ListResolverQueryLogConfigs", json!({})).await;
    let mut with_appended = expected.clone();
    with_appended.push(appended);
    assert_eq!(
        listed_ids(&after),
        with_appended,
        "a configuration created after the first page must be appended, got {after}"
    );
    assert_eq!(
        count(&after, "TotalCount"),
        i64::try_from(with_appended.len()).unwrap_or_default(),
        "TotalCount must follow the store, got {after}"
    );
}
