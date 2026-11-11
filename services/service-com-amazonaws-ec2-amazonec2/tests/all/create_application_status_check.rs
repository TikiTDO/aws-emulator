// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateApplicationStatusCheck`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=CreateApplicationStatusCheck`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation CREATES one application status check from the
//! configuration the request names, filling every member the request leaves out with the
//! value AWS documents as its default ("Interval is 60 seconds, Timeout is 6 seconds,
//! FailureThreshold is 2, SuccessThreshold is 2, StatusCodeMatcher is 200,
//! InitializationGracePeriodSeconds is 300 seconds", `Aggregation` "defaults to included",
//! `Path` "Default: /"), stores it, and answers with the check a later
//! `DescribeApplicationStatusChecks` reports — refusing, before anything is stored, a request
//! outside the bounds the operation publishes.
//!
//! So the table below spans that class rather than one recorded request: each row names a
//! different SUBSET of the members (one naming only the two required ones, one naming every
//! one of them) and a different value for each — the published bounds of `PortNumber`, of
//! `Timeout` and of `InitializationGracePeriodSeconds`, both aggregation settings, both
//! protocols, both IP versions, single-code and range status code matchers, zero, one and two
//! health check paths with one and two destinations each, and zero, one and two tags. The
//! enum-valued members are driven by the model's own catalogs
//! (`AGGREGATION_STATUS_ENUM_VALUES`, `NETWORK_PROTOCOL_ENUM_VALUES`,
//! `IP_VERSION_ENUM_VALUES`, `IP_SCOPE_ENUM_VALUES`) rather than by a value this emulator
//! happens to produce, and the refusal table below drives each published bound at the bound
//! and one step past it.

use aws_smithy_types::DateTime;
use ec2::ops::types::{
    ApplicationStatusCheckResponseObject, AGGREGATION_STATUS_ENUM_VALUES, IP_SCOPE_ENUM_VALUES,
    IP_VERSION_ENUM_VALUES, NETWORK_PROTOCOL_ENUM_VALUES,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, APPLICATION_STATUS_CHECK_ID_PREFIX,
    APPLICATION_STATUS_CHECK_INTERVAL_SECONDS, APPLICATION_STATUS_CHECK_TIMEOUT_RANGE,
    MAX_APPLICATION_STATUS_CHECKS, MAX_HEALTH_CHECK_PATHS, MAX_STATUS_CODE_MATCHER_CHARS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query;
// The XML tree readers every suite of this crate shares — "the child element named X", its
// text, and the entries of the set named X — rather than another copy of them here.
use super::wire::{child, child_text, is_long_id, set_items};

/// The Region every call here is signed for. A check's configuration is a fact of the check
/// rather than of the Region, so one Region is used throughout — and it is a Region of the
/// catalog, so the default network whose subnets the health check path rows name is the one an
/// account already has there.
const REGION: &str = "us-west-2";

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `CreateApplicationStatusCheck` call carrying `params`, as its HTTP status and body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    ec2_query::call(router, REGION, "CreateApplicationStatusCheck", params).await
}

// --- reading the response ----------------------------------------------------

/// One destination of a reported health check path: `(its subnet id, its security group id)`.
type ReportedDestination = (String, Option<String>);

/// One reported health check path: `(the source's subnet id, the source's security group id,
/// its destinations)`.
type ReportedPath = (Option<String>, Option<String>, Vec<ReportedDestination>);

/// The configuration members of one reported `ApplicationStatusCheckResponseObject`: what a
/// create takes from the request (or fills in from a documented default), and nothing the
/// service decides for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Configuration {
    aggregation: Option<String>,
    protocol: Option<String>,
    port: Option<String>,
    path: Option<String>,
    device_index: Option<String>,
    ip_version: Option<String>,
    ip_scope: Option<String>,
    interval: Option<String>,
    timeout: Option<String>,
    failure_threshold: Option<String>,
    success_threshold: Option<String>,
    status_code_matcher: Option<String>,
    grace: Option<String>,
    /// One entry per health check path the check carries.
    paths: Vec<ReportedPath>,
    tags: Vec<(String, String)>,
}

