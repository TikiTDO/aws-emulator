// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListResolverQueryLogConfigAssociations`.
//!
//! The rule: the operation reports the VPC-to-configuration associations the
//! account has made, computed from the store and projected exactly as
//! `GetResolverQueryLogConfigAssociation` projects one; it narrows that
//! collection to the records whose named member matches one of a filter's
//! values, orders it by the element `SortBy` names in the direction `SortOrder`
//! asks for, cuts it into pages a cursor resumes, and reports the size of the
//! whole collection (`TotalCount`) and of the selected one
//! (`TotalFilteredCount`) beside the page.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};

use super::harness::{
    call, emulator, expect_error, ok, parse_time, string_member, without_status, Emulator,
};

/// One seeded association: the configuration and the VPC it joins, and the `Id`
/// the associate minted for it.
struct Seeded {
    config_id: String,
    resource_id: String,
    id: String,
}

/// The configurations the suite seeds, each with the VPCs it is associated with,
/// in the order the associates are made.
///
/// They span what a query over the ASSOCIATIONS selects and orders on: three
/// configurations, so an association is selected by a
/// `ResolverQueryLogConfigId` that names some of them; a configuration with no
/// VPC at all, so a configuration contributes nothing to the collection; and VPC
/// ids whose alphabetical order is NOT the order the associates were made in.
const SEEDS: [(&str, &str, &[&str]); 3] = [
    (
        "assoc-listing-logs",
        "arn:aws:logs:us-west-2:123456789012:log-group:/assoc-listing:*",
        &["vpc-000000000000000e0", "vpc-000000000000000c0"],
    ),
    (
        "assoc-listing-empty",
        "arn:aws:s3:::amzn-s3-demo-bucket-assoc-listing",
        &[],
    ),
    (
        "assoc-listing-kinesis",
        "arn:aws:kinesis:us-east-2:012345678901:stream/assoc-listing",
        &[
            "vpc-000000000000000a0",
            "vpc-000000000000000d0",
            "vpc-000000000000000b0",
        ],
    ),
];

/// Create every configuration of [`SEEDS`], associate each with the VPCs its row
/// names, and answer the associations in the order they were made.
async fn seed_associations(router: &Emulator) -> Vec<Seeded> {
    let mut seeded = Vec::new();
    for (index, (name, destination, vpcs)) in SEEDS.iter().enumerate() {
        // Each configuration delivers to a destination the account holds, which
        // the create resolves against the owning service's store.
        router.hold_query_log_destinations([destination]);
        let created = ok(
            router,
            "CreateResolverQueryLogConfig",
            json!({
                "CreatorRequestId": format!("req-assoc-listing-{index}"),
                "Name": name,
                "DestinationArn": destination,
            }),
        )
        .await;
        let config_id = string_member(
            created
                .get("ResolverQueryLogConfig")
                .unwrap_or_else(|| panic!("the create answers a configuration, got {created}")),
            "Id",
        );
        for resource_id in *vpcs {
            // Each association is made with a VPC of the account's own, which
            // the associate resolves against the Amazon EC2 store.
            router.hold_vpcs([resource_id]);
            let associated = ok(
                router,
                "AssociateResolverQueryLogConfig",
                json!({
                    "ResolverQueryLogConfigId": config_id.clone(),
                    "ResourceId": resource_id,
                }),
            )
            .await;
            let association = associated
                .get("ResolverQueryLogConfigAssociation")
                .unwrap_or_else(|| {
                    panic!("the associate answers an association, got {associated}")
                });
            seeded.push(Seeded {
                config_id: config_id.clone(),
                resource_id: (*resource_id).to_string(),
                id: string_member(association, "Id"),
            });
        }
    }
    seeded
}

/// The associations one response carries.
fn listed(body: &Value) -> Vec<Value> {
    body.get("ResolverQueryLogConfigAssociations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id`s one response carries, in the order it reported them.
fn listed_ids(body: &Value) -> Vec<String> {
    listed(body)
        .iter()
        .map(|association| string_member(association, "Id"))
        .collect()
}

/// A count member of one response (`TotalCount`, `TotalFilteredCount`), which
/// the model marks required and defaults to zero.
fn count(body: &Value, member: &str) -> i64 {
    body.get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("the response carries a {member}, got {body}"))
}

/// The id of the association seeded for the VPC `resource_id`.
fn id_of(seeded: &[Seeded], resource_id: &str) -> String {
    seeded
        .iter()
        .find(|association| association.resource_id == resource_id)
        .map(|association| association.id.clone())
        .unwrap_or_else(|| panic!("{resource_id} is one of the seeded VPCs"))
}

/// The value a listed association presents for the member a query sorts by, as
/// something orderable: a timestamp compares as a NUMBER (so an earlier instant
/// comes before a later one whatever digits either timestamp was rendered with),
/// every other sortable member as text — and a member no association reports
/// (`Error`) as the empty text, so every association ties on it.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Ordered {
    Number(i128),
    Text(String),
}

