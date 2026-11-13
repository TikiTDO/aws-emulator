// SPDX-License-Identifier: CC0-1.0

//! OpsCenter's OpsItems end to end: `CreateOpsItem` mints one, `GetOpsItem` reports it.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`], [`error_code`])
//! dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the
//! multi-service router. Nothing reaches into the state struct, so a response that is not
//! computed from persisted state cannot pass: every read follows a write.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — which optional members a create names, the OpsItem type, the
//! severity, the category, the operational-data key and value size, the source spelling,
//! the dedup string, the Region — rather than the single OpsItem a recorded unit happened
//! to create.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// The OpsItem types "Systems Manager supports" (`smithy_query(query="doc",
/// shape="CreateOpsItemRequest", member="OpsItemType")`: "/aws/issue", "/aws/insight",
/// "/aws/changerequest").
const OPS_ITEM_TYPES: &[&str] = &["/aws/issue", "/aws/insight", "/aws/changerequest"];

/// The severities an OpsItem may carry (`smithy_query(query="doc", shape="OpsItem",
/// member="Severity")`: "Severity options range from 1 to 4").
const SEVERITIES: &[&str] = &["1", "2", "3", "4"];

/// The categories an OpsItem may carry (`smithy_query(query="doc", shape="OpsItem",
/// member="Category")`: "Category options include: Availability, Cost, Performance,
/// Recovery, Security").
const CATEGORIES: &[&str] = &[
    "Availability",
    "Cost",
    "Performance",
    "Recovery",
    "Security",
];

/// The strings a `Source` may not contain (`CreateOpsItem`'s `Source` doc: "The source
/// name can't contain the following strings: aws, amazon, and amzn").
const FORBIDDEN_SOURCE_STRINGS: &[&str] = &["aws", "amazon", "amzn"];

/// What an operational-data key may not begin with (`CreateOpsItem`'s `OperationalData`
/// doc: "Operational data keys can't begin with the following: amazon, aws, amzn, ssm,
/// /amazon, /aws, /amzn, /ssm").
const RESERVED_DATA_PREFIXES: &[&str] = &[
    "amazon", "aws", "amzn", "ssm", "/amazon", "/aws", "/amzn", "/ssm",
];

/// "The value has a maximum size of 20 KB" (the same doc).
const MAX_DATA_VALUE: usize = 20 * 1024;

/// `CreateOpsItem` with `body`, returning `(status, body)`.
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "CreateOpsItem", body).await
}

/// `CreateOpsItem`, asserting it succeeded, and returning its response.
///
/// Part of this suite's OpsItem harness, which `all/ops_item_updates.rs` reaches an
/// OpsItem through rather than opening one its own way.
pub(crate) async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateOpsItem must succeed, got {status} {response}"
    );
    response
}