/// One reported check: its id, its configuration, and the members the service itself decides.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: String,
    configuration: Configuration,
    target_tags: Vec<(String, String)>,
    last_updated: Option<String>,
    creation: Option<String>,
    modify: Option<String>,
    deletion: Option<String>,
}

/// The `(key, value)` pairs of one `key`/`value` item set.
fn pairs(node: roxmltree::Node<'_, '_>, set: &str) -> Vec<(String, String)> {
    set_items(node, set)
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// One `<applicationStatusCheck>` element, read out of the XML.
fn row_of(node: roxmltree::Node<'_, '_>) -> Row {
    Row {
        id: child_text(node, "applicationStatusCheckId").unwrap_or_default(),
        configuration: Configuration {
            aggregation: child_text(node, "aggregation"),
            protocol: child_text(node, "protocol"),
            port: child_text(node, "port"),
            path: child_text(node, "path"),
            device_index: child_text(node, "deviceIndex"),
            ip_version: child_text(node, "ipVersion"),
            ip_scope: child_text(node, "ipScope"),
            interval: child_text(node, "interval"),
            timeout: child_text(node, "timeout"),
            failure_threshold: child_text(node, "failureThreshold"),
            success_threshold: child_text(node, "successThreshold"),
            status_code_matcher: child_text(node, "statusCodeMatcher"),
            grace: child_text(node, "initializationGracePeriodSeconds"),
            paths: set_items(node, "healthCheckPathSet")
                .into_iter()
                .map(|path| {
                    let source = child(path, "source");
                    (
                        source.and_then(|source| child_text(source, "subnetId")),
                        source.and_then(|source| child_text(source, "securityGroupId")),
                        set_items(path, "destinationSet")
                            .into_iter()
                            .map(|destination| {
                                (
                                    child_text(destination, "subnetId").unwrap_or_default(),
                                    child_text(destination, "securityGroupId"),
                                )
                            })
                            .collect(),
                    )
                })
                .collect(),
            tags: pairs(node, "tagSet"),
        },
        target_tags: pairs(node, "targetTagAssociationSet"),
        last_updated: child_text(node, "lastUpdatedAt"),
        creation: child_text(node, "creationTime"),
        modify: child_text(node, "modifyTime"),
        deletion: child_text(node, "deletionTime"),
    }
}

/// The one check a `CreateApplicationStatusCheck` response carries.
fn created(case: &str, body: &str) -> Row {
    let document = super::wire::document(case, body);
    let check = child(document.root_element(), "applicationStatusCheck").unwrap_or_else(|| {
        panic!("case {case:?}: no <applicationStatusCheck> in {body}");
    });
    row_of(check)
}

/// The checks `DescribeApplicationStatusChecks` reports, narrowed by `params`.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<Row> {
    let (status, body) =
        ec2_query::call(router, REGION, "DescribeApplicationStatusChecks", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeApplicationStatusChecks must succeed, got {status}: {body}"
    );
    let document = super::wire::document(case, &body);
    set_items(document.root_element(), "applicationStatusCheckSet")
        .into_iter()
        .map(row_of)
        .collect()
}

/// The one check `DescribeApplicationStatusChecks` reports for `id`.
async fn described_one(router: &MultiServiceRouter, case: &str, id: &str) -> Row {
    let rows = described(router, case, &[p("ApplicationStatusCheckId.1", id)]).await;
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: the describe must report exactly {id}, got {rows:?}"
    );
    rows.into_iter().next().unwrap_or_else(|| unreachable!())
}

/// Every check the account holds, by id, in the order the describe reports them.
async fn stored_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    described(router, case, &[])
        .await
        .into_iter()
        .map(|row| row.id)
        .collect()
}

// --- the ambient network the health check path rows name ---------------------

/// Two subnet ids of the Region's default VPC, read back through `DescribeSubnets` — real ids
/// of the ambient network an account already has, rather than ids written into this file.
async fn subnet_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = ec2_query::call(router, REGION, "DescribeSubnets", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeSubnets must succeed, got {status}: {body}"
    );
    let ids = ec2_query::elements(&body, "subnetId");
    assert!(
        ids.len() >= 2,
        "the Region's default VPC must hold at least two subnets, got {ids:?}"
    );
    ids.into_iter().take(2).collect()
}