fn ordered_value(case: &str, association: &Value, member: &str) -> Ordered {
    match member {
        "CreationTime" => {
            let at = parse_time(case, member, &string_member(association, member));
            Ordered::Number(i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos()))
        }
        _ => Ordered::Text(string_member(association, member)),
    }
}

/// Every stored association is listed in association order, projected as its own
/// reader projects it, with the two counts the model requires.
#[tokio::test]
async fn associations_are_listed_from_state_and_projected_as_the_reader_projects_them() {
    let router = emulator();
    let seeded = seed_associations(&router).await;
    let expected: Vec<String> = seeded
        .iter()
        .map(|association| association.id.clone())
        .collect();

    let page = ok(&router, "ListResolverQueryLogConfigAssociations", json!({})).await;
    assert_eq!(
        listed_ids(&page),
        expected,
        "case \"no filters\": every stored association must be listed in association order, got \
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
        "case \"no filters\": both counts must be the size of the collection, got {page}"
    );

    // Each entry is the same projection the single-record reader answers. The
    // status is compared apart, because it is the member the clock is allowed to
    // move between the two calls.
    for (index, id) in expected.iter().enumerate() {
        let fetched = ok(
            &router,
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
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
                    .get("ResolverQueryLogConfigAssociation")
                    .unwrap_or(&Value::Null)
            ),
            "case {id:?}: the listed association must be the stored one, got {from_list}"
        );
        let status = string_member(&from_list, "Status");
        assert!(
            status == "CREATING" || status == "ACTIVE",
            "case {id:?}: a listed association reports a status of its own slice, got {status:?}"
        );
        // "If the value of `Status` is a value other than `FAILED`, `Error` is
        // null": this slice leaves no failed association, so neither member is
        // reported.
        assert!(
            from_list.get("Error").is_none() && from_list.get("ErrorMessage").is_none(),
            "case {id:?}: an association that has not failed reports no Error, got {from_list}"
        );
    }

    // An account that has associated nothing is reported as an empty page with
    // both counts at zero — never as an absent list. A configuration with no VPC
    // contributes nothing, so creating one changes neither.
    let bare = emulator();
    bare.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket-bare"]);
    ok(
        &bare,
        "CreateResolverQueryLogConfig",
        json!({
            "CreatorRequestId": "req-assoc-listing-bare",
            "Name": "assoc-listing-bare",
            "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket-bare",
        }),
    )
    .await;
    let empty = ok(&bare, "ListResolverQueryLogConfigAssociations", json!({})).await;
    assert_eq!(
        (
            listed(&empty).len(),
            count(&empty, "TotalCount"),
            count(&empty, "TotalFilteredCount")
        ),
        (0, 0, 0),
        "case \"a configuration with no VPC\": the page must be an empty list with zero counts, \
         got {empty}"
    );
    assert!(
        empty
            .get("ResolverQueryLogConfigAssociations")
            .is_some(),
        "case \"a configuration with no VPC\": the page must still carry the list member, got \
         {empty}"
    );
}

