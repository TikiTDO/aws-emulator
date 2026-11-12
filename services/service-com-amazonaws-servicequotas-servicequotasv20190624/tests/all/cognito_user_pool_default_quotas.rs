// SPDX-License-Identifier: CC0-1.0

//! Amazon Cognito User Pools' published default-quota catalog, over the wire.
//!
//! The rule under test is about the CATALOG rather than about one row: every quota this
//! emulator publishes for a `ServiceCode` is keyed on the `QuotaCode` AWS mints for THAT
//! quota and carries that quota's own published facts — its name, default value, unit,
//! adjustability, and the window, usage metric and text AWS publishes it with, each only
//! where this catalog can cite it — and the service publishes nothing under a code AWS
//! does not mint for it, so a code AWS never minted resolves nowhere and a code AWS minted
//! for a quota this catalog cannot cite resolves nowhere either.
//!
//! The table spans the whole published `cognito-idp` catalog, and its rows differ along
//! everything the rule generalises over: a quota published as a RATE against one
//! published as a COUNT, an adjustable one against a fixed one, a rate served per user
//! pool against one served for the whole account, and values from 15 to 1000. Each row is
//! asserted through the listing, through both single reads and in two Regions, because
//! the facts travel with the quota's code rather than with the call that reads it.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas, emulator, Client};

/// The `ServiceCode` under test, and the display name every one of its rows reports.
const SERVICE_CODE: &str = "cognito-idp";
const SERVICE_NAME: &str = "Amazon Cognito User Pools";

/// The `AWS/Usage` `Service` dimension Amazon Cognito User Pools publishes its call
/// counts under.
const USAGE_SERVICE: &str = "Cognito User Pool";

/// What AWS publishes a `cognito-idp` quota as.
enum Published {
    /// The combined call rate of one throttling CATEGORY, over a one-second window and
    /// metered by that category's `AWS/Usage` call count.
    CategoryRate {
        /// The category the rate is served for, which is the subject of the quota's name,
        /// of its description and of its metric's `Resource` dimension alike.
        category: &'static str,
    },
    /// A COUNT of the resources of one plane: no window, no call count, and no
    /// `Description` — the service publishes one for each of these quotas and its text is
    /// not among the data available here, unlike the category sentence, which is fixed
    /// text around the category's own name. The catalog publishes no text it cannot cite,
    /// so what is asserted here is that the member is ABSENT rather than filled with
    /// wording authored in this emulator and served as AWS's.
    Count,
}

/// One published row of the catalog.
struct Row {
    /// AWS's own `QuotaCode` for the quota.
    quota_code: &'static str,
    /// `QuotaName`, as AWS publishes it.
    quota_name: &'static str,
    /// The AWS default `Value`.
    value: f64,
    /// Whether AWS lets an increase be requested.
    adjustable: bool,
    /// What AWS publishes the quota as.
    kind: Published,
}

/// The whole published `cognito-idp` catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `cognito-idp` service code, as
/// `src/state/quota_rows/cognito_idp.rs` carries it — the category rates and the resource
/// counts of Amazon Cognito User Pools. Restated here so a row that silently changes its
/// code, its value, its plane or its class fails.
const ROWS: &[Row] = &[
    // A category rate AWS publishes as FIXED.
    Row {
        quota_code: "L-574C86AE",
        quota_name: "Rate of UserResourceUpdate requests",
        value: 25.0,
        adjustable: false,
        kind: Published::CategoryRate {
            category: "UserResourceUpdate",
        },
    },
    // ... and one it lets you provision a rate limit within.
    Row {
        quota_code: "L-5987B8A0",
        quota_name: "Rate of UserCreation requests",
        value: 50.0,
        adjustable: true,
        kind: Published::CategoryRate {
            category: "UserCreation",
        },
    },
    // The COUNT of the account's user pools, and the count of one pool's resource
    // servers: neither is a rate, so neither carries a window or a call count.
    Row {
        quota_code: "L-66E6DF30",
        quota_name: "User pools per account",
        value: 1000.0,
        adjustable: true,
        kind: Published::Count,
    },
    Row {
        quota_code: "L-7CDAF993",
        quota_name: "Resource servers per user pool",
        value: 25.0,
        adjustable: true,
        kind: Published::Count,
    },
    Row {
        quota_code: "L-7D6E8ED3",
        quota_name: "Rate of UserAccountRecovery requests",
        value: 30.0,
        adjustable: false,
        kind: Published::CategoryRate {
            category: "UserAccountRecovery",
        },
    },
    // A category whose rate AWS serves for the WHOLE ACCOUNT, which is the plane the
    // quota's own name states.
    Row {
        quota_code: "L-B7575496",
        quota_name: "Rate of UserPoolResourceUpdate requests per account",
        value: 15.0,
        adjustable: false,
        kind: Published::CategoryRate {
            category: "UserPoolResourceUpdate",
        },
    },
    Row {
        quota_code: "L-D6BD5178",
        quota_name: "Rate of UserRead requests",
        value: 120.0,
        adjustable: true,
        kind: Published::CategoryRate {
            category: "UserRead",
        },
    },
];

