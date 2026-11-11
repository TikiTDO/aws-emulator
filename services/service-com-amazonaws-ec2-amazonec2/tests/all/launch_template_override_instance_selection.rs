// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for how a LAUNCH TEMPLATE OVERRIDE says which instance type its pool
//! launches, driven over the wire the service actually speaks — `ec2Query`, through the
//! crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test is the one note the model puts on both fleet APIs' overrides: "if you
//! specify `InstanceRequirements`, you can't specify `InstanceType`"
//! (`LaunchTemplateOverrides$InstanceRequirements` and
//! `FleetLaunchTemplateOverridesRequest$InstanceRequirements`). An override is one Spot pool of
//! the fleet, and the two members are two ways of saying what that pool launches, so:
//!
//! * an override carrying BOTH is refused with `InvalidParameterCombinationException` and the
//!   one sentence AWS words that refusal with, whichever fleet API it was sent to and wherever
//!   in the overrides list it sits;
//! * an override carrying instance REQUIREMENTS alone is refused with `UnsupportedOperation`:
//!   this service resolves no instance types from a requirement set, and a pool that quietly
//!   launched the template's own instance type instead would not be the pool the request
//!   described;
//! * an override carrying an instance TYPE alone is the pool it names, and the fleet is
//!   accepted.
//!
//! Each row is a distinct input along what that generalises over — which fleet API, where in
//! the overrides list the override sits, which instance type it names and which requirement
//! members it carries — and every refusal is checked to have stored NOTHING.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, INSTANCE_REQUIREMENTS_WITH_INSTANCE_TYPE_MESSAGE,
    INVALID_PARAMETER_COMBINATION_EXCEPTION,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The ARN of the IAM role a Spot Fleet request grants the fleet. Its account is NOT this
/// emulator's: the role is the caller's.
const FLEET_ROLE: &str = "arn:aws:iam::111122223333:role/aws-ec2-spot-fleet-tagging-role";

/// The instance type the launch template itself carries, which an override replaces.
const TEMPLATE_TYPE: &str = "m5.xlarge";

/// One request's form parameters.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// An AMI of the published catalog a fresh account can already see, read out of the service's
/// OWN state: a launch template must name an image the account can resolve.
fn image_id() -> String {
    Ec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image")
}

/// Create the launch template both APIs' rows launch from, and answer its id.
async fn launch_template(case: &str, router: &MultiServiceRouter) -> String {
    let (status, body) = call(
        router,
        REGION,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "override-selection"),
            p("LaunchTemplateData.ImageId", image_id()),
            p("LaunchTemplateData.InstanceType", TEMPLATE_TYPE),
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

/// Which fleet API a row sends its overrides to. Both take "the launch template and
/// overrides", so both are held to the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Api {
    /// `RequestSpotFleet`, whose configuration nests the launch template configs.
    SpotFleet,
    /// `CreateFleet`, which carries them at the top level.
    Fleet,
}

impl Api {
    fn action(self) -> &'static str {
        match self {
            Api::SpotFleet => "RequestSpotFleet",
            Api::Fleet => "CreateFleet",
        }
    }

    /// The parameters that make a request of this API valid apart from its overrides: the
    /// fleet's target capacity, its launch template, and whatever else the API requires.
    fn base(self, template_id: &str) -> Params {
        match self {
            Api::SpotFleet => vec![
                p("SpotFleetRequestConfig.IamFleetRole", FLEET_ROLE),
                p("SpotFleetRequestConfig.TargetCapacity", "1"),
                p(
                    "SpotFleetRequestConfig.LaunchTemplateConfigs.1.\
                     LaunchTemplateSpecification.LaunchTemplateId",
                    template_id,
                ),
            ],
            Api::Fleet => vec![
                p("TargetCapacitySpecification.TotalTargetCapacity", "1"),
                p(
                    "LaunchTemplateConfigs.1.LaunchTemplateSpecification.LaunchTemplateId",
                    template_id,
                ),
            ],
        }
    }

    /// The parameter prefix of one `Overrides` entry of this API's first launch template
    /// config.
    fn override_prefix(self, position: usize) -> String {
        match self {
            Api::SpotFleet => format!(
                "SpotFleetRequestConfig.LaunchTemplateConfigs.1.Overrides.{position}"
            ),
            Api::Fleet => format!("LaunchTemplateConfigs.1.Overrides.{position}"),
        }
    }
}

/// What one override of a row carries.
struct Override {
    /// `Overrides.<n>.InstanceType`, when the override names one.
    instance_type: Option<&'static str>,
    /// `Overrides.<n>.InstanceRequirements.<member>` pairs, when the override carries a
    /// requirement set. The members are the modelled ranges
    /// (`smithy_query(query="shape",shape="VCpuCountRange,MemoryMiB")`).
    requirements: &'static [(&'static str, &'static str)],
}