/// A filter restricts the page to the records whose named member matches one of
/// `Values`, and `TotalFilteredCount` counts that selection while `TotalCount`
/// keeps reporting the whole collection.
///
/// The filter names are exactly the catalog the model documents for this
/// operation (`smithy_query(query="doc",shape="Filter",member="Name")`, section
/// **ListResolverQueryLogConfigAssociations**): `CreationTime`, `Error`, `Id`,
/// `ResolverQueryLogConfigId`, `ResourceId`, `Status` — each exercised here, in
/// the documented spelling and in the early upper-snake one the same doc says is
/// "still supported".
#[tokio::test]
async fn associations_are_filtered_on_every_documented_member() {
    let router = emulator();
    let seeded = seed_associations(&router).await;
    let all: Vec<String> = seeded
        .iter()
        .map(|association| association.id.clone())
        .collect();
    let collection_size = i64::try_from(all.len()).unwrap_or_default();
    let first_config = seeded
        .first()
        .map(|association| association.config_id.clone())
        .unwrap_or_default();
    let last_config = seeded
        .iter()
        .next_back()
        .map(|association| association.config_id.clone())
        .unwrap_or_default();

    let unfiltered = ok(&router, "ListResolverQueryLogConfigAssociations", json!({})).await;
    let first = listed(&unfiltered)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the page carries the first association, got {unfiltered}"));
    let first_created = string_member(&first, "CreationTime");
    // Timestamps are rendered to a fixed resolution, so two associations made in
    // the same instant share one: the expected rows for a `CreationTime` filter
    // are read off the unfiltered page rather than assumed.
    let created_together: Vec<String> = listed(&unfiltered)
        .iter()
        .filter(|association| string_member(association, "CreationTime") == first_created)
        .map(|association| string_member(association, "Id"))
        .collect();
    // The Unix-time spelling `Filter$Name` documents for `CreationTime` names a
    // whole second, so it selects every association made inside that second —
    // read off the unfiltered page for the same reason as the set above.
    let unix_second = |stamp: &str| -> String {
        parse_time("CreationTime", "CreationTime", stamp)
            .secs()
            .to_string()
    };
    let first_created_unix = unix_second(&first_created);
    let created_in_the_same_second: Vec<String> = listed(&unfiltered)
        .iter()
        .filter(|association| {
            unix_second(&string_member(association, "CreationTime")) == first_created_unix
        })
        .map(|association| string_member(association, "Id"))
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
            "Id, one nothing minted",
            json!([{ "Name": "Id", "Values": ["rqlca-0000000000000000"] }]),
            vec![],
        ),
        (
            "ResourceId",
            json!([{ "Name": "ResourceId", "Values": ["vpc-000000000000000a0"] }]),
            vec![id_of(&seeded, "vpc-000000000000000a0")],
        ),
        (
            "RESOURCE_ID, the early spelling, two VPCs",
            json!([{
                "Name": "RESOURCE_ID",
                "Values": ["vpc-000000000000000b0", "vpc-000000000000000e0"],
            }]),
            vec![
                id_of(&seeded, "vpc-000000000000000e0"),
                id_of(&seeded, "vpc-000000000000000b0"),
            ],
        ),
        (
            "ResourceId, a VPC nothing is associated with",
            json!([{ "Name": "ResourceId", "Values": ["vpc-000000000000000f0"] }]),
            vec![],
        ),
        (
            "ResolverQueryLogConfigId, the first configuration",
            json!([{ "Name": "ResolverQueryLogConfigId", "Values": [first_config.clone()] }]),
            vec![
                id_of(&seeded, "vpc-000000000000000e0"),
                id_of(&seeded, "vpc-000000000000000c0"),
            ],
        ),
        (
            "RESOLVER_QUERY_LOG_CONFIG_ID, the early spelling, both configurations",
            json!([{
                "Name": "RESOLVER_QUERY_LOG_CONFIG_ID",
                "Values": [first_config.clone(), last_config.clone()],
            }]),
            all.clone(),
        ),
        (
            "ResolverQueryLogConfigId, a configuration nothing minted",
            json!([{ "Name": "ResolverQueryLogConfigId", "Values": ["rqlc-0000000000000000"] }]),
            vec![],
        ),
        (
            "Status, the statuses the records report",
            json!([{ "Name": "Status", "Values": ["CREATING", "ACTIVE"] }]),
            all.clone(),
        ),
        (
            "STATUS, one none reports",
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
            "CREATION_TIME, an instant no association was made at",
            json!([{ "Name": "CREATION_TIME", "Values": ["1999-12-31T23:59:59Z"] }]),
            vec![],
        ),
        (
            "CreationTime, a Unix second no association was made in",
            json!([{ "Name": "CreationTime", "Values": ["946684799"] }]),
            vec![],
        ),
        (
            // "If Status is a value other than FAILED, Error is null": no
            // association of this slice has failed, so none reports an Error and
            // a filter on that member selects none of them.
            "Error, DESTINATION_NOT_FOUND",
            json!([{ "Name": "Error", "Values": ["DESTINATION_NOT_FOUND"] }]),
            vec![],
        ),
        (
            "ERROR, the early spelling, ACCESS_DENIED",
            json!([{ "Name": "ERROR", "Values": ["ACCESS_DENIED"] }]),
            vec![],
        ),
        (
            "two filters combine with AND, both matching",
            json!([
                { "Name": "ResolverQueryLogConfigId", "Values": [last_config.clone()] },
                { "Name": "ResourceId", "Values": ["vpc-000000000000000d0"] },
            ]),
            vec![id_of(&seeded, "vpc-000000000000000d0")],
        ),
        (
            "two filters combine with AND, disagreeing",
            json!([
                { "Name": "ResolverQueryLogConfigId", "Values": [first_config.clone()] },
                { "Name": "ResourceId", "Values": ["vpc-000000000000000d0"] },
            ]),
            vec![],
        ),
        (
            "an empty value list matches nothing",
            json!([{ "Name": "ResourceId", "Values": [] }]),
            vec![],
        ),
        (
            "an empty filter list restricts nothing",
            json!([]),
            all.clone(),
        ),
    ];

    for (case, filters, expected) in cases {
        let page = ok(
            &router,
            "ListResolverQueryLogConfigAssociations",
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
            collection_size,
            "case {case:?}: TotalCount must stay the size of the collection, got {page}"
        );
    }

    // A name outside THIS operation's catalog is refused, not ignored —
    // including the ones that are valid for the endpoint, rule, rule association
    // and query-logging-configuration lists.
    for case in [
        "Arn",
        "AssociationCount",
        "CreatorRequestId",
        "Destination",
        "DestinationArn",
        "Name",
        "OwnerId",
        "ShareStatus",
        "Direction",
        "HostVPCId",
        "VPCId",
        "ResolverRuleId",
    ] {
        expect_error(
            &router,
            "ListResolverQueryLogConfigAssociations",
            json!({ "Filters": [{ "Name": case, "Values": ["anything"] }] }),
            case,
            "InvalidParameterException",
        )
        .await;
    }
}

