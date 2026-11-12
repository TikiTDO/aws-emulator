// SPDX-License-Identifier: CC0-1.0

//! An Elastic Load Balancing load balancer CHECKS the target groups it routes to, publishes
//! the host counts of each as CloudWatch metric data, and holds its own network interfaces in
//! the account's EC2 inventory.
//!
//! Two rules are under test, and each needs three crates to agree (ELBv2 checks and places,
//! EC2 holds the inventory, CloudWatch answers for the series), so the suite lives here: a
//! crate-local test can only ever see one side of either.
//!
//! * **THE HOST-COUNT SERIES.** A load balancer publishes `HealthyHostCount` and
//!   `UnHealthyHostCount` into the namespace of its own family, under the dimension pair
//!   `TargetGroup` + `LoadBalancer`, for every target group a listener or a rule of it
//!   forwards to — counted off the health it reports for that group's registrations, where a
//!   registration the emulator holds nothing behind fails its checks and one it does holds up
//!   — and publishes NOTHING for a target group no listener of it references. An alarm over
//!   that series is then graded from those stored datapoints.
//! * **THE LOAD BALANCER'S OWN INTERFACES.** Creating a load balancer puts one
//!   service-managed interface in each subnet it is attached to, carrying a private address
//!   of that subnet, the `NetworkInterfaceType` of its family and the `RequesterId` of the
//!   service that made it; deleting the load balancer gives them back.
//!
//! The rows differ along what the rules generalise over: how a target group is routed (a
//! listener's default action, a rule of it, not at all), what stands behind its
//! registrations (an address a network interface of the account holds, addresses nothing
//! holds), how many registrations it has, the load balancer's family (application, network)
//! and the number of subnets it is placed in. The recorded configuration an issue happens to
//! mention is a single row.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element};

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The API version an ec2Query request carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// The API version an awsQuery request naming the ELBv2 model carries.
const ELB_VERSION: &str = "2015-12-01";

/// CloudWatch's `X-Amz-Target` prefix and signing name (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The namespace an Application Load Balancer's metrics live in, and the two host counts it
/// publishes per target group.
const APPLICATION_NAMESPACE: &str = "AWS/ApplicationELB";
const HEALTHY_HOST_COUNT: &str = "HealthyHostCount";
const UNHEALTHY_HOST_COUNT: &str = "UnHealthyHostCount";

/// The period one datapoint of that series occupies: these counts are published per minute.
const PERIOD: i64 = 60;

/// How long a row waits for what the emulator's accelerated schedule publishes, and how
/// often it looks. Generous on purpose: what is waited for takes a second or two, and the
/// deadline is only here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);

/// The VPC and the subnets every row runs in.
const VPC_CIDR: &str = "10.70.0.0/16";
const SUBNET_CIDRS: [(&str, &str); 3] = [
    ("10.70.1.0/24", "us-east-1a"),
    ("10.70.2.0/24", "us-east-1b"),
    ("10.70.3.0/24", "us-east-1c"),
];

/// Two addresses of the first subnet that nothing in the emulator answers on: high in the
/// range, so they are not addresses the inventory hands an interface of its own.
const DEAD_ADDRESSES: [&str; 2] = ["10.70.1.200", "10.70.1.201"];

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Epoch seconds, the unit a CloudWatch timestamp is read and written in.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// One form body: the action, the version and the parameters.
fn form(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

/// Send one form-encoded request to the service `signing_name` addresses, and answer its
/// status and body.
async fn send(router: &MultiServiceRouter, signing_name: &str, body: String) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Drive one ec2Query action that must succeed, and answer its body.
async fn ec2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = send(router, "ec2", form(action, EC2_API_VERSION, params)).await;
    assert_eq!(
        status, 200,
        "setup: EC2 {action} must succeed, got {status} with body {body}"
    );
    body
}