/// A security group id of the Region's default VPC, read back through
/// `DescribeSecurityGroups`.
async fn security_group_id(router: &MultiServiceRouter) -> String {
    let (status, body) = ec2_query::call(router, REGION, "DescribeSecurityGroups", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeSecurityGroups must succeed, got {status}: {body}"
    );
    ec2_query::elements(&body, "groupId")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the Region's default VPC must hold a security group: {body}"))
}

// --- the cases ---------------------------------------------------------------

/// One row of the table: the request a caller makes, and the configuration the check it
/// creates must then hold — which is also the configuration the describe must report.
struct Case {
    label: &'static str,
    /// The members the request names, beyond the two the model makes required.
    extra: Vec<(String, String)>,
    protocol: &'static str,
    port: i32,
    expected: Configuration,
}

impl Case {
    /// The whole request: the two required members and whatever else the row names.
    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            p("Protocol", self.protocol),
            p("Port", self.port.to_string()),
        ];
        params.extend(self.extra.iter().cloned());
        params
    }
}

/// `Some(value.to_string())`, for a member the response must report.
fn text(value: &str) -> Option<String> {
    Some(value.to_string())
}

/// The form fields of one health check path at `index` (counting from 1): its source and its
/// destinations, as ec2Query flattens the `HealthCheckPaths` list and the `Destinations` list
/// inside it.
fn path_fields(
    index: usize,
    source: &str,
    source_group: Option<&str>,
    destinations: &[(&str, Option<&str>)],
) -> Vec<(String, String)> {
    let mut fields = vec![p(&format!("HealthCheckPath.{index}.Source.SubnetId"), source)];
    if let Some(group) = source_group {
        fields.push(p(
            &format!("HealthCheckPath.{index}.Source.SecurityGroupId"),
            group,
        ));
    }
    for (nth, (subnet, group)) in destinations.iter().enumerate() {
        let position = nth + 1;
        fields.push(p(
            &format!("HealthCheckPath.{index}.Destination.{position}.SubnetId"),
            *subnet,
        ));
        if let Some(group) = group {
            fields.push(p(
                &format!("HealthCheckPath.{index}.Destination.{position}.SecurityGroupId"),
                *group,
            ));
        }
    }
    fields
}

/// The form fields of a `TagSpecifications` entry naming the check this call creates.
fn tag_fields(resource_type: &str, tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut fields = vec![p("TagSpecification.1.ResourceType", resource_type)];
    for (nth, (key, value)) in tags.iter().enumerate() {
        let position = nth + 1;
        fields.push(p(&format!("TagSpecification.1.Tag.{position}.Key"), *key));
        fields.push(p(&format!("TagSpecification.1.Tag.{position}.Value"), *value));
    }
    fields
}