/// `SortBy` orders the page by the element it names, in the direction
/// `SortOrder` asks for, and reports the same set of associations whichever
/// element and direction are chosen.
///
/// The elements are exactly the catalog that operation's `SortBy` documents
/// (`smithy_query(query="doc",shape="ListResolverQueryLogConfigAssociationsRequest",member="SortBy")`):
/// `CreationTime`, `Error`, `Id`, `ResolverQueryLogConfigId`, `ResourceId`,
/// `Status`.
#[tokio::test]
async fn sorting_orders_the_page_by_the_element_sortby_names() {
    let router = emulator();
    let seeded = seed_associations(&router).await;
    let mut all: Vec<String> = seeded
        .iter()
        .map(|association| association.id.clone())
        .collect();
    all.sort();

    for member in [
        "CreationTime",
        "Error",
        "Id",
        "ResolverQueryLogConfigId",
        "ResourceId",
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
            let page = ok(&router, "ListResolverQueryLogConfigAssociations", request).await;

            let values: Vec<Ordered> = listed(&page)
                .iter()
                .map(|association| ordered_value(&case, association, member))
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
                "case {case:?}: sorting must report the same associations, got {page}"
            );
            assert_eq!(
                count(&page, "TotalCount"),
                i64::try_from(all.len()).unwrap_or_default(),
                "case {case:?}: TotalCount must stay the size of the collection, got {page}"
            );
        }
    }

    // Ordering by a member with a known order is checked against the VPC ids the
    // seeds carry, so a sort that only reported SOME order would not pass.
    let by_resource = ok(
        &router,
        "ListResolverQueryLogConfigAssociations",
        json!({ "SortBy": "ResourceId", "SortOrder": "ASCENDING" }),
    )
    .await;
    assert_eq!(
        listed(&by_resource)
            .iter()
            .map(|association| string_member(association, "ResourceId"))
            .collect::<Vec<String>>(),
        vec![
            "vpc-000000000000000a0",
            "vpc-000000000000000b0",
            "vpc-000000000000000c0",
            "vpc-000000000000000d0",
            "vpc-000000000000000e0",
        ],
        "case \"SortBy ResourceId ASCENDING\": the VPC ids must come back in alphabetical order, \
         got {by_resource}"
    );
    let by_resource_desc = ok(
        &router,
        "ListResolverQueryLogConfigAssociations",
        json!({ "SortBy": "ResourceId", "SortOrder": "DESCENDING" }),
    )
    .await;
    assert_eq!(
        listed(&by_resource_desc)
            .iter()
            .map(|association| string_member(association, "ResourceId"))
            .collect::<Vec<String>>(),
        vec![
            "vpc-000000000000000e0",
            "vpc-000000000000000d0",
            "vpc-000000000000000c0",
            "vpc-000000000000000b0",
            "vpc-000000000000000a0",
        ],
        "case \"SortBy ResourceId DESCENDING\": the VPC ids must come back reversed, got \
         {by_resource_desc}"
    );

    // `Error` is null on every association this slice stores, so every one ties
    // on it and the stable sort leaves them in association order — in either
    // direction.
    let association_order: Vec<String> = seeded
        .iter()
        .map(|association| association.id.clone())
        .collect();
    for order in ["ASCENDING", "DESCENDING"] {
        let by_error = ok(
            &router,
            "ListResolverQueryLogConfigAssociations",
            json!({ "SortBy": "Error", "SortOrder": order }),
        )
        .await;
        assert_eq!(
            listed_ids(&by_error),
            association_order,
            "case \"SortBy Error {order}\": associations that tie must keep association order, \
             got {by_error}"
        );
    }

    // Sorting is a pure read: the unsorted query still reports association
    // order.
    let again = ok(&router, "ListResolverQueryLogConfigAssociations", json!({})).await;
    assert_eq!(
        listed_ids(&again),
        association_order,
        "case \"after sorting\": an unsorted query must still report association order, got \
         {again}"
    );

    // An element outside the documented catalog is refused, not ignored — among
    // them the members the CONFIGURATION list can be sorted by but an
    // association cannot.
    for (case, request) in [
        (
            "SortBy Name, a member of the configuration catalog",
            json!({ "SortBy": "Name" }),
        ),
        (
            "SortBy AssociationCount, a member of the configuration catalog",
            json!({ "SortBy": "AssociationCount" }),
        ),
        (
            "SortBy a member of no catalog",
            json!({ "SortBy": "DomainName" }),
        ),
        (
            "SortBy the list member itself",
            json!({ "SortBy": "ResolverQueryLogConfigAssociations" }),
        ),
    ] {
        expect_error(
            &router,
            "ListResolverQueryLogConfigAssociations",
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
            json!({ "SortBy": "Id", "SortOrder": "ascending" }),
        ),
        (
            "SortOrder of a sort nothing asked for",
            json!({ "SortOrder": "REVERSED" }),
        ),
        (
            "an empty filter name",
            json!({ "Filters": [{ "Name": "", "Values": ["anything"] }] }),
        ),
        ("an empty SortBy", json!({ "SortBy": "" })),
    ] {
        let (status, body) =
            call(&router, "ListResolverQueryLogConfigAssociations", request).await;
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
async fn paging_walks_every_association_exactly_once() {
    let router = emulator();
    let seeded = seed_associations(&router).await;
    let expected: Vec<String> = seeded
        .iter()
        .map(|association| association.id.clone())
        .collect();
    let collection_size = i64::try_from(expected.len()).unwrap_or_default();
    let last_config = seeded
        .iter()
        .next_back()
        .map(|association| association.config_id.clone())
        .unwrap_or_default();

    for (case, page_size) in [
        ("one association per page", 1),
        ("two associations per page", 2),
        ("a page larger than the collection", 100),
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = token.as_ref() {
                request["NextToken"] = json!(token);
            }
            let page = ok(&router, "ListResolverQueryLogConfigAssociations", request).await;
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case:?}: a page must hold at most MaxResults associations, got {page}"
            );
            assert_eq!(
                (
                    count(&page, "TotalCount"),
                    count(&page, "TotalFilteredCount")
                ),
                (collection_size, collection_size),
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
            "case {case:?}: paging must walk every association exactly once, in association order"
        );
    }

    // Paging a SORTED query walks that query's order exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request =
            json!({ "MaxResults": 2, "SortBy": "ResourceId", "SortOrder": "DESCENDING" });
        if let Some(token) = token.as_ref() {
            request["NextToken"] = json!(token);
        }
        let page = ok(&router, "ListResolverQueryLogConfigAssociations", request).await;
        walked.extend(
            listed(&page)
                .iter()
                .map(|association| string_member(association, "ResourceId")),
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
            "vpc-000000000000000e0",
            "vpc-000000000000000d0",
            "vpc-000000000000000c0",
            "vpc-000000000000000b0",
            "vpc-000000000000000a0",
        ],
        "case \"paging a sorted query\": the pages must walk the sorted order once"
    );

    // A cursor is scoped to the query that minted it: the collection it indexes
    // is the filtered, ordered one, so resuming with a different `Filters`,
    // `SortBy` or `SortOrder` is refused rather than cut at the wrong position.
    let first = ok(
        &router,
        "ListResolverQueryLogConfigAssociations",
        json!({
            "MaxResults": 1,
            "Filters": [{ "Name": "ResolverQueryLogConfigId", "Values": [last_config.clone()] }],
            "SortBy": "ResourceId",
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
                "SortBy": "ResourceId",
                "SortOrder": "ASCENDING",
            }),
        ),
        (
            "a cursor minted under a filter, replayed under another",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{ "Name": "ResourceId", "Values": ["vpc-000000000000000a0"] }],
                "SortBy": "ResourceId",
                "SortOrder": "ASCENDING",
            }),
        ),
        (
            "a cursor minted under one SortBy, replayed under another",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{
                    "Name": "ResolverQueryLogConfigId",
                    "Values": [last_config.clone()],
                }],
                "SortBy": "Id",
                "SortOrder": "ASCENDING",
            }),
        ),
        (
            "a cursor minted ascending, replayed descending",
            json!({
                "MaxResults": 1,
                "NextToken": token.clone(),
                "Filters": [{
                    "Name": "ResolverQueryLogConfigId",
                    "Values": [last_config.clone()],
                }],
                "SortBy": "ResourceId",
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
            "ListResolverQueryLogConfigAssociations",
            request,
            case,
            "InvalidParameterException",
        )
        .await;
    }

    // The same cursor under the query that minted it resumes that query.
    let resumed = ok(
        &router,
        "ListResolverQueryLogConfigAssociations",
        json!({
            "MaxResults": 1,
            "NextToken": token,
            "Filters": [{ "Name": "ResolverQueryLogConfigId", "Values": [last_config] }],
            "SortBy": "ResourceId",
            "SortOrder": "ASCENDING",
        }),
    )
    .await;
    assert_eq!(
        listed(&resumed)
            .iter()
            .map(|association| string_member(association, "ResourceId"))
            .collect::<Vec<String>>(),
        vec!["vpc-000000000000000b0"],
        "the cursor must resume the query that minted it, got {resumed}"
    );
}