/// Drive one awsQuery action against the ELBv2 model that must succeed, and answer its body.
async fn elbv2(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let (status, body) = send(
        router,
        "elasticloadbalancingv2",
        form(action, ELB_VERSION, params),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: ELBv2 {action} must succeed, got {status} with body {body}"
    );
    body
}

/// Call one CloudWatch operation over its awsJson wire, requiring it to succeed.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{CLOUDWATCH_SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with body \
         {parsed}"
    );
    parsed
}

/// The network every row runs in: one VPC, three subnets in different zones, and a security
/// group of that VPC.
struct Network {
    vpc: String,
    /// `(subnet id, CIDR block)` for each subnet, in the order created.
    subnets: Vec<(String, String)>,
    security_group: String,
}

/// Create that network through EC2 itself, so every value asserted against is one EC2
/// reported rather than one this suite made up.
async fn seed_network(router: &MultiServiceRouter) -> Network {
    let vpc = element(
        &ec2(router, "CreateVpc", &[("CidrBlock", VPC_CIDR)]).await,
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (cidr, zone) in SUBNET_CIDRS {
        let created = ec2(
            router,
            "CreateSubnet",
            &[
                ("VpcId", vpc.as_str()),
                ("CidrBlock", cidr),
                ("AvailabilityZone", zone),
            ],
        )
        .await;
        subnets.push((element(&created, "subnetId"), cidr.to_string()));
    }
    let security_group = element(
        &ec2(
            router,
            "CreateSecurityGroup",
            &[
                ("GroupName", "elbv2-host-counts"),
                ("GroupDescription", "the load balancers own VPC"),
                ("VpcId", vpc.as_str()),
            ],
        )
        .await,
        "groupId",
    );
    Network {
        vpc,
        subnets,
        security_group,
    }
}

/// Create an `ip` target group in `vpc` and answer its ARN.
async fn ip_target_group(router: &MultiServiceRouter, name: &str, vpc: &str) -> String {
    element(
        &elbv2(
            router,
            "CreateTargetGroup",
            &[
                ("Name", name),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("TargetType", "ip"),
                ("VpcId", vpc),
            ],
        )
        .await,
        "TargetGroupArn",
    )
}

/// Register every address in `addresses` with the target group `group`.
async fn register(router: &MultiServiceRouter, group: &str, addresses: &[&str]) {
    let mut keys: Vec<String> = Vec::new();
    for index in 0..addresses.len() {
        keys.push(format!("Targets.member.{}.Id", index + 1));
    }
    let mut params: Vec<(&str, &str)> = vec![("TargetGroupArn", group)];
    for (key, address) in keys.iter().zip(addresses) {
        params.push((key.as_str(), address));
    }
    elbv2(router, "RegisterTargets", &params).await;
}

/// The `app/<name>/<id>` suffix of a load balancer ARN, which is how an Elastic Load
/// Balancing dimension names it.
fn load_balancer_dimension(arn: &str) -> String {
    arn.split_once(":loadbalancer/")
        .map(|(_, suffix)| suffix.to_string())
        .unwrap_or_else(|| panic!("a load balancer ARN carries its family and name: {arn}"))
}

/// The `targetgroup/<name>/<id>` suffix of a target group ARN, which is how that dimension
/// names it.
fn target_group_dimension(arn: &str) -> String {
    arn.split_once(":targetgroup/")
        .map(|(_, suffix)| format!("targetgroup/{suffix}"))
        .unwrap_or_else(|| panic!("a target group ARN carries its resource type: {arn}"))
}

/// Every datapoint CloudWatch reports for one host-count series, as the `Maximum` of each
/// one-minute window, oldest first.
async fn host_counts(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    target_group: &str,
    load_balancer: &str,
) -> Vec<f64> {
    let now = now_secs();
    let reported = cloudwatch(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": APPLICATION_NAMESPACE,
            "MetricName": metric,
            "Dimensions": [
                {"Name": "TargetGroup", "Value": target_group_dimension(target_group)},
                {"Name": "LoadBalancer", "Value": load_balancer_dimension(load_balancer)},
            ],
            "StartTime": now - 1_800.0,
            "EndTime": now + 60.0,
            "Period": PERIOD,
            "Statistics": ["Maximum"],
        }),
    )
    .await;
    let mut points: Vec<(f64, f64)> = reported["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| {
            let stamp = point["Timestamp"].as_f64().unwrap_or_else(|| {
                panic!("case {case:?}: every datapoint carries a Timestamp, got {reported}")
            });
            let maximum = point["Maximum"].as_f64().unwrap_or_else(|| {
                panic!("case {case:?}: every datapoint carries a Maximum, got {reported}")
            });
            (stamp, maximum)
        })
        .collect();
    points.sort_by(|left, right| left.0.total_cmp(&right.0));
    points.into_iter().map(|(_, maximum)| maximum).collect()
}