/// `(key, value)` pairs as the response reports them.
fn owned_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The table. Each row names a different subset of the members and different values for them;
/// between them the rows cover every member of all four modelled catalogs, both published
/// bounds of `PortNumber`, of `Timeout` and of `InitializationGracePeriodSeconds`, and zero,
/// one and two health check paths, destinations and tags.
fn cases(subnets: &[String], group: &str) -> Vec<Case> {
    let (timeout_min, timeout_max) = APPLICATION_STATUS_CHECK_TIMEOUT_RANGE;
    vec![
        Case {
            label: "only the required members, so every documented default",
            extra: Vec::new(),
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[0],
            // The published minimum of `PortNumber` (`range 1..65535`).
            port: 1,
            expected: Configuration {
                // "If you do not specify a value, it defaults to included."
                aggregation: text("included"),
                protocol: text(NETWORK_PROTOCOL_ENUM_VALUES[0]),
                port: text("1"),
                // "Default: /".
                path: text("/"),
                // The device every instance has, and the version and scope of the address a
                // health check reaches it on: the documented defaults for the three members
                // the request left out.
                device_index: text("0"),
                ip_version: text(IP_VERSION_ENUM_VALUES[0]),
                ip_scope: text(IP_SCOPE_ENUM_VALUES[0]),
                // "Interval is 60 seconds, Timeout is 6 seconds, FailureThreshold is 2,
                // SuccessThreshold is 2, StatusCodeMatcher is 200,
                // InitializationGracePeriodSeconds is 300 seconds."
                interval: text("60"),
                timeout: text("6"),
                failure_threshold: text("2"),
                success_threshold: text("2"),
                status_code_matcher: text("200"),
                grace: text("300"),
                paths: Vec::new(),
                tags: Vec::new(),
            },
        },
        Case {
            label: "every member named: https on the published maximum port, two tags",
            extra: {
                let mut extra = vec![
                    p("Aggregation", AGGREGATION_STATUS_ENUM_VALUES[1]),
                    p("Path", "/healthz"),
                    p("DeviceIndex", "3"),
                    p("IpVersion", IP_VERSION_ENUM_VALUES[1]),
                    p("IpScope", IP_SCOPE_ENUM_VALUES[0]),
                    p("Interval", APPLICATION_STATUS_CHECK_INTERVAL_SECONDS.to_string()),
                    // The published maximum of `Timeout`.
                    p("Timeout", timeout_max.to_string()),
                    p("FailureThreshold", "10"),
                    p("SuccessThreshold", "5"),
                    p("StatusCodeMatcher", "200-299,301"),
                    // The published maximum of `InitializationGracePeriodSeconds`.
                    p("InitializationGracePeriodSeconds", "600"),
                ];
                extra.extend(tag_fields(
                    "application-status-check",
                    &[("Name", "checkout"), ("Tier", "web")],
                ));
                extra
            },
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[1],
            // The published maximum of `PortNumber`.
            port: 65_535,
            expected: Configuration {
                aggregation: text(AGGREGATION_STATUS_ENUM_VALUES[1]),
                protocol: text(NETWORK_PROTOCOL_ENUM_VALUES[1]),
                port: text("65535"),
                path: text("/healthz"),
                device_index: text("3"),
                ip_version: text(IP_VERSION_ENUM_VALUES[1]),
                ip_scope: text(IP_SCOPE_ENUM_VALUES[0]),
                interval: text("60"),
                timeout: text(&timeout_max.to_string()),
                failure_threshold: text("10"),
                success_threshold: text("5"),
                status_code_matcher: text("200-299,301"),
                grace: text("600"),
                paths: Vec::new(),
                tags: owned_pairs(&[("Name", "checkout"), ("Tier", "web")]),
            },
        },
        Case {
            label: "the published minimum timeout and grace period, one tag, device index 0",
            extra: {
                let mut extra = vec![
                    p("Aggregation", AGGREGATION_STATUS_ENUM_VALUES[0]),
                    p("Path", "/status"),
                    // "The value must be greater than or equal to 0" (`DeviceIndex`).
                    p("DeviceIndex", "0"),
                    p("IpVersion", IP_VERSION_ENUM_VALUES[0]),
                    // The published minimum of `Timeout`.
                    p("Timeout", timeout_min.to_string()),
                    p("FailureThreshold", "1"),
                    p("SuccessThreshold", "1"),
                    p("StatusCodeMatcher", "204"),
                    // The published minimum of `InitializationGracePeriodSeconds`.
                    p("InitializationGracePeriodSeconds", "-1"),
                ];
                extra.extend(tag_fields("application-status-check", &[("Name", "api")]));
                extra
            },
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[0],
            port: 8_080,
            expected: Configuration {
                aggregation: text(AGGREGATION_STATUS_ENUM_VALUES[0]),
                protocol: text(NETWORK_PROTOCOL_ENUM_VALUES[0]),
                port: text("8080"),
                path: text("/status"),
                device_index: text("0"),
                ip_version: text(IP_VERSION_ENUM_VALUES[0]),
                ip_scope: text(IP_SCOPE_ENUM_VALUES[0]),
                interval: text("60"),
                timeout: text(&timeout_min.to_string()),
                failure_threshold: text("1"),
                success_threshold: text("1"),
                status_code_matcher: text("204"),
                grace: text("-1"),
                paths: Vec::new(),
                tags: owned_pairs(&[("Name", "api")]),
            },
        },
        Case {
            label: "one health check path with one destination",
            extra: path_fields(1, &subnets[0], None, &[(&subnets[1], None)]),
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[1],
            port: 443,
            expected: Configuration {
                aggregation: text("included"),
                protocol: text(NETWORK_PROTOCOL_ENUM_VALUES[1]),
                port: text("443"),
                path: text("/"),
                device_index: text("0"),
                ip_version: text(IP_VERSION_ENUM_VALUES[0]),
                ip_scope: text(IP_SCOPE_ENUM_VALUES[0]),
                interval: text("60"),
                timeout: text("6"),
                failure_threshold: text("2"),
                success_threshold: text("2"),
                status_code_matcher: text("200"),
                grace: text("300"),
                paths: vec![(
                    Some(subnets[0].clone()),
                    None,
                    vec![(subnets[1].clone(), None)],
                )],
                tags: Vec::new(),
            },
        },
        Case {
            label: "two health check paths, the second with two destinations and groups",
            extra: {
                let mut extra = path_fields(1, &subnets[1], Some(group), &[(&subnets[0], None)]);
                extra.extend(path_fields(
                    2,
                    &subnets[0],
                    None,
                    &[(&subnets[0], Some(group)), (&subnets[1], None)],
                ));
                extra.push(p("StatusCodeMatcher", "200,202,300-399"));
                extra
            },
            protocol: NETWORK_PROTOCOL_ENUM_VALUES[0],
            port: 80,
            expected: Configuration {
                aggregation: text("included"),
                protocol: text(NETWORK_PROTOCOL_ENUM_VALUES[0]),
                port: text("80"),
                path: text("/"),
                device_index: text("0"),
                ip_version: text(IP_VERSION_ENUM_VALUES[0]),
                ip_scope: text(IP_SCOPE_ENUM_VALUES[0]),
                interval: text("60"),
                timeout: text("6"),
                failure_threshold: text("2"),
                success_threshold: text("2"),
                status_code_matcher: text("200,202,300-399"),
                grace: text("300"),
                paths: vec![
                    (
                        Some(subnets[1].clone()),
                        Some(group.to_string()),
                        vec![(subnets[0].clone(), None)],
                    ),
                    (
                        Some(subnets[0].clone()),
                        None,
                        vec![
                            (subnets[0].clone(), Some(group.to_string())),
                            (subnets[1].clone(), None),
                        ],
                    ),
                ],
                tags: Vec::new(),
            },
        },
    ]
}

