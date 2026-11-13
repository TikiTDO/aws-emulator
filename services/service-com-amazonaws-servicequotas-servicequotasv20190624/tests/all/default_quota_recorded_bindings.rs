// SPDX-License-Identifier: CC0-1.0

//! Which quota the recorded default-quota listing of a service binds a `QuotaCode` to,
//! over the wire.
//!
//! `QuotaCode` is the identity Service Quotas publishes a quota under: a client reads a
//! code out of AWS's catalog and then sizes workloads, pre-flights capacity and asks for
//! increases against the row published under it. So the row a code carries is the quota
//! AWS publishes under THAT code — its `QuotaName`, `Value`, `Unit`, `Adjustable`,
//! `Period`, `UsageMetric` and `Description` — and the server does not re-author the
//! text of either string member.
//!
//! The two rules asserted here, each over a TABLE spanning ten `ServiceCode`s rather
//! than one recorded row:
//!
//! * every pinned code reports the quota published under it, through the listing, through
//!   `GetAWSDefaultServiceQuota` and through `GetServiceQuota` alike — and `Unit` and
//!   `Value` are one pair, so a size published in gigabytes is never reported as the same
//!   magnitude in megabytes;
//! * across the WHOLE catalog of each of those services, a code resolves exactly one row,
//!   that row is identical however it is read, and it carries a `Description`.
//!
//! The rows differ along everything the rules generalise over: a per-API request rate
//! metered by that API's `AWS/Usage` call count (`events`, `firehose`), a resource count
//! metered by a resource count under a DIFFERENT `AWS/Usage` display name (`firehose`), a
//! rate the service meters in its own metric namespace with no dimensions (`firehose`), a
//! counted plane with no window at all (`fms`, `fsx`, `gamelift`, `guardduty`,
//! `imagebuilder`), a size in gigabytes (`glacier`), a duration in minutes
//! (`groundstation`), descriptions in the service's own terse form (`glue`), adjustable
//! and not-adjustable rows, and values from 1 to 1,048,576.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas as defaults, emulator};
use crate::recorded_rows_events_through_imagebuilder::PUBLISHED_WITHOUT_A_DESCRIPTION;

/// The `UsageMetric` a published row carries, as the recorded listing reports it.
enum Metered {
    /// The `AWS/Usage` `CallCount` of one API, under the service's `AWS/Usage` display
    /// name: what a per-API request-rate row is metered by.
    CallCount {
        usage_service: &'static str,
        api: &'static str,
    },
    /// The `AWS/Usage` `ResourceCount` of one resource series, under the service's
    /// `AWS/Usage` display name: what a resource plane is metered by.
    ResourceCount {
        usage_service: &'static str,
        resource: &'static str,
    },
    /// A metric the service publishes in its OWN namespace, which needs no dimension to
    /// name the series it measures.
    ServiceNamespace {
        namespace: &'static str,
        metric_name: &'static str,
    },
}

impl Metered {
    /// The `MetricInfo` JSON this metric is published as.
    fn published(&self) -> Value {
        match self {
            Self::CallCount { usage_service, api } => json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "CallCount",
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {
                    "Service": usage_service,
                    "Resource": api,
                    "Type": "API",
                    "Class": "None",
                },
            }),
            Self::ResourceCount {
                usage_service,
                resource,
            } => json!({
                "MetricNamespace": "AWS/Usage",
                "MetricName": "ResourceCount",
                "MetricStatisticRecommendation": "Maximum",
                "MetricDimensions": {
                    "Service": usage_service,
                    "Resource": resource,
                    "Type": "Resource",
                    "Class": "None",
                },
            }),
            Self::ServiceNamespace {
                namespace,
                metric_name,
            } => json!({
                "MetricNamespace": namespace,
                "MetricName": metric_name,
                "MetricStatisticRecommendation": "Sum",
                "MetricDimensions": {},
            }),
        }
    }
}

