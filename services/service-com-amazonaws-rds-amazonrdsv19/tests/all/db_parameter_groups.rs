// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's DB-parameter-group slice:
//! `CreateDBParameterGroup` and the `DescribeDBParameterGroups` that reads it back.
//!
//! A DB parameter group is the INSTANCE-scoped sibling of a cluster parameter group, and
//! the control plane every AWS SDK client of the shared `rds` signing name reaches, so
//! the suite drives it the way those clients do: real awsQuery form POSTs through a
//! `MultiServiceRouter` carrying the SigV4 credential scope the router keys on, with
//! every write read back through a describe.
//!
//! The rules are asserted over TABLES spanning the class each rule generalises over,
//! not over one request:
//!
//! * the create-then-read rule over rows differing in the name's case and length, the
//!   engine family (one per engine this front door serves, including one the emulator
//!   publishes a parameter catalog for) and the description;
//! * the name rule over every clause the model states, at the published bound and one
//!   character past it;
//! * the family rule over each published Amazon DocumentDB family, one that carries the
//!   prefix but names no engine version, and families of the other engines;
//! * the store bounds over each caller-sized value a create carries;
//! * the regional scope over two regions and one name;
//! * pagination at the page size and across the marker.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    self, DOCDB_PARAMETER_GROUP_FAMILIES, MAX_FREE_FORM_LEN, MAX_PARAMETER_GROUP_NAME_LEN,
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, texts, REGION};

/// The canonical account id every generated emulator presents, as the sibling suites
/// assert it in an ARN.
const ACCOUNT: &str = "000000000000";

/// How many `tag` elements a response carries.
fn count(xml: &str, tag: &str) -> usize {
    texts(xml, tag).len()
}

/// The ARN a group stored in `region` under `name` reports: the `pg` resource type AWS
/// uses for a DB parameter group under the `rds` ARN namespace.
///
/// `pub(crate)` so the sibling suites of this one test target assert the one ARN shape
/// rather than growing a second copy of it.
pub(crate) fn expected_arn(region: &str, name: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:pg:{name}")
}

/// `CreateDBParameterGroup` with its three required members.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    family: &str,
    description: &str,
) -> (u16, String) {
    call(
        router,
        "CreateDBParameterGroup",
        &[
            ("DBParameterGroupName", name),
            ("DBParameterGroupFamily", family),
            ("Description", description),
        ],
    )
    .await
}

/// `DescribeDBParameterGroups` naming one group.
async fn describe_one(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DescribeDBParameterGroups",
        &[("DBParameterGroupName", name)],
    )
    .await
}

/// One row of the create-then-read table: what the request asks for, and the name the
/// store is expected to key the group under.
struct GroupCase {
    case: &'static str,
    requested_name: &'static str,
    stored_name: &'static str,
    family: &'static str,
    description: &'static str,
}

/// The class the create rule generalises over: the name's case and length, the engine
/// family and the description.
///
/// The families are the forms `CreateDBParameterGroupMessage$DBParameterGroupFamily`
/// documents for the engines this front door serves (`mysql8.0`, `postgres16`,
/// `aurora-mysql8.0`, `oracle-ee-19`, `sqlserver-ex-15.0`), plus one Amazon DocumentDB
/// family the emulator publishes a parameter catalog for.
const GROUP_CASES: &[GroupCase] = &[
    GroupCase {
        case: "a mixed-case name is stored lowercased",
        requested_name: "Probe-PG",
        stored_name: "probe-pg",
        family: "mysql8.0",
        description: "a-mysql-group",
    },
    GroupCase {
        case: "an upper-case name is stored lowercased",
        requested_name: "UPPER-PG",
        stored_name: "upper-pg",
        family: "postgres16",
        description: "a-postgres-group",
    },
    GroupCase {
        case: "an already-lowercase name is stored as sent",
        requested_name: "aurora-pg",
        stored_name: "aurora-pg",
        family: "aurora-mysql8.0",
        description: "an-aurora-group",
    },
    GroupCase {
        case: "a one-character name",
        requested_name: "a",
        stored_name: "a",
        family: "oracle-ee-19",
        description: "an-oracle-group",
    },
    GroupCase {
        case: "a name carrying digits and hyphens",
        requested_name: "pg-9-alpha0",
        stored_name: "pg-9-alpha0",
        family: "sqlserver-ex-15.0",
        description: "a-sqlserver-group",
    },
    GroupCase {
        case: "a family the emulator publishes a catalog for",
        requested_name: "DocDB-PG",
        stored_name: "docdb-pg",
        family: "docdb5.0",
        description: "a-docdb-group",
    },
];