// --- the tests ---------------------------------------------------------------

/// Every row of the table: the create answers the configuration it was asked for with every
/// documented default filled in, stores it under a freshly minted id, and the describe reports
/// that same check back unchanged.
#[tokio::test]
async fn create_stores_the_configuration_it_reports() {
    let (router, _engine) = ec2_query::emulator();
    let subnets = subnet_ids(&router).await;
    let group = security_group_id(&router).await;
    let mut minted: Vec<String> = Vec::new();

    for case in cases(&subnets, &group) {
        let label = case.label;
        let (status, body) = create(&router, &case.params()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with body {body}"
        );
        let row = created(label, &body);

        assert!(
            is_long_id(APPLICATION_STATUS_CHECK_ID_PREFIX, &row.id),
            "case {label:?}: expected an {APPLICATION_STATUS_CHECK_ID_PREFIX}- id, \
             got {:?}",
            row.id
        );
        assert!(
            !minted.contains(&row.id),
            "case {label:?}: expected an id no earlier create was given, got {:?} \
             among {minted:?}",
            row.id
        );
        assert_eq!(
            row.configuration, case.expected,
            "case {label:?}: expected the configuration the request named with every \
             documented default filled in, got {:?}",
            row.configuration
        );

        // "The date and time when the application status check was created": this call is
        // that event, so the instant is there and is an instant.
        let creation = row
            .creation
            .clone()
            .unwrap_or_else(|| panic!("case {label:?}: no creationTime in {body}"));
        assert!(
            ec2_query::as_timestamp(&creation).is_some(),
            "case {label:?}: expected creationTime to be an ISO 8601 instant, got {creation:?}"
        );
        // Nothing has modified the check, nothing has deleted it, and no health check has run
        // for it — "You must associate the check with instances or tags using
        // AssociateApplicationStatusCheck before health checks start" — so none of those
        // three instants is reported, and it is monitoring no tags.
        assert_eq!(
            row.modify, None,
            "case {label:?}: a check nothing has modified must report no modifyTime, \
             got {:?}",
            row.modify
        );
        assert_eq!(
            row.deletion, None,
            "case {label:?}: a check nothing has deleted must report no deletionTime, \
             got {:?}",
            row.deletion
        );
        assert_eq!(
            row.last_updated, None,
            "case {label:?}: a check no health check has run for must report no \
             lastUpdatedAt, got {:?}",
            row.last_updated
        );
        assert!(
            row.target_tags.is_empty(),
            "case {label:?}: a check nobody has associated must report no target tag \
             associations, got {:?}",
            row.target_tags
        );

        // The round trip: what the create answered is what the store holds.
        let described = described_one(&router, label, &row.id).await;
        assert_eq!(
            described, row,
            "case {label:?}: the describe must report the created check unchanged, \
             got {described:?}"
        );
        minted.push(row.id);
    }

    let mut expected = minted.clone();
    expected.sort();
    assert_eq!(
        stored_ids(&router, "every row").await,
        expected,
        "the account must hold exactly one check per create, got {minted:?}"
    );
}