/// One published default-quota row, as the recorded default-quota listing of its
/// `ServiceCode` reports it.
struct Published {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The published description, where the evidence states its text; `None` pins none
    /// (a wrong assertion would be worse than no assertion), and the
    /// every-row-is-described rule below still covers it.
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` of a quota published over a window.
    period: Option<(i32, &'static str)>,
    /// The usage metric, for a quota usage is published for.
    metric: Option<Metered>,
}

/// The `ServiceCode`s whose whole catalogs the second rule walks.
const COVERED_SERVICES: &[&str] = &[
    "events",
    "firehose",
    "fms",
    "fsx",
    "gamelift",
    "glacier",
    "glue",
    "groundstation",
    "guardduty",
    "imagebuilder",
];

/// The rows this suite pins.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each of these ten
/// service codes, which states for each code the quota published under it — its name,
/// default value, unit, adjustability, window, usage metric and, where quoted, its
/// description text.
const PUBLISHED_ROWS: &[Published] = &[
    // events — the endpoint-API rate and the account's invocation rate, two rates whose
    // codes this catalog had bound to another API's rate and to a resource plane.
    Published {
        service_code: "events",
        quota_code: "L-CB72B930",
        quota_name: "CreateEndpoint throttle limit in transactions per second",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: Some((1, "SECOND")),
        metric: Some(Metered::CallCount {
            usage_service: "EventBridge",
            api: "CreateEndpoint",
        }),
    },
    Published {
        service_code: "events",
        quota_code: "L-5540C5E3",
        quota_name: "Invocations throttle limit in transactions per second",
        value: 18_750.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: Some((1, "SECOND")),
        metric: Some(Metered::CallCount {
            usage_service: "EventBridge",
            api: "Invocations",
        }),
    },
    // A target plane, published with AWS's own sentence rather than a paraphrase of it.
    Published {
        service_code: "events",
        quota_code: "L-388D1D08",
        quota_name: "Targets per rule",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some("Maximum number of targets that can be associated with a rule"),
        period: None,
        metric: None,
    },
    Published {
        service_code: "events",
        quota_code: "L-DF788473",
        quota_name: "[EventsV2] Event Buses",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of V2 Event Buses owned by this account per Region. Does \
             not include buses shared with this account.",
        ),
        period: Some((5, "MINUTE")),
        metric: Some(Metered::ResourceCount {
            usage_service: "EventBridge",
            resource: "EventsV2/EventBus",
        }),
    },
    // firehose — the delivery-stream plane, whose resource count is published under the
    // service's SHORT AWS/Usage display name while its API call counts are not.
    Published {
        service_code: "firehose",
        quota_code: "L-14BB0BE7",
        quota_name: "Delivery streams",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of delivery streams you can create in this account in the \
             current Region.",
        ),
        period: Some((5, "MINUTE")),
        metric: Some(Metered::ResourceCount {
            usage_service: "Firehose",
            resource: "DeliveryStreams",
        }),
    },
    Published {
        service_code: "firehose",
        quota_code: "L-47C73A38",
        quota_name: "Rate of UntagDeliveryStream requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of UntagDeliveryStream requests you can make per second in \
             this account in the current Region.",
        ),
        period: Some((1, "SECOND")),
        metric: Some(Metered::CallCount {
            usage_service: "Kinesis Firehose",
            api: "UntagDeliveryStream",
        }),
    },
    Published {
        service_code: "firehose",
        quota_code: "L-36A7E04C",
        quota_name: "Rate of ListTagsForDeliveryStream requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of ListTagsForDeliveryStream requests you can make per \
             second in this account in the current Region.",
        ),
        period: Some((1, "SECOND")),
        metric: Some(Metered::CallCount {
            usage_service: "Kinesis Firehose",
            api: "ListTagsForDeliveryStream",
        }),
    },
    // A rate the service meters in its OWN namespace: no dimensions name the series.
    Published {
        service_code: "firehose",
        quota_code: "L-FDED3C5B",
        quota_name: "Rate of ListDeliveryStream requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: Some((1, "SECOND")),
        metric: Some(Metered::ServiceNamespace {
            namespace: "AWS/Firehose",
            metric_name: "ListDeliveryStreams.Requests",
        }),
    },
    // fms — counted planes under codes this catalog had bound to control-plane rates, so
    // they carry neither a window nor a call count.
    Published {
        service_code: "fms",
        quota_code: "L-49265FF2",
        quota_name: "Custom managed application lists in any content audit security group \
                     policy setting",
        value: 1.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "fms",
        quota_code: "L-3F3557EB",
        quota_name: "Resources per resource set",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "fms",
        quota_code: "L-742188BD",
        quota_name: "Protocols per protocol list",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of protocols that you can define in a protocol list.",
        ),
        period: None,
        metric: None,
    },
    // fsx — a COUNT of file systems under a code this catalog had bound to a throughput
    // capacity, so its unit is `None` and not a throughput unit.
    Published {
        service_code: "fsx",
        quota_code: "L-C48231E5",
        quota_name: "Lustre Scratch file systems",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "fsx",
        quota_code: "L-E43BDB2E",
        quota_name: "Windows SSD storage capacity",
        value: 524_288.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum amount of SSD storage capacity (in GiB) for all Amazon FSx for \
             Windows File Server file systems that you can have in this account.",
        ),
        period: None,
        metric: None,
    },
    Published {
        service_code: "fsx",
        quota_code: "L-E2C89679",
        quota_name: "ONTAP SSD storage capacity",
        value: 1_048_576.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum amount of SSD storage capacity (in GiB) for all Amazon FSx for \
             NetApp ONTAP file systems that you can have in this account.",
        ),
        period: None,
        metric: None,
    },
    // gamelift — queue, matchmaking and player-session planes under codes this catalog
    // had bound to game-session API rates.
    Published {
        service_code: "gamelift",
        quota_code: "L-BB62CF1D",
        quota_name: "Queue destinations per game session queue",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "gamelift",
        quota_code: "L-8AE49BBD",
        quota_name: "Matchmaking rule sets per region",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "gamelift",
        quota_code: "L-EC03D793",
        quota_name: "Player sessions per game session",
        value: 200.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    // glacier — the SIZE planes, where the unit and the value are one pair: the part size
    // is four gigabytes, not four thousand and ninety-six megabytes.
    Published {
        service_code: "glacier",
        quota_code: "L-4BF4C362",
        quota_name: "Multipart parts size.",
        value: 4.0,
        unit: "Gigabytes",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "glacier",
        quota_code: "L-E96017CA",
        quota_name: "Archive size in GB.",
        value: 40_000.0,
        unit: "Gigabytes",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "glacier",
        quota_code: "L-F127DF29",
        quota_name: "Provisioned capacity units",
        value: 2.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    // glue — the Data Catalog planes, whose descriptions the service publishes in its own
    // terse `Catalog: …` form, and two rates it publishes as NOT adjustable.
    Published {
        service_code: "glue",
        quota_code: "L-4F4B522D",
        quota_name: "Global s3 path max length",
        value: 2000.0,
        unit: "None",
        adjustable: true,
        description: Some("Catalog: global s3 path max length"),
        period: None,
        metric: None,
    },
    Published {
        service_code: "glue",
        quota_code: "L-D90AC94B",
        quota_name: "Global maximum administrators",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some("Catalog: global maximum administrators"),
        period: None,
        metric: None,
    },
    Published {
        service_code: "glue",
        quota_code: "L-E6C807E7",
        quota_name: "Concurrent compute capacity for Ray worker instances in M-DPUs per \
                     account.",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "glue",
        quota_code: "L-CB69DFA0",
        quota_name: "Rate of GetJobRun requests",
        value: 100.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: Some((1, "SECOND")),
        metric: Some(Metered::CallCount {
            usage_service: "Glue",
            api: "GetJobRun",
        }),
    },
    // groundstation — a contact DURATION in minutes and the mission-profile plane, under
    // codes this catalog had bound to per-config-type counters.
    Published {
        service_code: "groundstation",
        quota_code: "L-CCFDE387",
        quota_name: "Maximum Contact Duration",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "groundstation",
        quota_code: "L-5342B9BF",
        quota_name: "Mission profile limit",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    // guardduty — the trusted-IP-set and threat-intel-set planes, each under its own code,
    // and the retention window AWS publishes under its full name.
    Published {
        service_code: "guardduty",
        quota_code: "L-2C0E14B9",
        quota_name: "Threat intel sets",
        value: 6.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "guardduty",
        quota_code: "L-AFBA2260",
        quota_name: "Trusted IP sets",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "guardduty",
        quota_code: "L-373487E5",
        quota_name: "Finding retention period",
        value: 90.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of days a finding is retained. After 90 days findings are \
             deleted.",
        ),
        period: None,
        metric: None,
    },
    // imagebuilder — the recipe, component-parameter and build-concurrency planes, under
    // codes this catalog had bound to image-API rates.
    Published {
        service_code: "imagebuilder",
        quota_code: "L-1DF98342",
        quota_name: "Image recipes",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "imagebuilder",
        quota_code: "L-A58CFBED",
        quota_name: "Parameters per component",
        value: 25.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Published {
        service_code: "imagebuilder",
        quota_code: "L-BA4D191B",
        quota_name: "Concurrent builds",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of concurrent builds that can be in progress in this \
             account in the current Region.",
        ),
        period: None,
        metric: None,
    },
];

/// Every pinned code reports the quota published under it — name, value, unit,
/// adjustability, window, usage metric and, where the evidence states it, description —
/// through the listing, through the single default read and through the applied read.
#[tokio::test]
async fn a_published_row_reports_the_quota_bound_to_its_code() {
    let engine = emulator();
    let client = client(&engine, "us-east-2");

    let mut listings: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for row in PUBLISHED_ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = client.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            (
                quota["QuotaName"].clone(),
                quota["Value"].clone(),
                quota["Unit"].clone(),
                quota["Adjustable"].clone(),
            ),
            (
                json!(row.quota_name),
                json!(row.value),
                json!(row.unit),
                json!(row.adjustable),
            ),
            "case {case}: expected the quota AWS publishes under this code, got {quota}"
        );
        if let Some(description) = row.description {
            assert_eq!(
                quota["Description"],
                json!(description),
                "case {case}: expected the published description, got {quota}"
            );
        }
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window this quota is published over, got {quota}"
            ),
            None => assert!(
                quota.get("Period").is_none_or(Value::is_null),
                "case {case}: this quota is published over no window, got {quota}"
            ),
        }
        match &row.metric {
            Some(metric) => assert_eq!(
                quota["UsageMetric"],
                metric.published(),
                "case {case}: expected the usage metric published for this quota, got {quota}"
            ),
            None => assert!(
                quota.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: this quota publishes no usage metric, got {quota}"
            ),
        }

        let listed = match listings.get(row.service_code) {
            Some(listed) => listed,
            None => {
                let listed = defaults(&client, row.service_code).await;
                listings.entry(row.service_code).or_insert(listed)
            }
        };
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );

        // The applied plane is keyed on the same code, so a client that read the code out
        // of the default catalog resolves the same quota's value in force.
        let applied = client.ok("GetServiceQuota", pair).await["Quota"].clone();
        assert_eq!(
            (applied["QuotaName"].clone(), applied["Value"].clone()),
            (json!(row.quota_name), json!(row.value)),
            "case {case}: the applied read must resolve the same quota, got {applied}"
        );
    }
}

/// Over the WHOLE catalog of each covered service: a code resolves exactly one row, the
/// row the listing reports is the row the single read returns, and every row carries a
/// `Description` except the rows no source states a sentence for
/// ([`PUBLISHED_WITHOUT_A_DESCRIPTION`]) — so no code can carry two quotas' facts, none
/// carries a sentence of this catalog's own wording, and none carries another row's.
#[tokio::test]
async fn a_code_resolves_one_described_row_of_its_service() {
    let engine = emulator();
    let client = client(&engine, "eu-central-1");

    let mut rows = 0_usize;
    for service_code in COVERED_SERVICES {
        let listed = defaults(&client, service_code).await;
        assert!(
            !listed.is_empty(),
            "case {service_code}: the service must publish its catalog, got {listed:?}"
        );

        let mut codes: BTreeSet<String> = BTreeSet::new();
        for quota in &listed {
            let code = quota["QuotaCode"].as_str().unwrap_or_default().to_string();
            let case = format!("{service_code}/{code}");
            assert!(
                codes.insert(code.clone()),
                "case {case}: a QuotaCode identifies one quota of a service, got a second \
                 row under it in {listed:?}"
            );
            let description = quota["Description"].as_str().unwrap_or_default();
            let stated =
                !PUBLISHED_WITHOUT_A_DESCRIPTION.contains(&(*service_code, code.as_str()));
            assert_eq!(
                !description.is_empty(),
                stated,
                "case {case}: every published row carries its published description, and a \
                 row no source states one for carries none, got {quota}"
            );
            let read = client
                .ok(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": service_code, "QuotaCode": code}),
                )
                .await["Quota"]
                .clone();
            assert_eq!(
                &read, quota,
                "case {case}: the single read must return the row the listing reported, got \
                 {read}"
            );
            rows += 1;
        }
    }

    assert!(
        rows > PUBLISHED_ROWS.len(),
        "case catalog: the rule must be asserted over the whole catalog of each covered \
         service, got {rows} rows"
    );
}