/// A create answers with the group it stored, and every describe of it reports the same
/// record: the name folded to lowercase, the family and description as sent, and the ARN
/// derived from the request's region and the stored name.
#[tokio::test]
async fn a_created_group_is_computed_from_persisted_state() {
    let router = emulator();

    for GroupCase {
        case,
        requested_name,
        stored_name,
        family,
        description,
    } in GROUP_CASES
    {
        let created = expect_ok(case, create(&router, requested_name, family, description).await);
        assert!(
            created.contains("<CreateDBParameterGroupResponse"),
            "case {case:?}: expected the awsQuery CreateDBParameterGroupResponse envelope, \
             got {created}"
        );
        let arn = expected_arn(REGION, stored_name);
        let answered = (
            text(&created, "DBParameterGroupName"),
            text(&created, "DBParameterGroupFamily"),
            text(&created, "Description"),
            text(&created, "DBParameterGroupArn"),
        );
        assert_eq!(
            answered,
            (
                stored_name.to_string(),
                family.to_string(),
                description.to_string(),
                arn.clone(),
            ),
            "case {case:?}: the group the service answers with must be the one the request \
             asked for, got {created}"
        );

        // ...and it reads back by the lowercased name and by the name as sent.
        for (read_case, named) in [
            ("describe by the stored name", *stored_name),
            ("describe by the name as sent", *requested_name),
        ] {
            let case = format!("{case}: {read_case}");
            let listed = expect_ok(&case, describe_one(&router, named).await);
            assert_eq!(
                (
                    count(&listed, "DBParameterGroup"),
                    text(&listed, "DBParameterGroupName"),
                    text(&listed, "DBParameterGroupFamily"),
                    text(&listed, "Description"),
                    text(&listed, "DBParameterGroupArn"),
                ),
                (
                    1,
                    stored_name.to_string(),
                    family.to_string(),
                    description.to_string(),
                    arn.clone(),
                ),
                "case {case:?}: expected exactly the group just created, got {listed}"
            );
        }
    }

    // The unfiltered describe reports every group the region holds, in name order: the
    // ones created above, beside the default group RDS provides for each engine-version
    // family it publishes.
    let listed = expect_ok(
        "describe every group",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[("MaxRecords", "100")],
        )
        .await,
    );
    let mut expected: Vec<String> = GROUP_CASES
        .iter()
        .map(|row| row.stored_name.to_string())
        .collect();
    expected.sort();
    let reported = texts(&listed, "DBParameterGroupName");
    let created: Vec<String> = reported
        .iter()
        .filter(|name| !name.starts_with("default."))
        .cloned()
        .collect();
    assert_eq!(
        created, expected,
        "case \"describe every group\": expected every created group in name order, got {listed}"
    );
    // The AWS-managed catalog is there too: the account holds a default group per family
    // without any request having created one.
    let defaults: Vec<String> = reported
        .iter()
        .filter(|name| name.starts_with("default."))
        .cloned()
        .collect();
    assert!(
        defaults.len() > 1,
        "case \"describe every group\": expected the account's default parameter groups, got \
         {defaults:?} in {listed}"
    );
}