/// One refused request: what it names, and the refusal it must be answered with.
struct Refusal {
    label: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: &'static str,
    /// Text the message must carry — the value or the bound at fault, so a refusal cannot
    /// name the wrong one.
    names: String,
}

/// Every published bound, at the bound and one step past it, plus the structural requirements
/// on a health check path: each is refused with EC2's own code for it, and nothing is stored.
#[tokio::test]
async fn bad_requests_are_refused_and_store_nothing() {
    let (router, _engine) = ec2_query::emulator();
    let subnets = subnet_ids(&router).await;
    let (timeout_min, timeout_max) = APPLICATION_STATUS_CHECK_TIMEOUT_RANGE;
    let protocol = NETWORK_PROTOCOL_ENUM_VALUES[0];
    let required = |extra: Vec<(String, String)>| {
        let mut params = vec![p("Protocol", protocol), p("Port", "80")];
        params.extend(extra);
        params
    };
    let too_long_matcher = "2".repeat(MAX_STATUS_CODE_MATCHER_CHARS + 1);
    let unknown_subnet = "subnet-00000000000000fff";

    let refusals = vec![
        Refusal {
            label: "a path that does not start with a forward slash",
            params: required(vec![p("Path", "health")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "health".to_string(),
        },
        Refusal {
            label: "an interval below the one valid value",
            params: required(vec![p(
                "Interval",
                (APPLICATION_STATUS_CHECK_INTERVAL_SECONDS - 1).to_string(),
            )]),
            status: 400,
            code: "InvalidParameterValue",
            names: (APPLICATION_STATUS_CHECK_INTERVAL_SECONDS - 1).to_string(),
        },
        Refusal {
            label: "an interval above the one valid value",
            params: required(vec![p(
                "Interval",
                (APPLICATION_STATUS_CHECK_INTERVAL_SECONDS + 1).to_string(),
            )]),
            status: 400,
            code: "InvalidParameterValue",
            names: (APPLICATION_STATUS_CHECK_INTERVAL_SECONDS + 1).to_string(),
        },
        Refusal {
            label: "a timeout one below the published minimum",
            params: required(vec![p("Timeout", (timeout_min - 1).to_string())]),
            status: 400,
            code: "InvalidParameterValue",
            names: (timeout_min - 1).to_string(),
        },
        Refusal {
            label: "a timeout one above the published maximum",
            params: required(vec![p("Timeout", (timeout_max + 1).to_string())]),
            status: 400,
            code: "InvalidParameterValue",
            names: (timeout_max + 1).to_string(),
        },
        Refusal {
            label: "a failure threshold of zero",
            params: required(vec![p("FailureThreshold", "0")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "failureThreshold".to_string(),
        },
        Refusal {
            label: "a negative success threshold",
            params: required(vec![p("SuccessThreshold", "-2")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "successThreshold".to_string(),
        },
        Refusal {
            label: "a negative device index",
            params: required(vec![p("DeviceIndex", "-1")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "deviceIndex".to_string(),
        },
        Refusal {
            label: "a status code matcher that is not a status code at all",
            params: required(vec![p("StatusCodeMatcher", "healthy")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "healthy".to_string(),
        },
        Refusal {
            label: "a status code matcher naming fewer than three digits",
            params: required(vec![p("StatusCodeMatcher", "20")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "20".to_string(),
        },
        Refusal {
            label: "a range whose first value is not less than its second",
            params: required(vec![p("StatusCodeMatcher", "399-300")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "399-300".to_string(),
        },
        Refusal {
            label: "a range whose two values are equal",
            params: required(vec![p("StatusCodeMatcher", "300-300")]),
            status: 400,
            code: "InvalidParameterValue",
            names: "300-300".to_string(),
        },
        Refusal {
            label: "a status code matcher one character above the published maximum length",
            params: required(vec![p("StatusCodeMatcher", too_long_matcher.clone())]),
            status: 400,
            code: "InvalidParameterValue",
            names: MAX_STATUS_CODE_MATCHER_CHARS.to_string(),
        },
        Refusal {
            label: "a tag specification naming another resource type",
            params: required(tag_fields("instance", &[("Name", "nope")])),
            status: 400,
            code: "InvalidParameterValue",
            names: "instance".to_string(),
        },
        Refusal {
            label: "a health check path through a subnet the account does not have",
            params: required(path_fields(
                1,
                unknown_subnet,
                None,
                &[(&subnets[0], None)],
            )),
            status: 400,
            code: "InvalidSubnetID.NotFound",
            names: unknown_subnet.to_string(),
        },
        Refusal {
            label: "a health check path to a subnet the account does not have",
            params: required(path_fields(
                1,
                &subnets[0],
                None,
                &[(unknown_subnet, None)],
            )),
            status: 400,
            code: "InvalidSubnetID.NotFound",
            names: unknown_subnet.to_string(),
        },
        Refusal {
            label: "a health check path naming no destination",
            params: required(path_fields(1, &subnets[0], None, &[])),
            status: 400,
            code: "MissingParameter",
            names: subnets[0].clone(),
        },
        Refusal {
            label: "a health check path whose source names no subnet",
            params: required(vec![p(
                "HealthCheckPath.1.Destination.1.SubnetId",
                subnets[0].clone(),
            )]),
            status: 400,
            code: "MissingParameter",
            names: "source".to_string(),
        },
        Refusal {
            label: "one health check path above the maximum this emulator stores",
            params: required(
                (0..=MAX_HEALTH_CHECK_PATHS)
                    .flat_map(|index| {
                        path_fields(index + 1, &subnets[0], None, &[(&subnets[1], None)])
                    })
                    .collect(),
            ),
            status: 400,
            code: "InvalidParameterValue",
            names: MAX_HEALTH_CHECK_PATHS.to_string(),
        },
        Refusal {
            label: "a dry run of a request that would have succeeded",
            params: required(vec![p("DryRun", "true")]),
            status: 412,
            code: "DryRunOperation",
            names: "DryRun".to_string(),
        },
    ];

    for refusal in refusals {
        let label = refusal.label;
        let (status, body) = create(&router, &refusal.params).await;
        assert_eq!(
            status, refusal.status,
            "case {label:?}: expected HTTP {}, got {status} with body {body}",
            refusal.status
        );
        assert_eq!(
            ec2_query::error_code(&body),
            refusal.code,
            "case {label:?}: expected the code {:?}, got body {body}",
            refusal.code
        );
        let message = ec2_query::error_message(&body);
        assert!(
            message.contains(&refusal.names),
            "case {label:?}: expected the message to name {:?}, got {message:?}",
            refusal.names
        );
    }

    assert!(
        stored_ids(&router, "after every refusal")
            .await
            .is_empty(),
        "a refused create must store nothing, got {:?}",
        stored_ids(&router, "after every refusal").await
    );
}

/// "If you retry a request with the same token, the service ignores the request but does not
/// return an error": the same token with the same configuration reports the check the first
/// call created and creates no second one, and the same token with a different configuration
/// is the mismatch EC2 refuses.
#[tokio::test]
async fn a_client_token_is_idempotent_for_the_request_it_was_first_given_with() {
    let (router, _engine) = ec2_query::emulator();
    let protocol = NETWORK_PROTOCOL_ENUM_VALUES[1];
    let first = vec![
        p("Protocol", protocol),
        p("Port", "8443"),
        p("Path", "/ping"),
        p("ClientToken", "token-a"),
    ];

    let (status, body) = create(&router, &first).await;
    assert_eq!(status, 200, "the first create must succeed, got {status}: {body}");
    let created_once = created("the first create", &body);

    let (status, body) = create(&router, &first).await;
    assert_eq!(status, 200, "a replay must succeed, got {status}: {body}");
    let replayed = created("a replay of the same request", &body);
    assert_eq!(
        replayed, created_once,
        "a replay must report the check the first call created, got {replayed:?}"
    );
    assert_eq!(
        stored_ids(&router, "after the replay").await,
        vec![created_once.id.clone()],
        "a replay must create no second check"
    );

    let mut different = first.clone();
    different.retain(|(key, _)| key != "Port");
    different.push(p("Port", "8444"));
    let (status, body) = create(&router, &different).await;
    assert_eq!(
        status, 400,
        "the same token for another configuration must be refused, got {status}: {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "IdempotentParameterMismatch",
        "expected IdempotentParameterMismatch, got {body}"
    );
    assert_eq!(
        stored_ids(&router, "after the mismatch").await,
        vec![created_once.id],
        "a refused replay must create no second check"
    );

    // A token nobody used before is a fresh create rather than a replay.
    let mut other_token = first.clone();
    other_token.retain(|(key, _)| key != "ClientToken");
    other_token.push(p("ClientToken", "token-b"));
    let (status, body) = create(&router, &other_token).await;
    assert_eq!(status, 200, "a new token must create a check, got {status}: {body}");
    assert_eq!(
        stored_ids(&router, "after the second token").await.len(),
        2,
        "a second token must create a second check: {body}"
    );
}

/// "You can create a maximum of 50 application status checks for each account": the create
/// past that quota is refused, and a check inside its deletion grace period does not stand
/// between the account and its next create.
#[tokio::test]
async fn the_account_quota_bounds_how_many_checks_a_caller_can_create() {
    let (router, engine) = ec2_query::emulator();
    let store = engine.state::<Ec2State>();
    let mut seeded: Vec<String> = Vec::new();
    {
        let mut guard = store.lock();
        for _ in 0..MAX_APPLICATION_STATUS_CHECKS {
            // Minted the way every id of this service is, so the quota is counted over the
            // id form the store actually holds.
            let id = guard.next_id(APPLICATION_STATUS_CHECK_ID_PREFIX);
            guard.application_status_checks.insert(
                id.clone(),
                ApplicationStatusCheckResponseObject {
                    application_status_check_id: Some(id.clone()),
                    ..ApplicationStatusCheckResponseObject::default()
                },
            );
            seeded.push(id);
        }
    }

    let params = vec![p("Protocol", NETWORK_PROTOCOL_ENUM_VALUES[0]), p("Port", "80")];
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 400,
        "a create past the quota must be refused, got {status}: {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "ResourceLimitExceeded",
        "expected ResourceLimitExceeded, got {body}"
    );
    let message = ec2_query::error_message(&body);
    assert!(
        message.contains(&MAX_APPLICATION_STATUS_CHECKS.to_string()),
        "expected the refusal to name the quota of {MAX_APPLICATION_STATUS_CHECKS}, \
         got {message:?}"
    );

    // One of them deleted: "recently deleted application status checks that remain available
    // during the deletion grace period" are not checks the account still holds, so the quota
    // has room again.
    {
        let mut guard = store.lock();
        let deleted = seeded
            .first()
            .cloned()
            .unwrap_or_else(|| unreachable!("the quota is above zero"));
        if let Some(check) = guard.application_status_checks.get_mut(&deleted) {
            check.deletion_time = Some(DateTime::from_secs(1_767_225_600));
        }
    }
    let (status, body) = create(&router, &params).await;
    assert_eq!(
        status, 200,
        "a create inside the quota must succeed once a check is deleted, got {status}: {body}"
    );
    let row = created("after a deletion", &body);
    assert!(
        !seeded.contains(&row.id),
        "the create must mint a fresh id, got {:?}",
        row.id
    );
}