/// `GetOpsItem`'s `OpsItem`, asserting the read succeeded.
pub(crate) async fn get_ok(router: &MultiServiceRouter, case: &str, ops_item_id: &str) -> Value {
    let (status, response) = call(
        router,
        "GetOpsItem",
        &json!({ "OpsItemId": ops_item_id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetOpsItem {ops_item_id} must succeed, got {status} {response}"
    );
    response["OpsItem"].clone()
}

/// A body carrying only `CreateOpsItem`'s three required members, with `title` as the
/// title and the source and description derived from it.
pub(crate) fn minimal(title: &str) -> Value {
    json!({
        "Title": title,
        "Description": format!("what {title} is about"),
        "Source": "emulator-tests",
    })
}

/// Whether `id` has the form the modelled `OpsItemId` pattern requires
/// (`^(oi)-[0-9a-f]{12}$`).
fn is_ops_item_id(id: &str) -> bool {
    id.strip_prefix("oi-").is_some_and(|digits| {
        digits.len() == 12 && digits.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// The ARN the service must mint for the OpsItem `id` names in `region`.
pub(crate) fn ops_item_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ssm:{region}:{ACCOUNT}:opsitem/{id}")
}

/// One row of the round-trip table: which optional members the create names.
struct RoundTrip<'a> {
    case: &'a str,
    body: Value,
}

/// Every form of create round-trips: what the create persisted is what a later
/// `GetOpsItem` reports, and the members the create never named — the status, the version,
/// the creator, the times, the ARN — are the service's own.
#[tokio::test]
async fn created_ops_items_round_trip_through_get() {
    let router = router();
    let cases = vec![
        RoundTrip {
            case: "only the required members",
            body: minimal("A disk is filling up"),
        },
        RoundTrip {
            case: "every optional member of an issue",
            body: json!({
                "Title": "A node stopped reporting",
                "Description": "The SSM Agent on one node has not pinged for an hour.",
                "Source": "emulator-tests",
                "OpsItemType": "/aws/issue",
                "Priority": 1,
                "Category": "Availability",
                "Severity": "2",
                "Notifications": [{ "Arn": "arn:aws:sns:us-west-2:000000000000:ops" }],
                "OperationalData": {
                    "logs": { "Value": "s3://bucket/key", "Type": "SearchableString" },
                    "hint": { "Value": "restart the agent", "Type": "String" }
                },
                "Tags": [{ "Key": "Team", "Value": "Platform" }]
            }),
        },
        RoundTrip {
            case: "an insight with a priority at the top of the modelled range",
            body: json!({
                "Title": "Duplicate OpsItems about one node",
                "Description": "Several OpsItems name the same impacted resource.",
                "Source": "emulator-tests",
                "OpsItemType": "/aws/insight",
                "Priority": 5,
                "Category": "Performance",
                "Severity": "4"
            }),
        },
        RoundTrip {
            case: "a change request carrying all four workflow times",
            body: json!({
                "Title": "Patch the fleet",
                "Description": "Apply the monthly patch baseline.",
                "Source": "emulator-tests",
                "OpsItemType": "/aws/changerequest",
                "ActualStartTime": 1_700_000_000,
                "ActualEndTime": 1_700_003_600,
                "PlannedStartTime": 1_700_000_000,
                "PlannedEndTime": 1_700_007_200
            }),
        },
        RoundTrip {
            case: "a service-defined operational-data key and a related OpsItem",
            body: json!({
                "Title": "An S3 bucket became public",
                "Description": "A bucket policy now grants public read.",
                "Source": "emulator-tests",
                "Category": "Security",
                "Severity": "1",
                "OperationalData": {
                    "/aws/resources": {
                        "Value": "[{\"arn\":\"arn:aws:s3:::a-bucket\"}]",
                        "Type": "SearchableString"
                    },
                    "/aws/automations": {
                        "Value": "[{\"automationType\":\"AWS:SSM:ExecuteAutomation\"}]",
                        "Type": "String"
                    }
                }
            }),
        },
        RoundTrip {
            case: "an operational-data value exactly at the published size limit",
            body: json!({
                "Title": "A long error string",
                "Description": "The error string is as long as operational data may be.",
                "Source": "emulator-tests",
                "OperationalData": {
                    "error": { "Value": "e".repeat(MAX_DATA_VALUE), "Type": "String" }
                }
            }),
        },
    ];

    let mut seen: Vec<String> = Vec::new();
    for RoundTrip { case, body } in &cases {
        let created = create_ok(&router, case, body).await;
        let id = created["OpsItemId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: CreateOpsItem reports an OpsItemId, got {created}"))
            .to_string();
        assert!(
            is_ops_item_id(&id),
            "case {case}: expected an OpsItemId of the form oi-<12 hex digits>, got {id:?}"
        );
        assert_eq!(
            created["OpsItemArn"].as_str(),
            Some(ops_item_arn(REGION, &id).as_str()),
            "case {case}: expected the OpsItem's ARN, got {created}"
        );
        assert!(
            !seen.contains(&id),
            "case {case}: expected an id no other OpsItem carries, got {id:?} among {seen:?}"
        );
        seen.push(id.clone());

        let read = get_ok(&router, case, &id).await;
        // Everything the request named is reported back as it was sent.
        for member in [
            "Title",
            "Description",
            "Source",
            "Priority",
            "Category",
            "Severity",
            "Notifications",
            "OperationalData",
            "ActualStartTime",
            "ActualEndTime",
            "PlannedStartTime",
            "PlannedEndTime",
        ] {
            if let Some(sent) = body.get(member) {
                assert_eq!(
                    &read[member], sent,
                    "case {case}: expected {member} {sent}, got {}",
                    read[member]
                );
            }
        }
        // The type a create named, or the documented default for a create that named none.
        let expected_type = body
            .get("OpsItemType")
            .and_then(Value::as_str)
            .unwrap_or("/aws/issue");
        assert_eq!(
            read["OpsItemType"].as_str(),
            Some(expected_type),
            "case {case}: expected OpsItemType {expected_type}, got {}",
            read["OpsItemType"]
        );
        assert_eq!(
            read["Status"].as_str(),
            Some("Open"),
            "case {case}: expected a created OpsItem to be Open, got {}",
            read["Status"]
        );
        // `OpsItem$Version` is a token the service derives at every write of the record,
        // so a create reports one of its own rather than a value the request could predict.
        // What the token does either side of an edit is `all/ops_item_updates.rs`'s.
        assert!(
            read["Version"]
                .as_str()
                .is_some_and(|token| token.parse::<u64>().is_ok_and(|value| value > 0)),
            "case {case}: expected a minted version token, got {}",
            read["Version"]
        );
        assert_eq!(
            read["OpsItemId"].as_str(),
            Some(id.as_str()),
            "case {case}: expected the read to report its own id, got {}",
            read["OpsItemId"]
        );
        assert_eq!(
            read["OpsItemArn"].as_str(),
            Some(ops_item_arn(REGION, &id).as_str()),
            "case {case}: expected the OpsItem's ARN, got {}",
            read["OpsItemArn"]
        );
        for member in ["CreatedBy", "LastModifiedBy"] {
            let reported = read[member].as_str().unwrap_or_default();
            assert!(
                reported.starts_with("arn:aws:iam::") && reported.contains(ACCOUNT),
                "case {case}: expected {member} to be an ARN of this account, got {reported:?}"
            );
        }
        for member in ["CreatedTime", "LastModifiedTime"] {
            assert!(
                read[member].as_f64().is_some_and(|epoch| epoch > 0.0),
                "case {case}: expected {member} to be an epoch instant, got {}",
                read[member]
            );
        }
        assert_eq!(
            read["CreatedTime"], read["LastModifiedTime"],
            "case {case}: an unedited OpsItem was last modified when it was created, got {} and {}",
            read["CreatedTime"], read["LastModifiedTime"]
        );
    }

    // A read that followed five other creates still reports the first OpsItem, so a
    // later create never overwrites an earlier OpsItem's record.
    let first = get_ok(&router, "the first OpsItem after every other create", &seen[0]).await;
    assert_eq!(
        first["Title"].as_str(),
        Some("A disk is filling up"),
        "expected the first OpsItem to be unchanged, got {first}"
    );
}

/// A `Source` carrying one of the forbidden strings is refused, whatever its spelling or
/// where in the name it sits; one that carries none is accepted.
#[tokio::test]
async fn a_source_carrying_a_forbidden_string_is_refused() {
    let router = router();
    let mut refused: Vec<String> = Vec::new();
    for forbidden in FORBIDDEN_SOURCE_STRINGS {
        refused.push((*forbidden).to_string());
        refused.push(format!("my-{forbidden}-source"));
        refused.push(forbidden.to_uppercase());
        refused.push(format!("{}{}", forbidden[..1].to_uppercase(), &forbidden[1..]));
    }
    for source in &refused {
        let body = json!({
            "Title": "A source that names the service",
            "Description": "The source carries one of the forbidden strings.",
            "Source": source,
        });
        let (status, response) = create(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {source:?}: expected the create to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemInvalidParameterException",
            "case {source:?}: expected OpsItemInvalidParameterException, got {response}"
        );
        assert_eq!(
            response["ParameterNames"],
            json!(["Source"]),
            "case {source:?}: expected the refusal to name Source, got {response}"
        );
    }

    for source in ["emulator-tests", "EC2", "my-own-monitor", "Trusted Advisor"] {
        let created = create_ok(&router, source, &json!({
            "Title": format!("An OpsItem from {source}"),
            "Description": "The source carries none of the forbidden strings.",
            "Source": source,
        }))
        .await;
        let id = created["OpsItemId"].as_str().unwrap_or_default().to_string();
        let read = get_ok(&router, source, &id).await;
        assert_eq!(
            read["Source"].as_str(),
            Some(source),
            "case {source:?}: expected the source to round-trip, got {}",
            read["Source"]
        );
    }
}

/// An operational-data key beginning with a reserved prefix is refused unless it is one
/// of the keys Systems Manager itself defines.
#[tokio::test]
async fn reserved_operational_data_keys_are_refused() {
    let router = router();
    let mut refused: Vec<String> = Vec::new();
    for prefix in RESERVED_DATA_PREFIXES {
        refused.push((*prefix).to_string());
        refused.push(format!("{prefix}/something"));
        refused.push(format!("{prefix}-detail"));
    }
    // A key the service defines, spelled in another casing, is a reserved key the service
    // does not define: the reads of those keys look them up as the service spells them, so
    // admitting another casing would store data no read would ever find.
    for miscased in ["/AWS/dedup", "/Aws/resources", "/AWS/AUTOMATIONS"] {
        refused.push(miscased.to_string());
    }
    for key in &refused {
        let body = json!({
            "Title": "An OpsItem with a reserved data key",
            "Description": "The operational-data key begins with a reserved prefix.",
            "Source": "emulator-tests",
            "OperationalData": { key: { "Value": "whatever", "Type": "String" } },
        });
        let (status, response) = create(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {key:?}: expected the create to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemInvalidParameterException",
            "case {key:?}: expected OpsItemInvalidParameterException, got {response}"
        );
        assert_eq!(
            response["ParameterNames"],
            json!(["OperationalData"]),
            "case {key:?}: expected the refusal to name OperationalData, got {response}"
        );
    }

    // The keys the service defines under those prefixes, and keys that merely CONTAIN a
    // reserved word without beginning with one.
    for key in [
        "/aws/resources",
        "/aws/automations",
        "/aws/dedup",
        "my-aws-notes",
        "team/aws",
    ] {
        let value = if key == "/aws/dedup" {
            format!("{{\"dedupString\": \"{key}\"}}")
        } else {
            "whatever".to_string()
        };
        let created = create_ok(&router, key, &json!({
            "Title": format!("An OpsItem keyed with {key}"),
            "Description": "The operational-data key is one a caller may send.",
            "Source": "emulator-tests",
            "OperationalData": { key: { "Value": value, "Type": "String" } },
        }))
        .await;
        let id = created["OpsItemId"].as_str().unwrap_or_default().to_string();
        let read = get_ok(&router, key, &id).await;
        assert_eq!(
            read["OperationalData"][key]["Value"].as_str(),
            Some(value.as_str()),
            "case {key:?}: expected the operational data to round-trip, got {}",
            read["OperationalData"]
        );
    }
}

/// An operational-data value is accepted at the published size limit and refused one byte
/// past it.
#[tokio::test]
async fn an_operational_data_value_is_bounded_at_twenty_kilobytes() {
    let router = router();
    let cases: &[(&str, usize, bool)] = &[
        ("one byte", 1, true),
        ("half the limit", MAX_DATA_VALUE / 2, true),
        ("exactly the limit", MAX_DATA_VALUE, true),
        ("one byte past the limit", MAX_DATA_VALUE + 1, false),
        ("twice the limit", MAX_DATA_VALUE * 2, false),
    ];
    for (case, size, accepted) in cases {
        let body = json!({
            "Title": format!("An OpsItem whose data value is {case}"),
            "Description": "The operational-data value is sized by the case.",
            "Source": "emulator-tests",
            "OperationalData": { "payload": { "Value": "p".repeat(*size), "Type": "String" } },
        });
        let (status, response) = create(&router, &body).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: a {size}-byte value is within the limit, got {status} {response}"
            );
            let id = response["OpsItemId"].as_str().unwrap_or_default().to_string();
            let read = get_ok(&router, case, &id).await;
            assert_eq!(
                read["OperationalData"]["payload"]["Value"]
                    .as_str()
                    .map(str::len),
                Some(*size),
                "case {case}: expected a {size}-byte value to round-trip, got {}",
                read["OperationalData"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: a {size}-byte value is over the limit, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "OpsItemInvalidParameterException",
                "case {case}: expected OpsItemInvalidParameterException, got {response}"
            );
        }
    }
}

/// Each documented value set is enforced: a type, a severity or a category the service
/// does not support is refused, naming the member, and every documented value is
/// accepted.
#[tokio::test]
async fn documented_value_sets_are_enforced() {
    let router = router();
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "OpsItemType",
            OPS_ITEM_TYPES,
            &["/aws/incident", "issue", "/AWS/ISSUE", "/aws/issue/extra"],
        ),
        ("Severity", SEVERITIES, &["0", "5", "HIGH", "1.0"]),
        (
            "Category",
            CATEGORIES,
            &["Compliance", "availability", "Cost " ],
        ),
    ];
    for (member, accepted, refused) in cases {
        for value in *accepted {
            let mut body = minimal(&format!("An OpsItem whose {member} is {value}"));
            body[*member] = json!(value);
            let created = create_ok(&router, &format!("{member}={value}"), &body).await;
            let id = created["OpsItemId"].as_str().unwrap_or_default().to_string();
            let read = get_ok(&router, &format!("{member}={value}"), &id).await;
            assert_eq!(
                read[*member].as_str(),
                Some(*value),
                "case {member}={value}: expected the value to round-trip, got {}",
                read[*member]
            );
        }
        for value in *refused {
            let mut body = minimal(&format!("An OpsItem whose {member} is {value}"));
            body[*member] = json!(value);
            let (status, response) = create(&router, &body).await;
            assert_eq!(
                status, 400,
                "case {member}={value:?}: expected the create to be refused, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "OpsItemInvalidParameterException",
                "case {member}={value:?}: expected OpsItemInvalidParameterException, got {response}"
            );
            assert_eq!(
                response["ParameterNames"],
                json!([member]),
                "case {member}={value:?}: expected the refusal to name {member}, got {response}"
            );
        }
    }
}