/// The host counts of one series, once its most recent datapoint reports `wanted`.
async fn host_counts_reaching(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    target_group: &str,
    load_balancer: &str,
    wanted: f64,
) -> Vec<f64> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let counts = host_counts(router, case, metric, target_group, load_balancer).await;
        if counts.last().is_some_and(|last| *last == wanted) || Instant::now() >= deadline {
            assert_eq!(
                counts.last().copied(),
                Some(wanted),
                "case {case:?}: expected the {metric} series to reach {wanted}, got {counts:?}"
            );
            return counts;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The `(state, reason, description)` each registration of `group` is reported in.
async fn target_health(router: &MultiServiceRouter, group: &str) -> Vec<(String, String, String)> {
    let body = elbv2(
        router,
        "DescribeTargetHealth",
        &[("TargetGroupArn", group)],
    )
    .await;
    let mut reported: Vec<(String, String, String)> = Vec::new();
    for health in body.split("<TargetHealth").skip(1) {
        // `<TargetHealthDescriptions>` carries the same prefix, so only a chunk where the
        // name ENDS — at the tag's closing bracket or at an attribute — is one health.
        if !health.starts_with('>') && !health.starts_with(' ') {
            continue;
        }
        reported.push((
            opt_element(health, "State").unwrap_or_default(),
            opt_element(health, "Reason").unwrap_or_default(),
            opt_element(health, "Description").unwrap_or_default(),
        ));
    }
    reported
}

/// One row of the host-count table: how a target group is routed, what its registrations
/// stand for, and the counts the series must reach.
struct Row {
    case: &'static str,
    /// The target group's name.
    name: &'static str,
    /// Whether a listener of the load balancer forwards to it, and how.
    routing: Routing,
    /// How many of its registrations the emulator holds something behind.
    healthy: f64,
    /// How many of them it holds nothing behind.
    unhealthy: f64,
}

/// How a row's target group is reached from the load balancer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Routing {
    /// The listener's own default action forwards to it.
    DefaultAction,
    /// A rule of that listener forwards to it, at the given priority.
    Rule(i32),
    /// Nothing forwards to it.
    Unrouted,
}