/// A list is a pure read that reports the store as it stands: two lists of an
/// unchanged store agree, an association made afterwards appears in the next one
/// with the counts moved with it, and one taken out of its configuration is gone
/// from it.
#[tokio::test]
async fn listing_reports_the_store_and_changes_nothing() {
    let router = emulator();
    let seeded = seed_associations(&router).await;
    let expected: Vec<String> = seeded
        .iter()
        .map(|association| association.id.clone())
        .collect();
    let first_config = seeded
        .first()
        .map(|association| association.config_id.clone())
        .unwrap_or_default();

    let before = ok(&router, "ListResolverQueryLogConfigAssociations", json!({})).await;
    let again = ok(&router, "ListResolverQueryLogConfigAssociations", json!({})).await;
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

    router.hold_vpcs(["vpc-000000000000000f0"]);
    let associated = ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({
            "ResolverQueryLogConfigId": first_config.clone(),
            "ResourceId": "vpc-000000000000000f0",
        }),
    )
    .await;
    let appended = string_member(
        associated
            .get("ResolverQueryLogConfigAssociation")
            .unwrap_or_else(|| panic!("the associate answers an association, got {associated}")),
        "Id",
    );
    let after = ok(&router, "ListResolverQueryLogConfigAssociations", json!({})).await;
    let mut with_appended = expected.clone();
    with_appended.push(appended.clone());
    assert_eq!(
        listed_ids(&after),
        with_appended,
        "an association made after the first page must be appended, got {after}"
    );
    assert_eq!(
        count(&after, "TotalCount"),
        i64::try_from(with_appended.len()).unwrap_or_default(),
        "TotalCount must follow the collection, got {after}"
    );

    // A disassociate takes the record out of its configuration, so the next list
    // no longer reports it and the counts fall with it.
    ok(
        &router,
        "DisassociateResolverQueryLogConfig",
        json!({
            "ResolverQueryLogConfigId": first_config,
            "ResourceId": "vpc-000000000000000f0",
        }),
    )
    .await;
    let after_removal = ok(&router, "ListResolverQueryLogConfigAssociations", json!({})).await;
    assert_eq!(
        listed_ids(&after_removal),
        expected,
        "a disassociated association must be gone from the next list, got {after_removal}"
    );
    assert_eq!(
        count(&after_removal, "TotalCount"),
        i64::try_from(expected.len()).unwrap_or_default(),
        "TotalCount must follow the collection down, got {after_removal}"
    );
}