/// A second create of a name the region already holds is the modelled already-exists
/// refusal, whatever case it is sent in, and the stored group is left as it was.
#[tokio::test]
async fn a_name_the_region_already_holds_is_refused_whatever_its_case() {
    let router = emulator();
    expect_ok(
        "the first create",
        create(&router, "Dup-PG", "mysql8.0", "the-first-group").await,
    );

    for named in ["Dup-PG", "dup-pg", "DUP-PG", "dUp-Pg"] {
        let case = format!("a second create named {named:?}");
        expect_error(
            &case,
            create(&router, named, "postgres16", "the-second-group").await,
            400,
            "DBParameterGroupAlreadyExists",
        );
    }

    let listed = expect_ok("the group after the refusals", describe_one(&router, "dup-pg").await);
    assert_eq!(
        (
            count(&listed, "DBParameterGroup"),
            text(&listed, "DBParameterGroupFamily"),
            text(&listed, "Description"),
        ),
        (1, "mysql8.0".to_string(), "the-first-group".to_string()),
        "a refused create must leave the stored group as the first one wrote it, got {listed}"
    );
}

/// One row of the name and family tables: the value under test and whether the rule
/// admits it.
struct RuleCase {
    case: &'static str,
    value: String,
    admitted: bool,
}

/// The name rule, over every clause `DBParameterGroupName` states ("Must be 1 to 255
/// letters, numbers, or hyphens. First character must be a letter. Can't end with a
/// hyphen or contain two consecutive hyphens"), at the published bound and one character
/// past it.
#[tokio::test]
async fn the_documented_name_rule_is_refused_at_its_bounds() {
    let router = emulator();
    let longest = format!("n{}", "a".repeat(MAX_PARAMETER_GROUP_NAME_LEN - 1));
    let too_long = format!("n{}", "a".repeat(MAX_PARAMETER_GROUP_NAME_LEN));
    let rows = [
        RuleCase {
            case: "a plain name",
            value: "names-plain".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "the longest name the rule admits",
            value: longest,
            admitted: true,
        },
        RuleCase {
            case: "one character past the bound",
            value: too_long,
            admitted: false,
        },
        RuleCase {
            case: "a single letter, the shortest name",
            value: "n".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "a name that does not start with a letter",
            value: "9names-digit".to_string(),
            admitted: false,
        },
        RuleCase {
            case: "a name that starts with a hyphen",
            value: "-names-hyphen".to_string(),
            admitted: false,
        },
        RuleCase {
            case: "a name that ends with a hyphen",
            value: "names-trailing-".to_string(),
            admitted: false,
        },
        RuleCase {
            case: "a name carrying two consecutive hyphens",
            value: "names--double".to_string(),
            admitted: false,
        },
        RuleCase {
            case: "a name carrying a character outside the set",
            value: "names.dotted".to_string(),
            admitted: false,
        },
        RuleCase {
            case: "a name carrying the dots a default group's name carries",
            value: "default.mysql8.0".to_string(),
            admitted: false,
        },
    ];

    for RuleCase {
        case,
        value,
        admitted,
    } in rows
    {
        let response = create(&router, &value, "mysql8.0", "a-group-under-test").await;
        if admitted {
            expect_ok(case, response);
            expect_ok(case, describe_one(&router, &value).await);
        } else {
            expect_error(case, response, 400, "InvalidParameterValue");
            // The refusal stored nothing. A name of the `default.` form is the exception:
            // that name is one of the groups RDS provides itself, so the describe reports
            // RDS's group rather than a 404 — and the create was still refused, which is
            // what this row is about.
            if value.starts_with("default.") {
                let listed = expect_ok(case, describe_one(&router, &value).await);
                assert_eq!(
                    text(&listed, "Description"),
                    "Default parameter group for mysql8.0",
                    "case {case:?}: the group of that name is the one RDS provides, not the \
                     refused create's, got {listed}"
                );
            } else {
                expect_error(
                    case,
                    describe_one(&router, &value).await,
                    404,
                    "DBParameterGroupNotFound",
                );
            }
        }
    }
}