/// Codes that resolve NOTHING for this service: the two descriptive stand-ins AWS never
/// mints, and codes in AWS's own namespace whose quota this catalog cannot cite.
///
/// SOURCE: the shape of an AWS-minted code (`L-` and eight hex characters, as
/// `catalog_identity.rs` asserts over the whole catalog) for the stand-ins, and the rows
/// `src/state/quota_rows/cognito_idp.rs` does NOT carry for the minted codes.
const UNRESOLVED_CODES: &[&str] = &[
    "UserPoolsPerAccount",
    "AppClientsPerUserPool",
    "L-026ADBA3",
    "L-F21F8BB4",
];

/// The `Description` AWS publishes for a category rate: fixed text whose only variable
/// part is the category's own name, worded as the provisionable sentence for a rate AWS
/// lets you request an increase for and as the shared-quota sentence for a fixed one.
fn category_sentence(category: &str, adjustable: bool) -> String {
    if adjustable {
        format!(
            "The maximum combined call rate (requests per second) you can request for all API \
             operations in the {category} category. Provision your rate limit within this quota \
             from the Amazon Cognito console or APIs. For the list of included operations, see \
             https://docs.aws.amazon.com/cognito/latest/developerguide/limits.html#category_operations."
        )
    } else {
        format!(
            "The maximum combined call rate (requests per second) for all API operations in the \
             {category} category. All operations within a category share the same quota. For the \
             list of included operations, see \
             https://docs.aws.amazon.com/cognito/latest/developerguide/limits.html#category_operations."
        )
    }
}

/// The whole record AWS publishes for `row` in `region`, as the wire reports it.
fn expected_record(row: &Row, region: &str) -> Value {
    let mut record = json!({
        "ServiceCode": SERVICE_CODE,
        "ServiceName": SERVICE_NAME,
        "QuotaArn": format!(
            "arn:aws:servicequotas:{region}::{SERVICE_CODE}/{}",
            row.quota_code
        ),
        "QuotaCode": row.quota_code,
        "QuotaName": row.quota_name,
        "Value": row.value,
        "Unit": "None",
        "Adjustable": row.adjustable,
        "GlobalQuota": false,
    });
    match row.kind {
        Published::CategoryRate { category } => {
            record["Description"] = json!(category_sentence(category, row.adjustable));
            record["Period"] = json!({"PeriodValue": 1, "PeriodUnit": "SECOND"});
            record["UsageMetric"] = json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "CallCount",
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {
                    "Service": USAGE_SERVICE,
                    "Class": "None",
                    "Resource": category,
                    "Type": "API",
                },
            });
        }
        Published::Count => {}
    }
    record
}

/// The row the listing published under `quota_code`, if any.
fn published<'rows>(rows: &'rows [Value], quota_code: &str) -> Option<&'rows Value> {
    rows.iter()
        .find(|row| row["QuotaCode"] == json!(quota_code))
}

/// Assert `served` carries exactly the record `row` publishes, member by member, and
/// nothing else.
fn assert_record(case: &str, served: &Value, row: &Row, region: &str) {
    let expected = expected_record(row, region);
    for (member, value) in expected.as_object().expect("an object") {
        assert_eq!(
            &served[member],
            value,
            "case {case}: expected {member} {value}, got {served}"
        );
    }
    // A count publishes no window, no call count and no description: a row that grew one
    // would be reporting a rate's facts, or this emulator's own wording, for a quota AWS
    // publishes as a count.
    if matches!(row.kind, Published::Count) {
        assert!(
            served.get("Period").is_none_or(Value::is_null)
                && served.get("UsageMetric").is_none_or(Value::is_null)
                && served.get("Description").is_none_or(Value::is_null),
            "case {case}: a count is published over no window, with no call count and with no \
             text this catalog cannot cite, got {served}"
        );
    }
}

