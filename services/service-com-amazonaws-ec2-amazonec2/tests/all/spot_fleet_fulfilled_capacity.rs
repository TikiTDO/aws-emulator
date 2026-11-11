// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the one set of instances a SPOT FLEET reports, driven over the wire
//! the service actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rule under test is that a Spot Fleet's instances and the capacity it reports as
//! FULFILLED are the same set: "the number of units fulfilled by this request compared to the
//! set target capacity" (`SpotFleetRequestConfigData$FulfilledCapacity`) is provided by the
//! instances the fleet's Spot Instance requests launched, and
//! `DescribeSpotFleetInstances` reports those instances — every one the fleet launched that
//! the account still holds, with the instance type the pool it was placed in asked for. So at
//! the instant the fleet reports its target capacity fulfilled, a caller that asked what the
//! fleet is running is told the instances fulfilling it, rather than an empty list: a client
//! waiting for capacity has something to act on as soon as the fleet says the capacity is
//! there.
//!
//! Every row is a distinct input along what that generalises over — how much target capacity
//! the fleet asks for, how many pools it spreads it over, what one instance of a pool
//! provides, which instance types it launches, and whether the launch is described by the
//! fleet's own launch specifications or by a launch template with overrides naming the
//! instance type — so the one case a recording happens to contain is one row among many
//! rather than the specification.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, SPOT_FLEET_ACTIVITY_FULFILLED,
    SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The ARN of the IAM role a Spot Fleet request grants the fleet. Its account is NOT this
/// emulator's: the role is the caller's.
const FLEET_ROLE: &str = "arn:aws:iam::111122223333:role/aws-ec2-spot-fleet-tagging-role";

/// One request's form parameters.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `SpotFleetRequestConfig.<member>` parameter.
fn c(member: &str, value: impl Into<String>) -> (String, String) {
    p(&format!("SpotFleetRequestConfig.{member}"), value)
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see: read out of the service's
/// OWN state rather than written here, because a launch must name an image the account can
/// resolve.
fn image_id() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

/// How a row describes the launch its pools make.
enum Launch {
    /// The fleet's own `LaunchSpecifications`: one entry per pool, with its instance type and
    /// what one of its instances provides.
    Specifications(&'static [(&'static str, Option<f64>)]),
    /// A launch template carrying `template_type`, with one `Overrides` entry per pool naming
    /// the instance type that pool launches instead.
    TemplateOverrides {
        template_type: &'static str,
        overrides: &'static [(&'static str, Option<f64>)],
    },
}

/// One row: a fleet configuration and the instance types it must end up running.
struct Row {
    case: &'static str,
    /// `SpotFleetRequestConfig.TargetCapacity`.
    target_capacity: u32,
    /// Whether the fleet spreads its target across every pool.
    diversified: bool,
    launch: Launch,
    /// How many instances of each type the fleet must report, by type.
    expected: &'static [(&'static str, usize)],
}

/// The rows. No two share a target capacity, pool count, weight and type combination.
const ROWS: &[Row] = &[
    Row {
        case: "one unit in one launch specification",
        target_capacity: 1,
        diversified: false,
        launch: Launch::Specifications(&[("t3.micro", None)]),
        expected: &[("t3.micro", 1)],
    },
    Row {
        case: "two units in one pool, one unit per instance",
        target_capacity: 2,
        diversified: false,
        launch: Launch::Specifications(&[("m5.large", Some(1.0))]),
        expected: &[("m5.large", 2)],
    },
    Row {
        case: "four units in one pool whose instance provides two",
        target_capacity: 4,
        diversified: false,
        launch: Launch::Specifications(&[("c5.xlarge", Some(2.0))]),
        expected: &[("c5.xlarge", 2)],
    },
    Row {
        case: "two units spread across two pools",
        target_capacity: 2,
        diversified: true,
        launch: Launch::Specifications(&[("t3.small", Some(1.0)), ("c6g.medium", Some(1.0))]),
        expected: &[("t3.small", 1), ("c6g.medium", 1)],
    },
    Row {
        case: "one unit from a launch template whose override names the instance type",
        target_capacity: 1,
        diversified: false,
        launch: Launch::TemplateOverrides {
            template_type: "m5.xlarge",
            overrides: &[("t4g.small", None)],
        },
        expected: &[("t4g.small", 1)],
    },
];

impl Row {
    /// How many instances this row's fleet must be running in total.
    fn expected_total(&self) -> usize {
        self.expected.iter().map(|(_, count)| *count).sum()
    }

    /// The expected instance types as a count by type, which is what the report is compared
    /// against: the report's ORDER is the fleet's placement order and is covered by the
    /// `DescribeSpotFleetInstances` suite.
    fn expected_counts(&self) -> BTreeMap<String, usize> {
        self.expected
            .iter()
            .map(|(instance_type, count)| ((*instance_type).to_string(), *count))
            .collect()
    }
}

/// Create the launch template one row needs, and answer its id.
async fn create_template(
    case: &str,
    router: &MultiServiceRouter,
    name: &str,
    instance_type: &str,
) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", image_id()),
            p("LaunchTemplateData.InstanceType", instance_type),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateLaunchTemplate must succeed, got {status}: {body}"
    );
    element(&body, "launchTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: no launchTemplateId in {body}"))
}