/// The family rule: a `docdb`-prefixed family that names no published Amazon DocumentDB
/// engine version is refused, and a family of any other engine this front door serves is
/// admitted — the catalog publishes nothing for it, so refusing it would invent a
/// constraint. The published families come from the catalog ITSELF.
#[tokio::test]
async fn the_family_rule_admits_every_published_family_and_refuses_an_absent_one() {
    let router = emulator();
    let mut rows: Vec<RuleCase> = DOCDB_PARAMETER_GROUP_FAMILIES
        .iter()
        .map(|family| RuleCase {
            case: "a published Amazon DocumentDB family",
            value: (*family).to_string(),
            admitted: true,
        })
        .collect();
    rows.extend([
        RuleCase {
            case: "the prefix in another case",
            value: "DocDB5.0".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "a docdb family that names no engine version",
            value: "docdb9.9".to_string(),
            admitted: false,
        },
        RuleCase {
            case: "another docdb family that names no engine version",
            value: "docdb2.6".to_string(),
            admitted: false,
        },
        RuleCase {
            case: "an RDS for MySQL family",
            value: "mysql8.4".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "an RDS for PostgreSQL family",
            value: "postgres13".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "an Aurora PostgreSQL family",
            value: "aurora-postgresql14".to_string(),
            admitted: true,
        },
        RuleCase {
            case: "an Amazon Neptune family",
            value: "neptune1.3".to_string(),
            admitted: true,
        },
    ]);

    for (index, RuleCase { case, value, admitted }) in rows.into_iter().enumerate() {
        let case = format!("{case} ({value})");
        let name = format!("fam-{index:02}");
        let response = create(&router, &name, &value, "a-group-under-test").await;
        if admitted {
            let created = expect_ok(&case, response);
            assert_eq!(
                text(&created, "DBParameterGroupFamily"),
                value,
                "case {case:?}: the family must be stored as the caller named it, got {created}"
            );
            let listed = expect_ok(&case, describe_one(&router, &name).await);
            assert_eq!(
                text(&listed, "DBParameterGroupFamily"),
                value,
                "case {case:?}: the describe must report the stored family, got {listed}"
            );
        } else {
            expect_error(&case, response, 400, "InvalidParameterValue");
            expect_error(
                &case,
                describe_one(&router, &name).await,
                404,
                "DBParameterGroupNotFound",
            );
        }
    }
}

/// One row of the store-bounds table: the caller-sized values a create carries, and
/// whether the bounds admit them.
struct BoundsCase<'a> {
    case: &'a str,
    name: &'a str,
    family: &'a str,
    description: &'a str,
    tag_key: &'a str,
    tag_value: &'a str,
    tag_count: usize,
    admitted: bool,
}