/// Every row of the catalog reports the facts AWS publishes under its own code, through
/// the listing and through both single reads, in every Region the catalog is served in.
#[tokio::test]
async fn every_published_row_reports_the_facts_aws_publishes_under_its_code() {
    let engine = emulator();

    for region in ["us-west-2", "eu-central-1"] {
        let caller = client(&engine, region);
        let listed = default_quotas(&caller, SERVICE_CODE).await;

        for row in ROWS {
            let case = format!("{region} {SERVICE_CODE}/{}", row.quota_code);
            let served = published(&listed, row.quota_code).unwrap_or_else(|| {
                panic!("case {case}: the listing must publish this quota, got {listed:?}")
            });
            assert_record(&case, served, row, region);

            let pair = json!({"ServiceCode": SERVICE_CODE, "QuotaCode": row.quota_code});
            let read = caller
                .ok("GetAWSDefaultServiceQuota", pair.clone())
                .await["Quota"]
                .clone();
            assert_eq!(
                &read, served,
                "case {case}: the single read must return the record the listing carries, got \
                 {read}"
            );

            // The applied plane is keyed on the same code, so a client that read a code
            // out of the default catalog resolves its value in force too. Its ARN names
            // the account whose value is in force, which the AWS default's does not, so
            // what travels with the code is the quota's own published text and value.
            let applied = caller.ok("GetServiceQuota", pair).await["Quota"].clone();
            assert_eq!(
                (
                    applied["QuotaCode"].clone(),
                    applied["QuotaName"].clone(),
                    applied["Value"].clone(),
                    applied["Adjustable"].clone(),
                    applied["Description"].clone(),
                    applied["Period"].clone(),
                    applied["UsageMetric"].clone(),
                ),
                (
                    served["QuotaCode"].clone(),
                    served["QuotaName"].clone(),
                    served["Value"].clone(),
                    served["Adjustable"].clone(),
                    served["Description"].clone(),
                    served["Period"].clone(),
                    served["UsageMetric"].clone(),
                ),
                "case {case}: the applied read must resolve the same quota's published facts, \
                 got {applied}"
            );
        }
    }
}

/// The catalog publishes EXACTLY the quotas it cites: no code AWS never minted, and no
/// row beyond the table above.
#[tokio::test]
async fn the_catalog_publishes_exactly_the_quotas_it_cites() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    let expected: BTreeSet<&str> = ROWS.iter().map(|row| row.quota_code).collect();
    for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
        let listed = crate::quota_workflow::walk_pages(
            &caller,
            operation,
            &json!({"ServiceCode": SERVICE_CODE}),
            "Quotas",
            None,
        )
        .await;
        let codes: BTreeSet<&str> = listed
            .iter()
            .filter_map(|row| row["QuotaCode"].as_str())
            .collect();
        assert_eq!(
            codes, expected,
            "case {operation}: expected exactly the cited published quotas, got {codes:?}"
        );
        assert_eq!(
            codes.len(),
            listed.len(),
            "case {operation}: every published quota has a code of its own, got {listed:?}"
        );
    }
}

/// A code this service publishes nothing under resolves nowhere, on every plane that
/// resolves a code — while the service itself still answers its listing.
#[tokio::test]
async fn a_code_the_service_publishes_nothing_under_resolves_nowhere() {
    let engine = emulator();
    let caller = client(&engine, "us-east-1");

    for code in UNRESOLVED_CODES {
        for operation in [
            "GetAWSDefaultServiceQuota",
            "GetServiceQuota",
            "RequestServiceQuotaIncrease",
        ] {
            let mut body = json!({"ServiceCode": SERVICE_CODE, "QuotaCode": code});
            if operation == "RequestServiceQuotaIncrease" {
                body["DesiredValue"] = json!(100.0);
            }
            let (status, error) = caller.refused(operation, body).await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {SERVICE_CODE}/{code}: {operation} must refuse a code this service \
                 publishes nothing under"
            );
        }
    }

    assert!(
        !default_quotas(&caller, SERVICE_CODE).await.is_empty(),
        "case {SERVICE_CODE}: the service code must still resolve its listing"
    );
}

/// Both reads of an unknown pair are refused the same way whatever Region they are
/// addressed to: the catalog is the same catalog in every Region.
#[tokio::test]
async fn the_published_catalog_is_the_same_catalog_in_every_region() {
    let engine = emulator();

    let mut seen: Vec<Vec<String>> = Vec::new();
    for region in ["us-west-2", "eu-west-1", "ap-northeast-1"] {
        let caller: Client = client(&engine, region);
        seen.push(
            default_quotas(&caller, SERVICE_CODE)
                .await
                .iter()
                .filter_map(|row| row["QuotaCode"].as_str().map(str::to_string))
                .collect(),
        );
    }
    let first = seen[0].clone();
    for (index, codes) in seen.iter().enumerate() {
        assert_eq!(
            codes, &first,
            "case region #{index}: every Region publishes the same quotas in the same order, got \
             {codes:?}"
        );
    }
}