/// The `RequestSpotFleet` parameters one row sends.
async fn fleet_params(case: &str, router: &MultiServiceRouter, row: &Row, index: usize) -> Params {
    let mut params = vec![
        c("IamFleetRole", FLEET_ROLE),
        c("TargetCapacity", row.target_capacity.to_string()),
    ];
    if row.diversified {
        params.push(c(
            "AllocationStrategy",
            SPOT_FLEET_ALLOCATION_STRATEGY_DIVERSIFIED,
        ));
    }
    match row.launch {
        Launch::Specifications(pools) => {
            for (position, (instance_type, weight)) in (1..).zip(pools) {
                params.push(c(
                    &format!("LaunchSpecifications.{position}.ImageId"),
                    image_id(),
                ));
                params.push(c(
                    &format!("LaunchSpecifications.{position}.InstanceType"),
                    *instance_type,
                ));
                if let Some(weight) = weight {
                    params.push(c(
                        &format!("LaunchSpecifications.{position}.WeightedCapacity"),
                        weight.to_string(),
                    ));
                }
            }
        }
        Launch::TemplateOverrides {
            template_type,
            overrides,
        } => {
            let template_id = create_template(
                case,
                router,
                &format!("spot-fleet-capacity-{index}"),
                template_type,
            )
            .await;
            params.push(c(
                "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateId",
                template_id,
            ));
            for (position, (instance_type, weight)) in (1..).zip(overrides) {
                params.push(c(
                    &format!("LaunchTemplateConfigs.1.Overrides.{position}.InstanceType"),
                    *instance_type,
                ));
                if let Some(weight) = weight {
                    params.push(c(
                        &format!("LaunchTemplateConfigs.1.Overrides.{position}.WeightedCapacity"),
                        weight.to_string(),
                    ));
                }
            }
        }
    }
    params
}

/// Request one Spot Fleet and answer the `SpotFleetRequestId` the service minted.
async fn request_fleet(case: &str, router: &MultiServiceRouter, params: &[(String, String)]) -> String {
    let (status, body) = call(router, REGION, "RequestSpotFleet", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RequestSpotFleet must succeed, got {status}: {body}"
    );
    element(&body, "spotFleetRequestId")
        .unwrap_or_else(|| panic!("case {case:?}: no spotFleetRequestId in {body}"))
}

/// The fleet's `ActivityStatus` and `FulfilledCapacity`, as `DescribeSpotFleetRequests`
/// reports them.
async fn reported_capacity(case: &str, router: &MultiServiceRouter, fleet_id: &str) -> (String, f64) {
    let (status, body) = call(
        router,
        REGION,
        "DescribeSpotFleetRequests",
        &indexed("SpotFleetRequestId", &[fleet_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSpotFleetRequests must succeed, got {status}: {body}"
    );
    let activity = element(&body, "activityStatus").unwrap_or_default();
    let fulfilled = element(&body, "fulfilledCapacity")
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or_default();
    (activity, fulfilled)
}

/// The instance types `DescribeSpotFleetInstances` reports for one fleet, counted by type,
/// beside how many instances it reported in all.
async fn running_types(
    case: &str,
    router: &MultiServiceRouter,
    fleet_id: &str,
) -> (BTreeMap<String, usize>, usize) {
    let (status, body) = call(
        router,
        REGION,
        "DescribeSpotFleetInstances",
        &[p("SpotFleetRequestId", fleet_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSpotFleetInstances must succeed, got {status}: {body}"
    );
    let reported = elements(&body, "instanceType");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for instance_type in &reported {
        *counts.entry(instance_type.clone()).or_default() += 1;
    }
    (counts, reported.len())
}

/// The rule: from the instant the fleet reports its target capacity FULFILLED, the instances
/// it reports are the ones fulfilling it — each of the type the pool it was placed in asked
/// for, and none of them waited for by a caller that has already been told the capacity is
/// there.
#[tokio::test]
async fn a_fleet_reports_the_instances_whose_capacity_it_reports_as_fulfilled() {
    let (router, _engine) = emulator();

    // Every row's fleet is requested first, so the capacity each one reports is reached by the
    // clock rather than by the order the rows are polled in.
    let mut fleets: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let params = fleet_params(row.case, &router, row, index).await;
        let fleet_id = request_fleet(row.case, &router, &params).await;
        fleets.push((row, fleet_id));
    }

    for (row, fleet_id) in &fleets {
        let case = row.case;
        // Poll the way a client waiting for capacity does, and stop at the FIRST report that
        // says the capacity is fulfilled: the deadline is a multiple of the service's own
        // transition span, so a fleet that never gets there fails the row instead of hanging.
        let deadline = Instant::now() + TRANSITION_DELAY * 8;
        let fulfilled = loop {
            let (activity, fulfilled) = reported_capacity(case, &router, fleet_id).await;
            if activity == SPOT_FLEET_ACTIVITY_FULFILLED {
                break fulfilled;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the fleet never reported {SPOT_FLEET_ACTIVITY_FULFILLED}, \
                 last activity status {activity:?} at {fulfilled} unit(s) fulfilled"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert!(
            fulfilled >= f64::from(row.target_capacity),
            "case {case:?}: a fulfilled fleet holds at least its target of {} unit(s), \
             got {fulfilled}",
            row.target_capacity
        );

        // The instances fulfilling that capacity, asked for in the same breath.
        let (counts, total) = running_types(case, &router, fleet_id).await;
        assert_eq!(
            total,
            row.expected_total(),
            "case {case:?}: expected {} instance(s) in the fulfilled fleet, got {counts:?}",
            row.expected_total()
        );
        assert_eq!(
            counts,
            row.expected_counts(),
            "case {case:?}: expected {:?}, got {counts:?}",
            row.expected_counts()
        );
    }
}