/// Nothing caller-sized reaches the store unchecked: a create is refused when the
/// description is past the free-form bound, or the tag list past AWS's tagging limits, and
/// the refusal stores nothing.
#[tokio::test]
async fn a_create_refuses_caller_sized_values_the_store_will_not_hold() {
    let router = emulator();
    // The family is no longer a caller-sized value: it has to be one of the published
    // engine-version families, so the free-form bound is driven through `Description`, the
    // other stored string this create carries.
    let longest_description = "d".repeat(MAX_FREE_FORM_LEN);
    let too_long_description = "d".repeat(MAX_FREE_FORM_LEN + 1);
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let too_long_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let too_long_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);

    /// A row whose values are all within the bounds; each case below changes one.
    fn admitted<'a>(case: &'a str, name: &'a str) -> BoundsCase<'a> {
        BoundsCase {
            case,
            name,
            family: "mysql8.0",
            description: "d",
            tag_key: "k",
            tag_value: "v",
            tag_count: 1,
            admitted: true,
        }
    }

    let rows = [
        BoundsCase {
            description: &longest_description,
            ..admitted("the longest description", "bounds-c")
        },
        BoundsCase {
            description: &too_long_description,
            admitted: false,
            ..admitted("one character past the description bound", "bounds-d")
        },
        BoundsCase {
            tag_key: &longest_key,
            ..admitted("the longest tag key", "bounds-e")
        },
        BoundsCase {
            tag_key: &too_long_key,
            admitted: false,
            ..admitted("one character past the tag-key bound", "bounds-f")
        },
        BoundsCase {
            tag_value: &longest_value,
            ..admitted("the longest tag value", "bounds-g")
        },
        BoundsCase {
            tag_value: &too_long_value,
            admitted: false,
            ..admitted("one character past the tag-value bound", "bounds-h")
        },
        BoundsCase {
            tag_count: MAX_TAGS_PER_RESOURCE,
            ..admitted("the tagging limit itself", "bounds-i")
        },
        BoundsCase {
            tag_count: MAX_TAGS_PER_RESOURCE + 1,
            admitted: false,
            ..admitted("one tag past the tagging limit", "bounds-j")
        },
    ];

    for BoundsCase {
        case,
        name,
        family,
        description,
        tag_key: key,
        tag_value: value,
        tag_count,
        admitted,
    } in rows
    {
        // The tag list, as the awsQuery form encodes a list of structures: each entry's
        // members carry the entry's 1-based index.
        let mut indexed: Vec<(String, String)> = Vec::new();
        for index in 1..=tag_count {
            indexed.push((format!("Tags.Tag.{index}.Key"), key.to_string()));
            indexed.push((format!("Tags.Tag.{index}.Value"), value.to_string()));
        }
        let mut params: Vec<(&str, &str)> = vec![
            ("DBParameterGroupName", name),
            ("DBParameterGroupFamily", family),
            ("Description", description),
        ];
        for (key, value) in &indexed {
            params.push((key.as_str(), value.as_str()));
        }

        let response = call(&router, "CreateDBParameterGroup", &params).await;
        if admitted {
            expect_ok(case, response);
        } else {
            expect_error(case, response, 400, "ValidationException");
            // The refusal stored nothing.
            expect_error(
                case,
                describe_one(&router, name).await,
                404,
                "DBParameterGroupNotFound",
            );
        }
    }
}

/// RDS is regional: one name holds a different group in each region, and a describe
/// signed for one region never reports another's.
#[tokio::test]
async fn one_name_names_a_different_group_in_each_region() {
    let router = emulator();
    // (region, family, description)
    let regions = [
        (REGION, "mysql8.0", "the-group-in-the-first-region"),
        ("eu-west-1", "postgres16", "the-group-in-the-second-region"),
        ("ap-south-1", "docdb5.0", "the-group-in-the-third-region"),
    ];

    for (region, family, description) in regions {
        expect_ok(
            region,
            call_in(
                &router,
                region,
                "CreateDBParameterGroup",
                &[
                    ("DBParameterGroupName", "shared-pg"),
                    ("DBParameterGroupFamily", family),
                    ("Description", description),
                ],
            )
            .await,
        );
    }

    for (region, family, description) in regions {
        let listed = expect_ok(
            region,
            call_in(
                &router,
                region,
                "DescribeDBParameterGroups",
                &[("DBParameterGroupName", "shared-pg")],
            )
            .await,
        );
        assert_eq!(
            (
                count(&listed, "DBParameterGroup"),
                text(&listed, "DBParameterGroupFamily"),
                text(&listed, "Description"),
                text(&listed, "DBParameterGroupArn"),
            ),
            (
                1,
                family.to_string(),
                description.to_string(),
                expected_arn(region, "shared-pg"),
            ),
            "case {region:?}: a describe must report the group its own region holds, got {listed}"
        );
    }

    // A name only another region holds is not found here.
    expect_ok(
        "a group only the first region holds",
        create(&router, "only-here-pg", "mysql8.0", "a-group-in-one-region").await,
    );
    for (region, _, _) in regions.iter().skip(1) {
        expect_error(
            region,
            call_in(
                &router,
                region,
                "DescribeDBParameterGroups",
                &[("DBParameterGroupName", "only-here-pg")],
            )
            .await,
            404,
            "DBParameterGroupNotFound",
        );
    }
}