#[tokio::test]
async fn a_load_balancer_publishes_the_host_counts_of_every_target_group_it_routes_to() {
    let router = build_router();
    let network = seed_network(&router).await;

    // One interface of the account, in the load balancer's own subnet and its own security
    // group: its address is an address something in this VPC answers on, and that group
    // admits the load balancer's health check on the checked port — which together are what
    // make a target registered at it one the load balancer can reach.
    let interface = ec2(
        &router,
        "CreateNetworkInterface",
        &[
            ("SubnetId", network.subnets[0].0.as_str()),
            ("SecurityGroupId.1", network.security_group.as_str()),
            ("Description", "a workload of the account"),
        ],
    )
    .await;
    ec2(
        &router,
        "AuthorizeSecurityGroupIngress",
        &[
            ("GroupId", network.security_group.as_str()),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", "80"),
            ("IpPermissions.1.ToPort", "80"),
            (
                "IpPermissions.1.Groups.1.GroupId",
                network.security_group.as_str(),
            ),
        ],
    )
    .await;
    let served_address = element(&interface, "privateIpAddress");
    assert!(
        !DEAD_ADDRESSES.contains(&served_address.as_str()),
        "setup: the inventory's own address must differ from the dead ones, got \
         {served_address}"
    );

    // The rows: a group each form of routing reaches, one whose registrations are served and
    // one whose are not, and the twin nothing routes to.
    let rows: [Row; 4] = [
        Row {
            case: "the recorded configuration: two dead addresses behind the default action",
            name: "hc-default",
            routing: Routing::DefaultAction,
            healthy: 0.0,
            unhealthy: 2.0,
        },
        Row {
            case: "two dead addresses behind a path rule",
            name: "hc-rule",
            routing: Routing::Rule(10),
            healthy: 0.0,
            unhealthy: 2.0,
        },
        Row {
            case: "one address the account's inventory serves",
            name: "hc-served",
            routing: Routing::Rule(20),
            healthy: 1.0,
            unhealthy: 0.0,
        },
        Row {
            case: "a twin no listener references",
            name: "hc-twin",
            routing: Routing::Unrouted,
            healthy: 0.0,
            unhealthy: 0.0,
        },
    ];

    let mut groups: Vec<String> = Vec::new();
    for row in &rows {
        let group = ip_target_group(&router, row.name, &network.vpc).await;
        let addresses: Vec<&str> = if row.name == "hc-served" {
            vec![served_address.as_str()]
        } else {
            DEAD_ADDRESSES.to_vec()
        };
        register(&router, &group, &addresses).await;
        groups.push(group);
    }

    let load_balancer = element(
        &elbv2(
            &router,
            "CreateLoadBalancer",
            &[
                ("Name", "host-count-alb"),
                ("Scheme", "internal"),
                ("Subnets.member.1", network.subnets[0].0.as_str()),
                ("Subnets.member.2", network.subnets[1].0.as_str()),
                ("SecurityGroups.member.1", network.security_group.as_str()),
            ],
        )
        .await,
        "LoadBalancerArn",
    );
    let default_group = groups
        .iter()
        .zip(&rows)
        .find(|(_, row)| row.routing == Routing::DefaultAction)
        .map(|(group, _)| group.clone())
        .expect("one row is reached by the default action");
    let listener = element(
        &elbv2(
            &router,
            "CreateListener",
            &[
                ("LoadBalancerArn", load_balancer.as_str()),
                ("Protocol", "HTTP"),
                ("Port", "80"),
                ("DefaultActions.member.1.Type", "forward"),
                (
                    "DefaultActions.member.1.TargetGroupArn",
                    default_group.as_str(),
                ),
            ],
        )
        .await,
        "ListenerArn",
    );
    for (group, row) in groups.iter().zip(&rows) {
        if let Routing::Rule(priority) = row.routing {
            let priority = priority.to_string();
            elbv2(
                &router,
                "CreateRule",
                &[
                    ("ListenerArn", listener.as_str()),
                    ("Priority", priority.as_str()),
                    ("Conditions.member.1.Field", "path-pattern"),
                    ("Conditions.member.1.Values.member.1", "/*"),
                    ("Actions.member.1.Type", "forward"),
                    ("Actions.member.1.TargetGroupArn", group.as_str()),
                ],
            )
            .await;
        }
    }

    // Every ROUTED group's series reaches the counts its registrations imply, and the health
    // read agrees with them: the series is counted off the very health a client reads.
    for (group, row) in groups.iter().zip(&rows) {
        if row.routing == Routing::Unrouted {
            continue;
        }
        let unhealthy = host_counts_reaching(
            &router,
            row.case,
            UNHEALTHY_HOST_COUNT,
            group,
            &load_balancer,
            row.unhealthy,
        )
        .await;
        assert!(
            unhealthy.iter().all(|count| *count <= row.unhealthy),
            "case {:?}: no window may count more unhealthy hosts than the group holds, got \
             {unhealthy:?}",
            row.case
        );
        let healthy = host_counts_reaching(
            &router,
            row.case,
            HEALTHY_HOST_COUNT,
            group,
            &load_balancer,
            row.healthy,
        )
        .await;
        assert!(
            healthy.iter().all(|count| *count <= row.healthy),
            "case {:?}: no window may count more healthy hosts than the group holds, got \
             {healthy:?}",
            row.case
        );

        let reported = target_health(&router, group).await;
        let expected_states: Vec<&str> = if row.unhealthy > 0.0 {
            vec!["unhealthy"; row.unhealthy as usize]
        } else {
            vec!["healthy"; row.healthy as usize]
        };
        let states: Vec<&str> = reported.iter().map(|(state, _, _)| state.as_str()).collect();
        assert_eq!(
            states, expected_states,
            "case {:?}: expected the health read to agree with the series, got {reported:?}",
            row.case
        );
        if row.unhealthy > 0.0 {
            for (state, reason, description) in &reported {
                assert!(
                    reason == "Target.Timeout" && !description.is_empty(),
                    "case {:?}: an unhealthy target carries the reason its check failed for \
                     and a description, got {state}/{reason}/{description:?}",
                    row.case
                );
            }
        }
    }

    // And the twin nothing routes to publishes no series at all under this load balancer's
    // dimension — neither count, not even a zero.
    let twin = groups
        .iter()
        .zip(&rows)
        .find(|(_, row)| row.routing == Routing::Unrouted)
        .map(|(group, _)| group.clone())
        .expect("one row is routed by nothing");
    for metric in [UNHEALTHY_HOST_COUNT, HEALTHY_HOST_COUNT] {
        let counts = host_counts(&router, "a twin no listener references", metric, &twin, &load_balancer)
            .await;
        assert!(
            counts.is_empty(),
            "case \"a twin no listener references\": expected no {metric} datapoint, got \
             {counts:?}"
        );
    }
    let unused = target_health(&router, &twin).await;
    assert!(
        unused
            .iter()
            .all(|(state, reason, _)| state == "unused" && reason == "Target.NotInUse"),
        "case \"a twin no listener references\": a group nothing forwards to has nothing \
         checking its targets, got {unused:?}"
    );

    // An alarm over the published series is graded from those datapoints — under
    // `notBreaching`, where silence cannot fire it — and the alarm over the twin's silent
    // series stays out of ALARM for exactly that reason.
    for (case, group, wanted) in [
        (
            "an alarm over a published series",
            default_group.as_str(),
            "ALARM",
        ),
        ("an alarm over the twin's silent series", twin.as_str(), "OK"),
    ] {
        let name = format!("unhealthy-hosts-{}", target_group_dimension(group).replace('/', "-"));
        cloudwatch(
            &router,
            case,
            "PutMetricAlarm",
            &json!({
                "AlarmName": name,
                "Namespace": APPLICATION_NAMESPACE,
                "MetricName": UNHEALTHY_HOST_COUNT,
                "Dimensions": [
                    {"Name": "TargetGroup", "Value": target_group_dimension(group)},
                    {"Name": "LoadBalancer", "Value": load_balancer_dimension(&load_balancer)},
                ],
                "Statistic": "Maximum",
                "Period": PERIOD,
                "EvaluationPeriods": 2,
                "Threshold": 0.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": "notBreaching",
            }),
        )
        .await;
        let deadline = Instant::now() + DEADLINE;
        let described = loop {
            let described = cloudwatch(
                &router,
                case,
                "DescribeAlarms",
                &json!({"AlarmNames": [name]}),
            )
            .await;
            let state = described["MetricAlarms"][0]["StateValue"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if state == wanted || Instant::now() >= deadline {
                assert_eq!(
                    state, wanted,
                    "case {case:?}: expected the alarm in {wanted}, got {described}"
                );
                break described;
            }
            ::tokio::time::sleep(POLL).await;
        };
        if wanted != "ALARM" {
            continue;
        }
        // The alarm that fired did so from REAL datapoints: its explanation names the
        // datapoints it graded, the threshold it compared them against and the statistic it
        // took over each window.
        let reason_data: Value = serde_json::from_str(
            described["MetricAlarms"][0]["StateReasonData"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the alarm carries its reason data, got {described}")),
        )
        .unwrap_or(Value::Null);
        let recent: Vec<f64> = reason_data["recentDatapoints"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_f64)
            .collect();
        assert!(
            recent.len() == 2 && recent.iter().all(|value| *value == 2.0),
            "case {case:?}: expected the two graded datapoints to be the published count, got \
             {reason_data}"
        );
        assert_eq!(
            (
                reason_data["threshold"].as_f64(),
                reason_data["statistic"].as_str()
            ),
            (Some(0.0), Some("Maximum")),
            "case {case:?}: expected the alarm's own threshold and statistic, got {reason_data}"
        );
    }
}

/// One row of the interface table: a load balancer to create, and what the inventory must
/// then hold for it.
struct InterfaceRow {
    case: &'static str,
    name: &'static str,
    /// The `Type` the create names, or `None` for the modelled default.
    lb_type: Option<&'static str>,
    /// How many of the network's subnets it is placed in.
    subnets: usize,
    /// The `NetworkInterfaceType` its own interfaces carry.
    interface_type: &'static str,
}

#[tokio::test]
async fn a_load_balancer_holds_one_service_managed_interface_per_subnet() {
    let router = build_router();
    let network = seed_network(&router).await;

    // A target group with two dead addresses registered, so the rows can assert that no
    // address a load balancer holds is one of them.
    let group = ip_target_group(&router, "iface-targets", &network.vpc).await;
    register(&router, &group, &DEAD_ADDRESSES).await;

    // The rows differ along the load balancer's family and the number of subnets it is
    // placed in — which is what the rule generalises over.
    let rows: [InterfaceRow; 2] = [
        InterfaceRow {
            case: "the recorded configuration: an internal application load balancer in two \
                   subnets",
            name: "iface-alb",
            lb_type: None,
            subnets: 2,
            interface_type: "load_balancer",
        },
        InterfaceRow {
            case: "a network load balancer in one subnet",
            name: "iface-nlb",
            lb_type: Some("network"),
            subnets: 1,
            interface_type: "network_load_balancer",
        },
    ];

    for row in &rows {
        let before = vpc_interface_ids(&router, &network.vpc).await;
        let mut params: Vec<(&str, &str)> = vec![("Name", row.name), ("Scheme", "internal")];
        if let Some(lb_type) = row.lb_type {
            params.push(("Type", lb_type));
        } else {
            // The groups of an Application Load Balancer govern traffic to the interfaces
            // below, so the row that has them starts with one.
            params.push(("SecurityGroups.member.1", network.security_group.as_str()));
        }
        let subnet_keys = ["Subnets.member.1", "Subnets.member.2"];
        for (key, (subnet, _)) in subnet_keys
            .iter()
            .zip(network.subnets.iter())
            .take(row.subnets)
        {
            params.push((key, subnet.as_str()));
        }
        let created = elbv2(&router, "CreateLoadBalancer", &params).await;
        let load_balancer = element(&created, "LoadBalancerArn");
        let placed = vpc_interface_ids(&router, &network.vpc).await;
        let mut mine: Vec<Interface> = Vec::new();
        for id in placed.iter().filter(|id| !before.contains(id)) {
            mine.push(describe_interface(&router, id).await);
        }
        assert_eq!(
            mine.len(),
            row.subnets,
            "case {:?}: expected one interface per subnet, got {mine:?}",
            row.case
        );
        let subnets: BTreeSet<&str> = network
            .subnets
            .iter()
            .take(row.subnets)
            .map(|(subnet, _)| subnet.as_str())
            .collect();
        for interface in &mine {
            assert_eq!(
                (
                    interface.interface_type.as_str(),
                    interface.requester.as_str(),
                    interface.requester_managed.as_str()
                ),
                (row.interface_type, "amazon-elb", "true"),
                "case {:?}: expected the family's interface type on a service-managed \
                 interface, got {interface:?}",
                row.case
            );
            assert!(
                subnets.contains(interface.subnet.as_str()),
                "case {:?}: an interface sits in a subnet the load balancer is attached to, \
                 got {interface:?}",
                row.case
            );
            let (_, cidr) = network
                .subnets
                .iter()
                .find(|(subnet, _)| *subnet == interface.subnet)
                .expect("the interface's subnet is one of the network's");
            let prefix = cidr
                .split_once(".0/")
                .map(|(network_part, _)| format!("{network_part}."))
                .expect("the subnet CIDR names its own range");
            assert!(
                interface.address.starts_with(&prefix),
                "case {:?}: an interface's address comes out of its own subnet ({cidr}), got \
                 {interface:?}",
                row.case
            );
            assert!(
                !DEAD_ADDRESSES.contains(&interface.address.as_str()),
                "case {:?}: no address a load balancer holds is a registered target's, got \
                 {interface:?}",
                row.case
            );
        }

        // The same query filtered on the requester selects exactly those interfaces: they
        // are the ones this service created and manages.
        let requested = vpc_interface_ids_requested_by(&router, &network.vpc, "amazon-elb").await;
        assert!(
            mine.iter()
                .all(|interface| requested.contains(&interface.id)),
            "case {:?}: expected the requester-id filter to select the load balancer's own \
             interfaces, got {requested:?}",
            row.case
        );

        // A load balancer's security groups govern traffic to exactly those interfaces, so a
        // change to them is a change to what the inventory reports for each one — while each
        // interface keeps the address it holds.
        if row.lb_type.is_none() {
            for interface in &mine {
                assert_eq!(
                    interface.groups,
                    vec![network.security_group.clone()],
                    "case {:?}: expected the load balancer's own group on its interface, got \
                     {interface:?}",
                    row.case
                );
            }
            let second = element(
                &ec2(
                    &router,
                    "CreateSecurityGroup",
                    &[
                        ("GroupName", "elbv2-host-counts-second"),
                        ("GroupDescription", "a second group of the same VPC"),
                        ("VpcId", network.vpc.as_str()),
                    ],
                )
                .await,
                "groupId",
            );
            elbv2(
                &router,
                "SetSecurityGroups",
                &[
                    ("LoadBalancerArn", load_balancer.as_str()),
                    ("SecurityGroups.member.1", second.as_str()),
                ],
            )
            .await;
            for interface in &mine {
                let reread = describe_interface(&router, &interface.id).await;
                assert_eq!(
                    (reread.groups.as_slice(), reread.address.as_str()),
                    (
                        std::slice::from_ref(&second),
                        interface.address.as_str()
                    ),
                    "case {:?}: expected the interface to report the group the load balancer \
                     carries now and keep its address, got {reread:?}",
                    row.case
                );
            }

            // And the set follows the SUBNETS the load balancer is attached to: a subnet it
            // still has keeps the interface already there — the load balancer never stopped
            // being reachable at that address — while the subnet it has left gives its
            // interface back and the one it has just been given gets one of its own.
            let retained = mine
                .iter()
                .find(|interface| interface.subnet == network.subnets[1].0)
                .expect("one interface sits in the second subnet")
                .clone();
            let left = mine
                .iter()
                .find(|interface| interface.subnet == network.subnets[0].0)
                .expect("one interface sits in the first subnet")
                .clone();
            elbv2(
                &router,
                "SetSubnets",
                &[
                    ("LoadBalancerArn", load_balancer.as_str()),
                    ("Subnets.member.1", network.subnets[1].0.as_str()),
                    ("Subnets.member.2", network.subnets[2].0.as_str()),
                ],
            )
            .await;
            let mut held: Vec<Interface> = Vec::new();
            for id in vpc_interface_ids_requested_by(&router, &network.vpc, "amazon-elb").await {
                held.push(describe_interface(&router, &id).await);
            }
            assert_eq!(
                held.len(),
                2,
                "case {:?}: expected one interface per subnet after the move, got {held:?}",
                row.case
            );
            assert!(
                held.iter().any(|interface| interface.id == retained.id
                    && interface.address == retained.address),
                "case {:?}: expected the interface of the retained subnet to be kept as it \
                 was, got {held:?}",
                row.case
            );
            assert!(
                !held.iter().any(|interface| interface.id == left.id),
                "case {:?}: expected the interface of the subnet the load balancer left to \
                 be given back, got {held:?}",
                row.case
            );
            assert!(
                held.iter()
                    .any(|interface| interface.subnet == network.subnets[2].0),
                "case {:?}: expected an interface in the subnet the load balancer was just \
                 given, got {held:?}",
                row.case
            );
            mine = held;
        }

        // And they do not outlive the load balancer.
        let mine_ids: BTreeSet<&str> = mine.iter().map(|interface| interface.id.as_str()).collect();
        elbv2(
            &router,
            "DeleteLoadBalancer",
            &[("LoadBalancerArn", load_balancer.as_str())],
        )
        .await;
        let left = vpc_interface_ids(&router, &network.vpc).await;
        assert!(
            left.iter().all(|id| !mine_ids.contains(id.as_str())),
            "case {:?}: a deleted load balancer holds no interface, got {left:?}",
            row.case
        );
    }
}

/// One network interface of the account, reduced to what these rows assert over.
#[derive(Debug, Clone)]
struct Interface {
    id: String,
    subnet: String,
    address: String,
    interface_type: String,
    requester: String,
    requester_managed: String,
    groups: Vec<String>,
}

/// The id of every interface EC2 reports in `vpc`, through the `vpc-id` filter a client
/// diagnosing its own VPC uses.
async fn vpc_interface_ids(router: &MultiServiceRouter, vpc: &str) -> Vec<String> {
    let body = ec2(
        router,
        "DescribeNetworkInterfaces",
        &[("Filter.1.Name", "vpc-id"), ("Filter.1.Value.1", vpc)],
    )
    .await;
    elements(&body, "networkInterfaceId")
}

/// The same read, additionally filtered on the service that created the interfaces.
async fn vpc_interface_ids_requested_by(
    router: &MultiServiceRouter,
    vpc: &str,
    requester: &str,
) -> Vec<String> {
    let body = ec2(
        router,
        "DescribeNetworkInterfaces",
        &[
            ("Filter.1.Name", "vpc-id"),
            ("Filter.1.Value.1", vpc),
            ("Filter.2.Name", "requester-id"),
            ("Filter.2.Value.1", requester),
        ],
    )
    .await;
    elements(&body, "networkInterfaceId")
}

/// One interface as EC2 describes it when asked for exactly it, which is how a client reads
/// an address off an id it found through a filter.
async fn describe_interface(router: &MultiServiceRouter, id: &str) -> Interface {
    let body = ec2(
        router,
        "DescribeNetworkInterfaces",
        &[("NetworkInterfaceId.1", id)],
    )
    .await;
    Interface {
        id: id.to_string(),
        subnet: opt_element(&body, "subnetId").unwrap_or_default(),
        address: opt_element(&body, "privateIpAddress").unwrap_or_default(),
        interface_type: opt_element(&body, "interfaceType").unwrap_or_default(),
        requester: opt_element(&body, "requesterId").unwrap_or_default(),
        requester_managed: opt_element(&body, "requesterManaged").unwrap_or_default(),
        groups: elements(&body, "groupId"),
    }
}