/// One row: which API, the overrides it sends, and what the call must answer.
struct Row {
    case: &'static str,
    api: Api,
    overrides: &'static [Override],
    /// The HTTP status and error code the call must answer, or `None` when it must succeed.
    refusal: Option<(u16, &'static str)>,
}

/// The modelled requirement members the rows carry, so no row's requirement set is another's.
/// `InstanceRequirementsRequest` — what `CreateFleet`'s override targets — makes `VCpuCount`
/// and `MemoryMiB` required members, so an EC2 Fleet row names both
/// (`smithy_query(query="shape",shape="VCpuCountRange,MemoryMiB")`).
const VCPU_AND_MEMORY: &[(&str, &str)] =
    &[("VCpuCount.Min", "2"), ("MemoryMiB.Min", "1024")];
const VCPU_RANGE: &[(&str, &str)] = &[("VCpuCount.Min", "4"), ("VCpuCount.Max", "8")];
const VCPU_RANGE_AND_MEMORY: &[(&str, &str)] = &[
    ("VCpuCount.Min", "4"),
    ("VCpuCount.Max", "8"),
    ("MemoryMiB.Min", "2048"),
];

const ROWS: &[Row] = &[
    Row {
        case: "a Spot Fleet override naming both an instance type and instance requirements",
        api: Api::SpotFleet,
        overrides: &[Override {
            instance_type: Some("t3.micro"),
            requirements: VCPU_AND_MEMORY,
        }],
        refusal: Some((400, INVALID_PARAMETER_COMBINATION_EXCEPTION)),
    },
    Row {
        case: "the SECOND Spot Fleet override naming both, after one that names only a type",
        api: Api::SpotFleet,
        overrides: &[
            Override {
                instance_type: Some("c5.large"),
                requirements: &[],
            },
            Override {
                instance_type: Some("t3.small"),
                requirements: VCPU_RANGE,
            },
        ],
        refusal: Some((400, INVALID_PARAMETER_COMBINATION_EXCEPTION)),
    },
    Row {
        case: "an EC2 Fleet override naming both an instance type and instance requirements",
        api: Api::Fleet,
        overrides: &[Override {
            instance_type: Some("t4g.small"),
            requirements: VCPU_RANGE_AND_MEMORY,
        }],
        refusal: Some((400, INVALID_PARAMETER_COMBINATION_EXCEPTION)),
    },
    Row {
        case: "a Spot Fleet override naming instance requirements alone",
        api: Api::SpotFleet,
        overrides: &[Override {
            instance_type: None,
            requirements: VCPU_AND_MEMORY,
        }],
        refusal: Some((400, "UnsupportedOperation")),
    },
    Row {
        case: "an EC2 Fleet override naming instance requirements alone",
        api: Api::Fleet,
        overrides: &[Override {
            instance_type: None,
            requirements: VCPU_AND_MEMORY,
        }],
        refusal: Some((400, "UnsupportedOperation")),
    },
    Row {
        case: "a Spot Fleet override naming an instance type alone",
        api: Api::SpotFleet,
        overrides: &[Override {
            instance_type: Some("t3.micro"),
            requirements: &[],
        }],
        refusal: None,
    },
    Row {
        case: "an EC2 Fleet override naming an instance type alone",
        api: Api::Fleet,
        overrides: &[Override {
            instance_type: Some("c6g.medium"),
            requirements: &[],
        }],
        refusal: None,
    },
];

/// How much this account holds of everything a fleet request writes: the fleets, the Spot
/// Fleet requests, the Spot Instance requests and the instances.
fn store_counts(engine: &engine::InMemoryEngine) -> (usize, usize, usize, usize) {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    (
        store.fleets.len(),
        store.spot_fleet_requests.len(),
        store.spot_instance_requests.len(),
        store.instances.len(),
    )
}

#[tokio::test]
async fn an_override_says_what_its_pool_launches_exactly_once() {
    for row in ROWS {
        let case = row.case;
        // One emulator per row, so what a row stored (or did not) is read against an account
        // only that row wrote to.
        let (router, engine) = emulator();
        let template_id = launch_template(case, &router).await;

        let mut params = row.api.base(&template_id);
        for (position, entry) in (1..).zip(row.overrides) {
            let prefix = row.api.override_prefix(position);
            if let Some(instance_type) = entry.instance_type {
                params.push(p(&format!("{prefix}.InstanceType"), instance_type));
            }
            for (member, value) in entry.requirements {
                params.push(p(
                    &format!("{prefix}.InstanceRequirements.{member}"),
                    *value,
                ));
            }
        }

        let before = store_counts(&engine);
        let (status, body) = call(&router, REGION, row.api.action(), &params).await;
        match row.refusal {
            Some((expected_status, expected_code)) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status} from {}, got {status}: \
                     {body}",
                    row.api.action()
                );
                let code = error_code(&body);
                assert_eq!(
                    code, expected_code,
                    "case {case:?}: expected the error code {expected_code:?}, got {code:?} \
                     in {body}"
                );
                if expected_code == INVALID_PARAMETER_COMBINATION_EXCEPTION {
                    let message = error_message(&body);
                    assert_eq!(
                        message, INSTANCE_REQUIREMENTS_WITH_INSTANCE_TYPE_MESSAGE,
                        "case {case:?}: expected the sentence \
                         {INSTANCE_REQUIREMENTS_WITH_INSTANCE_TYPE_MESSAGE:?}, got {message:?}"
                    );
                }
                let after = store_counts(&engine);
                assert_eq!(
                    after, before,
                    "case {case:?}: a refused fleet request stores nothing, got {after:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 from {}, got {status}: {body}",
                    row.api.action()
                );
                let after = store_counts(&engine);
                assert_ne!(
                    after, before,
                    "case {case:?}: an accepted fleet request places capacity, got {after:?}"
                );
            }
        }
    }
}