/// The describe refuses what it cannot serve and pages what it can: an unknown name is
/// the modelled 404, a `Filters` it does not support is refused, a `MaxRecords` outside
/// the documented range is refused, and a page resumes from the marker it handed out.
#[tokio::test]
async fn the_describe_refuses_what_it_cannot_serve_and_pages_what_it_can() {
    let router = emulator();

    expect_error(
        "a name no create stored",
        describe_one(&router, "absent-pg").await,
        404,
        "DBParameterGroupNotFound",
    );
    expect_error(
        "a filter the operation does not support",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[
                ("Filters.Filter.1.Name", "db-parameter-group-name"),
                ("Filters.Filter.1.Values.Value.1", "any-pg"),
            ],
        )
        .await,
        400,
        "ValidationException",
    );
    // `MaxRecords`: "Constraints: Minimum 20, maximum 100."
    for (case, max_records) in [
        ("one below the documented minimum", "19"),
        ("one above the documented maximum", "101"),
    ] {
        expect_error(
            case,
            call(
                &router,
                "DescribeDBParameterGroups",
                &[("MaxRecords", max_records)],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
    }

    // 25 groups of the account's own, paged at the documented minimum page size: 20, a
    // marker, then the rest. The walk starts at the token for the first of them, because
    // the default groups RDS provides are reported ahead of them in name order.
    let page_size = 20usize;
    let total = 25usize;
    let mut names: Vec<String> = (0..total).map(|index| format!("page-{index:02}")).collect();
    for name in &names {
        expect_ok(name, create(&router, name, "mysql8.0", "a-paged-group").await);
    }
    names.sort();

    let first = expect_ok(
        "the first page",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[
                ("MaxRecords", page_size.to_string()),
                ("Marker", state::encode_marker(&names[0])),
            ],
        )
        .await,
    );
    assert_eq!(
        texts(&first, "DBParameterGroupName"),
        names[1..=page_size].to_vec(),
        "case \"the first page\": expected the {page_size} groups after the marker in name \
         order, got {first}"
    );
    let marker = text(&first, "Marker");
    assert_eq!(
        marker,
        state::encode_marker(&names[page_size]),
        "case \"the first page\": the marker must be the opaque token for the last group the \
         page served, got {first}"
    );

    let second = expect_ok(
        "the second page",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[
                ("MaxRecords", page_size.to_string()),
                ("Marker", marker.clone()),
            ],
        )
        .await,
    );
    assert_eq!(
        texts(&second, "DBParameterGroupName"),
        names[page_size + 1..].to_vec(),
        "case \"the second page\": expected the groups after the marker, got {second}"
    );
    assert!(
        !second.contains("<Marker>"),
        "case \"the second page\": the last page must carry no marker, got {second}"
    );

    // A marker no page handed out is refused rather than resynchronised.
    expect_error(
        "a marker the service never issued",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[("Marker", "page-99")],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
}

/// The region's group count is capped: filling it is allowed, one more is the modelled
/// quota fault, and another region still has its own room.
#[tokio::test]
async fn the_region_quota_refuses_a_store_that_would_grow_past_it() {
    use service_com_amazonaws_rds_amazonrdsv19::state::DB_PARAMETER_GROUP_QUOTA_PER_REGION;

    let router = emulator();
    for index in 0..DB_PARAMETER_GROUP_QUOTA_PER_REGION {
        let name = format!("quota-{index:03}");
        expect_ok(&name, create(&router, &name, "mysql8.0", "a-quota-group").await);
    }
    expect_error(
        "one group past the region's quota",
        create(&router, "quota-over", "mysql8.0", "a-quota-group").await,
        400,
        "DBParameterGroupQuotaExceeded",
    );
    expect_error(
        "the refused group",
        describe_one(&router, "quota-over").await,
        404,
        "DBParameterGroupNotFound",
    );
    expect_ok(
        "the same name in another region",
        call_in(
            &router,
            "eu-central-1",
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "quota-over"),
                ("DBParameterGroupFamily", "mysql8.0"),
                ("Description", "a-quota-group"),
            ],
        )
        .await,
    );
}