/// A create carrying a dedup string an OpsItem is already open under is refused and told
/// which OpsItem that is; a different dedup string, a value that carries none, and a
/// create in another Region all open their own OpsItem.
#[tokio::test]
async fn a_dedup_string_already_open_is_refused_with_the_existing_id() {
    let router = router();
    /// A create whose `/aws/dedup` value carries `dedup`.
    fn with_dedup(title: &str, dedup: &str) -> Value {
        json!({
            "Title": title,
            "Description": format!("{title}, deduplicated on {dedup}"),
            "Source": "emulator-tests",
            "OperationalData": {
                "/aws/dedup": {
                    "Value": format!("{{\"dedupString\": \"{dedup}\"}}"),
                    "Type": "SearchableString"
                }
            }
        })
    }

    let dedups = ["disk-full", "/host/a:/dev/sda1", "agent-offline"];
    let mut first_ids: Vec<String> = Vec::new();
    for dedup in dedups {
        let created = create_ok(&router, dedup, &with_dedup("A deduplicated OpsItem", dedup)).await;
        first_ids.push(created["OpsItemId"].as_str().unwrap_or_default().to_string());
    }
    for (dedup, held) in dedups.iter().zip(&first_ids) {
        let (status, response) =
            create(&router, &with_dedup("The same work item again", dedup)).await;
        assert_eq!(
            status, 400,
            "case {dedup}: expected the second create to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemAlreadyExistsException",
            "case {dedup}: expected OpsItemAlreadyExistsException, got {response}"
        );
        assert_eq!(
            response["OpsItemId"].as_str(),
            Some(held.as_str()),
            "case {dedup}: expected the refusal to name the OpsItem already open, got {response}"
        );
    }

    // A dedup string nobody holds, and a `/aws/dedup` value that carries no dedup string
    // at all, both open their own OpsItem.
    for body in [
        with_dedup("A work item nobody filed yet", "a-brand-new-string"),
        json!({
            "Title": "A dedup key with no dedup string",
            "Description": "The /aws/dedup value is not a dedup document.",
            "Source": "emulator-tests",
            "OperationalData": {
                "/aws/dedup": { "Value": "{\"other\": \"disk-full\"}", "Type": "String" }
            }
        }),
    ] {
        let created = create_ok(&router, "an un-held dedup string", &body).await;
        let id = created["OpsItemId"].as_str().unwrap_or_default().to_string();
        assert!(
            !first_ids.contains(&id),
            "case {body}: expected its own OpsItem, got {id:?} among {first_ids:?}"
        );
    }

    // Dedup is per Region: the same dedup string at another Region's endpoint opens a
    // second OpsItem, because an OpsItem lives in the Region it was created in.
    let (status, elsewhere) = call_in_region(
        &router,
        "eu-central-1",
        "CreateOpsItem",
        &with_dedup("The same dedup string in another Region", dedups[0]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case another Region: the dedup string is held in us-west-2 only, got {status} {elsewhere}"
    );
    assert!(
        !first_ids.contains(&elsewhere["OpsItemId"].as_str().unwrap_or_default().to_string()),
        "case another Region: expected its own OpsItem, got {elsewhere}"
    );
}

/// A `GetOpsItem` of an id no OpsItem carries is refused, and an OpsItem is reached only
/// through the endpoint of the Region it was created in.
#[tokio::test]
async fn a_get_of_an_ops_item_nobody_created_is_refused() {
    let router = router();
    let created = create_ok(
        &router,
        "the one OpsItem this store holds",
        &minimal("The only OpsItem here"),
    )
    .await;
    let held = created["OpsItemId"].as_str().unwrap_or_default().to_string();

    // Well-formed ids the store holds nothing under, differing in their digits.
    for id in [
        "oi-000000000000",
        "oi-ffffffffffff",
        "oi-0123456789ab",
        "oi-deadbeef0123",
    ] {
        let (status, response) = call(&router, "GetOpsItem", &json!({ "OpsItemId": id })).await;
        assert_eq!(
            status, 400,
            "case {id}: expected the read to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemNotFoundException",
            "case {id}: expected OpsItemNotFoundException, got {response}"
        );
    }

    // The id the store DOES hold, read through another Region's endpoint.
    for region in ["us-east-1", "eu-central-1", "ap-southeast-2"] {
        let (status, response) = call_in_region(
            &router,
            region,
            "GetOpsItem",
            &json!({ "OpsItemId": held }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region}: an OpsItem of us-west-2 is not held there, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemNotFoundException",
            "case {region}: expected OpsItemNotFoundException, got {response}"
        );
    }

    // And through its own, where it is held.
    let read = get_ok(&router, "its own Region", &held).await;
    assert_eq!(
        read["OpsItemId"].as_str(),
        Some(held.as_str()),
        "expected the OpsItem to be read through its own Region, got {read}"
    );
}

/// An `OpsItemArn` the request names must be the ARN of the OpsItem its id names, and an
/// ARN of any other account is refused: this emulator serves exactly one account.
#[tokio::test]
async fn an_ops_item_arn_must_name_this_ops_item_in_this_account() {
    let router = router();
    let created = create_ok(&router, "the OpsItem under test", &minimal("An OpsItem with an ARN")).await;
    let held = created["OpsItemId"].as_str().unwrap_or_default().to_string();

    let (status, response) = call(
        &router,
        "GetOpsItem",
        &json!({ "OpsItemId": held, "OpsItemArn": ops_item_arn(REGION, &held) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case its own ARN: the read must succeed, got {status} {response}"
    );
    assert_eq!(
        response["OpsItem"]["OpsItemId"].as_str(),
        Some(held.as_str()),
        "case its own ARN: expected the OpsItem the id names, got {response}"
    );

    let cases: &[(&str, String, &str)] = &[
        (
            "an ARN of this account naming another OpsItem",
            ops_item_arn(REGION, "oi-0123456789ab"),
            "OpsItemNotFoundException",
        ),
        (
            "an ARN of this account in another Region",
            ops_item_arn("us-east-1", &held),
            "OpsItemNotFoundException",
        ),
        (
            "an ARN of another account",
            format!("arn:aws:ssm:{REGION}:111122223333:opsitem/{held}"),
            "OpsItemAccessDeniedException",
        ),
    ];
    for (case, arn, expected) in cases {
        let (status, response) = call(
            &router,
            "GetOpsItem",
            &json!({ "OpsItemId": held, "OpsItemArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the read to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            *expected,
            "case {case}: expected {expected}, got {response}"
        );
    }
}

/// A create that names an account other than the one this emulator serves is refused:
/// cross-account OpsItems need an account configured for them, and none is.
#[tokio::test]
async fn a_create_in_another_account_is_refused() {
    let router = router();
    for account in ["111122223333", "000000000001", "999999999999"] {
        let mut body = minimal("An OpsItem for somebody else's account");
        body["AccountId"] = json!(account);
        let (status, response) = create(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {account}: expected the create to be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "OpsItemAccessDeniedException",
            "case {account}: expected OpsItemAccessDeniedException, got {response}"
        );
    }

    // The account this emulator serves, named explicitly, is the account the create
    // targets anyway.
    let mut body = minimal("An OpsItem for this very account");
    body["AccountId"] = json!(ACCOUNT);
    let created = create_ok(&router, "this account", &body).await;
    let id = created["OpsItemId"].as_str().unwrap_or_default().to_string();
    let read = get_ok(&router, "this account", &id).await;
    assert_eq!(
        read["Title"].as_str(),
        Some("An OpsItem for this very account"),
        "expected the OpsItem to be held in this account, got {read}"
    );
}

/// Every create mints its own id, whatever the requests have in common: two OpsItems with
/// identical members are two work items.
#[tokio::test]
async fn every_create_mints_its_own_id() {
    let router = router();
    let mut ids: Vec<String> = Vec::new();
    for round in 0..12 {
        let created = create_ok(
            &router,
            &format!("round {round}"),
            &minimal("One of many identical OpsItems"),
        )
        .await;
        let id = created["OpsItemId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_ops_item_id(&id),
            "case round {round}: expected an OpsItemId of the form oi-<12 hex digits>, got {id:?}"
        );
        assert!(
            !ids.contains(&id),
            "case round {round}: expected a fresh id, got {id:?} among {ids:?}"
        );
        ids.push(id);
    }
    // Each id still resolves to its own OpsItem after every other create.
    for (round, id) in ids.iter().enumerate() {
        let read = get_ok(&router, &format!("round {round}"), id).await;
        assert_eq!(
            read["OpsItemId"].as_str(),
            Some(id.as_str()),
            "case round {round}: expected the id to resolve to its own OpsItem, got {read}"
        );
    }
}
