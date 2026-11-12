// SPDX-License-Identifier: CC0-1.0

//! TARGETS RESOLVE AGAINST THE RESOURCES THEY IDENTIFY, end to end over HTTP, across the
//! services that own those resources.
//!
//! RULE: an experiment's target resolves to the resources it IDENTIFIES — the ARNs it names
//! (`resourceArns`), or every resource of its `resourceType` in the account and Region that
//! carries each tag of its `resourceTags` — narrowed to those whose attribute at each
//! `filters` entry's `path` holds one of that entry's `values`, and then scoped by its
//! `selectionMode`; `ListExperimentResolvedTargets` reports one entry per resolved resource,
//! naming the target it came from, that target's `resourceType` and the resource's ARN.
//!
//! The resources of a tag-identified target belong to ANOTHER service, so they are read out
//! of that service's own store in the SAME shared engine — which is why this suite mounts
//! EC2 beside FIS, exactly as the assembled emulator does, launches its instances through
//! the real `ec2:RunInstances` and tags them through the real `ec2:CreateTags`, and seeds
//! the account's IAM roles the way `iam:CreateRole` writes them.
//!
//! The rows differ along what the rule generalises over: how a target identifies its
//! resources (by one tag pair, by two, by ARN), how many resources carry what it describes
//! (three, two, one, none), whether a filter narrows them (on an attribute one value of
//! which several resources hold, on one value none holds, on a path no member of the
//! resource names, with two admissible values), which `selectionMode` scopes them (`ALL`,
//! `COUNT(n)`, `PERCENT(n)`), which resource type — and so which owning store and which
//! record of it — the target names (an EC2 instance, a Spot Instance, an EC2 subnet, an IAM
//! role), and whether the resource is in the Region the experiment was started in.
//!
//! A tagged instance in ANOTHER Region, a tagged subnet in another Region, an instance
//! carrying no tag at all and a role whose tag value differs are launched or seeded beside
//! the rest: a resolution that matched everything would report them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_ec2_amazonec2::ops::types::{Subnet, Tag as Ec2Tag};
use service_com_amazonaws_fis_faultinjectionsimulator::state::IamState;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::{Role, Tag as IamTag};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn as iam_role_arn, DEFAULT_PATH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, poll_until_final, send, start_experiment, token, Answer, FALLBACK_REGION,
};

/// The Region the experiment is started in, and the one its resources have to be in.
const REGION: &str = FALLBACK_REGION;

/// A second Region, whose resources the experiment above must NOT resolve.
const OTHER_REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries (`::engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// A role ARN the emulator's IAM store holds no role for: with no stored role there is
/// nothing to show about its trust policy or its permissions, so every start goes ahead.
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-target-resolution";

/// The EC2 API version an ec2Query body carries.
const EC2_API_VERSION: &str = "2016-11-15";

/// The `targetInformation` key a resolved resource's ARN is reported under. Spelled out
/// here so a change to it fails this suite.
const TARGET_INFORMATION_ARN_KEY: &str = "arn";

/// How long a run is given to reach a terminal status: the emulator's whole compressed run
/// window plus the transitions around it and generous slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(30);

/// One emulator carrying FIS and EC2 over ONE shared engine, plus that engine: the
/// resolution reads the EC2 and IAM stores that live in the SAME engine, so the suite seeds
/// the AMI a launch needs and the account's roles there.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One EC2 call (ec2Query: a form POST answered with XML), signed for `region`.
async fn ec2_call(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(value);
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/ec2/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(::bytes::Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(&response.into_body()).to_string(),
    )
}

/// A `(key, value)` ec2Query form field.
fn field(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The text of the first `<name>…</name>` element of `xml`.
fn xml_element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// Launch ONE instance of `instance_type` in `region` and tag it with `tags`, through the
/// real EC2 operations, and answer the ARN of the instance EC2 minted.
async fn launch_tagged(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    image_id: &str,
    instance_type: &str,
    tags: &[(&str, &str)],
) -> String {
    let (status, body) = ec2_call(
        router,
        region,
        "RunInstances",
        &[
            field("ImageId", image_id),
            field("InstanceType", instance_type),
            field("MinCount", "1"),
            field("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances must succeed, got {status}: {body}"
    );
    let instance_id = xml_element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: no instanceId in {body}"));
    if !tags.is_empty() {
        let mut params = vec![field("ResourceId.1", instance_id.clone())];
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(field(&format!("Tag.{}.Key", index + 1), *key));
            params.push(field(&format!("Tag.{}.Value", index + 1), *value));
        }
        let (status, body) = ec2_call(router, region, "CreateTags", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateTags on {instance_id} must succeed, got {status}: {body}"
        );
    }
    format!("arn:aws:ec2:{region}:{ACCOUNT_ID}:instance/{instance_id}")
}

/// Seed one EC2 subnet of `region` into the account's EC2 store, carrying `tags` — the
/// record `CreateSubnet` writes — and answer the ARN EC2 reports for it.
///
/// A subnet is written into the store directly rather than created over the wire because
/// what this suite is about is the RESOLUTION reading another service's records, and a
/// subnet is one of them: `Subnet$SubnetArn`, `Subnet$Tags` and the members a filter path
/// names are what a describe of it reports.
fn seed_subnet(
    engine: &Arc<engine::InMemoryEngine>,
    subnet_id: &str,
    region: &str,
    tags: &[(&str, &str)],
) -> String {
    let state = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
    let mut store = state.lock();
    let arn = format!("arn:aws:ec2:{region}:{ACCOUNT_ID}:subnet/{subnet_id}");
    store.subnets.insert(
        subnet_id.to_string(),
        Subnet {
            subnet_id: Some(subnet_id.to_string()),
            subnet_arn: Some(arn.clone()),
            availability_zone: Some(format!("{region}a")),
            state: Some("available".to_string()),
            vpc_id: Some("vpc-00000000000000001".to_string()),
            cidr_block: Some("10.0.1.0/24".to_string()),
            tags: Some(
                tags.iter()
                    .map(|(key, value)| Ec2Tag {
                        key: Some((*key).to_string()),
                        value: Some((*value).to_string()),
                    })
                    .collect(),
            ),
            ..Subnet::default()
        },
    );
    arn
}

/// Make the instance `instance_arn` names a SPOT instance, by recording the Spot Instance
/// request it was launched to fulfill — "if the request is a Spot Instance request, the ID
/// of the request" (`Instance$SpotInstanceRequestId`), which is what makes EC2 report its
/// `InstanceLifecycle` as the Spot one.
fn make_spot(engine: &Arc<engine::InMemoryEngine>, instance_arn: &str) {
    let instance_id = instance_arn
        .rsplit('/')
        .next()
        .expect("the ARN names the instance")
        .to_string();
    let state = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
    let mut store = state.lock();
    store
        .instances
        .get_mut(&instance_id)
        .expect("the launched instance is in the EC2 store")
        .spot_instance_request_id = Some("sir-00000000000000001".to_string());
}

/// Seed `role_name` into the account's IAM store carrying `tags` — the record `CreateRole`
/// writes — and answer the ARN it is held under.
fn seed_role(engine: &Arc<engine::InMemoryEngine>, role_name: &str, tags: &[(&str, &str)]) -> String {
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    let arn = iam_role_arn(DEFAULT_PATH, role_name);
    store.roles.insert(
        name_key(role_name),
        Role {
            path: DEFAULT_PATH.to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            assume_role_policy_document: None,
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: Some(
                tags.iter()
                    .map(|(key, value)| IamTag {
                        key: (*key).to_string(),
                        value: (*value).to_string(),
                    })
                    .collect(),
            ),
            role_last_used: None,
            source_role_template: None,
        },
    );
    arn
}

/// One row: a target of the experiment template, and the resources it has to resolve to.
struct Row {
    case: &'static str,
    /// The target's name — the `targetName` every entry resolved from it carries.
    name: &'static str,
    /// `resourceType`: which owning store the target's resources come from.
    resource_type: &'static str,
    /// The ARNs the target NAMES, for a row that identifies its resources that way.
    resource_arns: Vec<String>,
    /// The `resourceTags` the target identifies its resources by, for a row that does.
    resource_tags: Vec<(&'static str, &'static str)>,
    /// The target's `filters`, as `(path, values)` pairs.
    filters: Vec<(&'static str, Vec<&'static str>)>,
    /// The target's `selectionMode`, in one of the three forms FIS documents.
    selection_mode: &'static str,
    /// The ARNs the target has to resolve to, in the order the resolution reports them.
    expected: Vec<String>,
}

impl Row {
    /// This row's target, as a template's `targets` map carries it.
    fn target(&self) -> Value {
        let mut target = Map::new();
        target.insert("resourceType".to_string(), json!(self.resource_type));
        target.insert("selectionMode".to_string(), json!(self.selection_mode));
        if !self.resource_arns.is_empty() {
            target.insert("resourceArns".to_string(), json!(self.resource_arns));
        }
        if !self.resource_tags.is_empty() {
            let tags: Map<String, Value> = self
                .resource_tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect();
            target.insert("resourceTags".to_string(), Value::Object(tags));
        }
        if !self.filters.is_empty() {
            let filters: Vec<Value> = self
                .filters
                .iter()
                .map(|(path, values)| json!({ "path": path, "values": values }))
                .collect();
            target.insert("filters".to_string(), json!(filters));
        }
        Value::Object(target)
    }
}

/// The ARNs the account's resources are seeded under, by the label this suite knows each
/// one by.
struct Seeded {
    /// `Stage=test`, `Team=chaos`, a `t3.micro` instance in the experiment's Region.
    test_chaos_micro: String,
    /// `Stage=test`, `Team=infra`, a `t3.micro` instance.
    test_infra_micro: String,
    /// `Stage=test`, `Team=chaos`, an `m5.large` instance — the same tags as the first row
    /// under a different attribute, which is what a filter tells apart.
    test_chaos_large: String,
    /// `Stage=prod`, `Team=chaos`: a tag value no `Stage=test` target describes.
    prod_chaos_micro: String,
    /// An instance carrying NO tag: described by no tag-identified target at all.
    untagged: String,
    /// `Stage=test`, `Team=chaos` — in ANOTHER Region.
    other_region: String,
    /// A SPOT instance, tagged `Stage=spot`: one of the account's instances, and the only
    /// one of them the Spot resource type's resources include.
    spot: String,
    /// An EC2 subnet tagged `Stage=net` — a second resource type in the same owning store.
    subnet: String,
    /// An EC2 subnet carrying the same tag in ANOTHER Region.
    other_region_subnet: String,
    /// An IAM role tagged `Stage=test`.
    test_role: String,
}

impl Seeded {
    /// The instances tagged `Stage=test` in the experiment's Region, in the order the
    /// resolution reports them (by ARN).
    fn stage_test_instances(&self) -> Vec<String> {
        let mut arns = vec![
            self.test_chaos_micro.clone(),
            self.test_infra_micro.clone(),
            self.test_chaos_large.clone(),
        ];
        arns.sort();
        arns
    }

    /// Those of them whose `InstanceType` is `t3.micro`, in the same order.
    fn stage_test_micro_instances(&self) -> Vec<String> {
        let mut arns = vec![
            self.test_chaos_micro.clone(),
            self.test_infra_micro.clone(),
        ];
        arns.sort();
        arns
    }

    /// The instances tagged `Stage=test` AND `Team=chaos` in the experiment's Region, in
    /// the same order.
    fn stage_test_team_chaos_instances(&self) -> Vec<String> {
        let mut arns = vec![self.test_chaos_micro.clone(), self.test_chaos_large.clone()];
        arns.sort();
        arns
    }
}

/// Launch and seed the account's resources, each tagged so that the rows below tell them
/// apart.
async fn seed(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    image_id: &str,
) -> Seeded {
    Seeded {
        test_chaos_micro: launch_tagged(
            router,
            "seed",
            REGION,
            image_id,
            "t3.micro",
            &[("Stage", "test"), ("Team", "chaos")],
        )
        .await,
        test_infra_micro: launch_tagged(
            router,
            "seed",
            REGION,
            image_id,
            "t3.micro",
            &[("Stage", "test"), ("Team", "infra")],
        )
        .await,
        test_chaos_large: launch_tagged(
            router,
            "seed",
            REGION,
            image_id,
            "m5.large",
            &[("Stage", "test"), ("Team", "chaos")],
        )
        .await,
        prod_chaos_micro: launch_tagged(
            router,
            "seed",
            REGION,
            image_id,
            "t3.micro",
            &[("Stage", "prod"), ("Team", "chaos")],
        )
        .await,
        untagged: launch_tagged(router, "seed", REGION, image_id, "t3.micro", &[]).await,
        other_region: launch_tagged(
            router,
            "seed",
            OTHER_REGION,
            image_id,
            "t3.micro",
            &[("Stage", "test"), ("Team", "chaos")],
        )
        .await,
        test_role: seed_role(engine, "fis-resolution-test", &[("Stage", "test")]),
        spot: {
            let arn = launch_tagged(
                router,
                "seed",
                REGION,
                image_id,
                "t3.micro",
                &[("Stage", "spot")],
            )
            .await;
            make_spot(engine, &arn);
            arn
        },
        subnet: seed_subnet(
            engine,
            "subnet-00000000000000001",
            REGION,
            &[("Stage", "net")],
        ),
        other_region_subnet: seed_subnet(
            engine,
            "subnet-00000000000000002",
            OTHER_REGION,
            &[("Stage", "net")],
        ),
    }
}

/// The targets the resolution rule is asserted over.
fn rows(seeded: &Seeded) -> Vec<Row> {
    let tagged = seeded.stage_test_instances();
    vec![
        Row {
            case: "one tag pair identifies every instance that carries it",
            name: "stageTest",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: tagged.clone(),
        },
        Row {
            case: "two tag pairs identify only the instances carrying both",
            name: "stageTestTeamChaos",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test"), ("Team", "chaos")],
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: seeded.stage_test_team_chaos_instances(),
        },
        Row {
            case: "a tag value no resource carries identifies nothing",
            name: "stageStaging",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "staging")],
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: Vec::new(),
        },
        Row {
            case: "a filter narrows the tagged instances to one attribute value",
            name: "stageTestMicro",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            // `Instance$InstanceType` is a member of the instance `DescribeInstances`
            // reports, which is where a filter path names an attribute.
            filters: vec![("InstanceType", vec!["t3.micro"])],
            selection_mode: "ALL",
            expected: seeded.stage_test_micro_instances(),
        },
        Row {
            case: "a filter admits a resource holding either of its values",
            name: "stageTestEitherType",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: vec![("InstanceType", vec!["m5.large", "t3.micro"])],
            selection_mode: "ALL",
            expected: tagged.clone(),
        },
        Row {
            case: "a filter value no identified resource holds narrows to nothing",
            name: "stageTestOtherType",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: vec![("InstanceType", vec!["c6g.medium"])],
            selection_mode: "ALL",
            expected: Vec::new(),
        },
        Row {
            case: "a filter path naming no attribute of the resource narrows to nothing",
            name: "stageTestUnknownPath",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: vec![("Nowhere.InThisShape", vec!["t3.micro"])],
            selection_mode: "ALL",
            expected: Vec::new(),
        },
        Row {
            case: "two filters both have to admit the resource",
            name: "stageTestTwoFilters",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: vec![
                ("InstanceType", vec!["t3.micro"]),
                ("Placement.AvailabilityZone", vec!["eu-central-1a"]),
            ],
            selection_mode: "ALL",
            expected: Vec::new(),
        },
        Row {
            case: "COUNT scopes the tag-identified resources",
            name: "stageTestCountTwo",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: Vec::new(),
            selection_mode: "COUNT(2)",
            expected: tagged.iter().take(2).cloned().collect(),
        },
        Row {
            case: "PERCENT scopes the tag-identified resources",
            name: "stageTestPercent",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: Vec::new(),
            // 50% of the three identified instances does not reach a second whole
            // resource, so one of them is kept.
            selection_mode: "PERCENT(50)",
            expected: tagged.iter().take(1).cloned().collect(),
        },
        Row {
            case: "another owning store: an IAM role identified by tag",
            name: "taggedRole",
            resource_type: "aws:iam:role",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: vec![seeded.test_role.clone()],
        },
        Row {
            case: "a role tag value no role carries identifies nothing",
            name: "untaggedRole",
            resource_type: "aws:iam:role",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "prod")],
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: Vec::new(),
        },
        Row {
            case: "a target that NAMES its resources resolves to the ones it named",
            name: "namedArns",
            resource_type: "aws:ec2:instance",
            resource_arns: vec![seeded.untagged.clone(), seeded.prod_chaos_micro.clone()],
            resource_tags: Vec::new(),
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: vec![seeded.untagged.clone(), seeded.prod_chaos_micro.clone()],
        },
        Row {
            case: "a filter narrows the resources a target NAMED",
            name: "namedArnsFiltered",
            resource_type: "aws:ec2:instance",
            resource_arns: vec![
                seeded.test_chaos_large.clone(),
                seeded.test_infra_micro.clone(),
            ],
            resource_tags: Vec::new(),
            filters: vec![("InstanceType", vec!["t3.micro"])],
            selection_mode: "ALL",
            expected: vec![seeded.test_infra_micro.clone()],
        },
        Row {
            case: "a NAMED resource this emulator holds no record of is still identified",
            name: "namedUnknown",
            resource_type: "aws:ec2:instance",
            resource_arns: vec![format!(
                "arn:aws:ec2:{REGION}:{ACCOUNT_ID}:instance/i-00000000000000fff"
            )],
            resource_tags: Vec::new(),
            filters: vec![("InstanceType", vec!["t3.micro"])],
            selection_mode: "ALL",
            expected: vec![format!(
                "arn:aws:ec2:{REGION}:{ACCOUNT_ID}:instance/i-00000000000000fff"
            )],
        },
        Row {
            case: "a tagged resource in another Region is not resolved",
            name: "stageTestOtherRegion",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test"), ("Team", "chaos")],
            // The other Region's instance carries these tags and this instance type, so a
            // resolution that ignored the Region would report it here.
            filters: vec![("InstanceType", vec!["t3.micro"])],
            selection_mode: "ALL",
            expected: vec![seeded.test_chaos_micro.clone()],
        },
        Row {
            case: "a second resource type of the same owning store: a subnet",
            name: "taggedSubnet",
            resource_type: "aws:ec2:subnet",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "net")],
            // `Subnet$State` is a member of the subnet a describe of it reports. The other
            // Region's subnet carries the same tag and the same state, so a resolution that
            // ignored the Region would report it here too.
            filters: vec![("State", vec!["available"])],
            selection_mode: "ALL",
            expected: vec![seeded.subnet.clone()],
        },
        Row {
            case: "the Spot resource type's resources are the Spot instances",
            name: "spotInstances",
            resource_type: "aws:ec2:spot-instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "spot")],
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: vec![seeded.spot.clone()],
        },
        Row {
            case: "a Spot instance is one of the account's instances too",
            name: "spotIsAnInstance",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "spot")],
            // `Instance$InstanceLifecycle` is what says a launch was a Spot one.
            filters: vec![("InstanceLifecycle", vec!["spot"])],
            selection_mode: "ALL",
            expected: vec![seeded.spot.clone()],
        },
        Row {
            case: "the Spot resource type holds no on-demand instance",
            name: "spotOnDemand",
            resource_type: "aws:ec2:spot-instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", "test")],
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: Vec::new(),
        },
    ]
}

/// The template body holding every row's target, plus one action so the template is
/// admissible.
fn template_body(label: &str, rows: &[Row]) -> Value {
    let mut targets = Map::new();
    for row in rows {
        targets.insert(row.name.to_string(), row.target());
    }
    json!({
        "clientToken": token(&format!("target-resolution-{label}")),
        "description": format!("targets resolve against tagged resources: {label}"),
        "roleArn": EXECUTION_ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": Value::Object(targets),
        // `aws:fis:wait` takes a required `duration`, an ISO 8601 duration within the
        // documented 1 to 720 minutes.
        "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT5M" } } }
    })
}

/// Create a template holding `rows` as its targets and start an experiment from it,
/// answering the experiment's minted id.
async fn experiment(router: &MultiServiceRouter, label: &str, rows: &[Row]) -> String {
    // `ClientToken` is `pattern ^[\S]+$`, so a case label carrying spaces is slugged
    // before it goes into one.
    let label = &label.replace(' ', "-");
    let created = create_template(router, template_body(label, rows)).await;
    assert_eq!(
        created.status, 200,
        "expected the template to be created, got {} with {:?}",
        created.status, created.body
    );
    let template_id = created.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no template id in {:?}", created.body))
        .to_string();
    let started = start_experiment(
        router,
        json!({
            "clientToken": token(&format!("target-resolution-start-{label}")),
            "experimentTemplateId": template_id
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "expected the start to be accepted, got {} with {:?}",
        started.status, started.body
    );
    started.member("experiment")["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no experiment id in {:?}", started.body))
        .to_string()
}

/// One page of `ListExperimentResolvedTargets`, optionally narrowed to one `targetName`.
async fn resolved(
    router: &MultiServiceRouter,
    experiment_id: &str,
    target_name: Option<&str>,
) -> Vec<Value> {
    let uri = match target_name {
        Some(name) => format!("/experiments/{experiment_id}/resolvedTargets?targetName={name}"),
        None => format!("/experiments/{experiment_id}/resolvedTargets"),
    };
    let answer: Answer = send(router, "GET", &uri, None, REGION).await;
    assert_eq!(
        answer.status, 200,
        "ListExperimentResolvedTargets must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .member("resolvedTargets")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The ARN one resolved entry reports.
fn entry_arn(case: &str, entry: &Value) -> String {
    entry["targetInformation"][TARGET_INFORMATION_ARN_KEY]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no resolved ARN in {entry:?}"))
        .to_string()
}

/// RULE: a target resolves to the resources it identifies — the ARNs it names, or the
/// resources of its `resourceType` carrying every tag of its `resourceTags` — narrowed by
/// its `filters` and scoped by its `selectionMode`.
#[tokio::test]
async fn a_target_resolves_to_the_resources_it_identifies() {
    let (router, engine) = emulator();
    let image_id = {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        guard.insert_image(ec2::state::ImageSpec {
            name: "fis-target-resolution".to_string(),
            ..ec2::state::ImageSpec::default()
        })
    };
    let seeded = seed(&router, &engine, &image_id).await;
    let rows = rows(&seeded);
    let experiment_id = experiment(&router, "table", &rows).await;

    for row in &rows {
        let case = row.case;
        let entries = resolved(&router, &experiment_id, Some(row.name)).await;
        let arns: Vec<String> = entries
            .iter()
            .map(|entry| entry_arn(case, entry))
            .collect();
        assert_eq!(
            arns, row.expected,
            "case {case:?}: expected the target {:?} to resolve to {:?}, got {arns:?}",
            row.name, row.expected
        );
        for entry in &entries {
            assert_eq!(
                entry["targetName"].as_str(),
                Some(row.name),
                "case {case:?}: every entry must name the target it came from, got {entry:?}"
            );
            assert_eq!(
                entry["resourceType"].as_str(),
                Some(row.resource_type),
                "case {case:?}: every entry must carry the target's resourceType, \
                 got {entry:?}"
            );
        }
    }

    // The whole resolution is the rows' resolutions in target-name order, and reading it
    // again reports the same page: the resolution is a snapshot the start computed, not a
    // fresh lookup per read.
    let mut by_name: Vec<&Row> = rows.iter().collect();
    by_name.sort_by_key(|row| row.name);
    let expected_all: Vec<String> = by_name
        .iter()
        .flat_map(|row| row.expected.clone())
        .collect();
    for pass in ["first read", "second read"] {
        let entries = resolved(&router, &experiment_id, None).await;
        let arns: Vec<String> = entries
            .iter()
            .map(|entry| entry_arn(pass, entry))
            .collect();
        assert_eq!(
            arns, expected_all,
            "case {pass:?}: expected the whole resolution in target-name order, got {arns:?}"
        );
    }

    // Nothing the resolution left out is in it: the untagged instance, the one in the other
    // Region and the `Stage=prod` instance only appear where a target named them.
    let reported: Vec<String> = resolved(&router, &experiment_id, None)
        .await
        .iter()
        .map(|entry| entry_arn("bystanders", entry))
        .collect();
    assert!(
        !reported.contains(&seeded.other_region),
        "case \"bystanders\": an instance in another Region must not resolve, got {reported:?}"
    );
    let tag_identified: Vec<String> = resolved(&router, &experiment_id, Some("stageTest"))
        .await
        .iter()
        .map(|entry| entry_arn("bystanders", entry))
        .collect();
    for (label, arn) in [
        ("the untagged instance", &seeded.untagged),
        ("the Stage=prod instance", &seeded.prod_chaos_micro),
        ("the other Region's instance", &seeded.other_region),
        ("the Spot instance", &seeded.spot),
    ] {
        assert!(
            !tag_identified.contains(arn),
            "case \"bystanders\": {label} is not described by Stage=test, \
             got {tag_identified:?}"
        );
    }
    let subnets = resolved(&router, &experiment_id, Some("taggedSubnet")).await;
    let subnet_arns: Vec<String> = subnets
        .iter()
        .map(|entry| entry_arn("bystanders", entry))
        .collect();
    assert!(
        !subnet_arns.contains(&seeded.other_region_subnet),
        "case \"bystanders\": a subnet in another Region must not resolve, got {subnet_arns:?}"
    );
}

/// RULE (the consequence of the same resolution): `emptyTargetResolutionMode` decides the
/// run on what the targets RESOLVED to — a tag-identified target that no resource of the
/// account carries the tags of resolves to nothing, which under `fail` ends the run
/// `failed` on that target, while one that resolves to a resource leaves the run to go
/// ahead.
#[tokio::test]
async fn the_resolution_decides_the_run_under_the_empty_target_resolution_mode() {
    let (router, engine) = emulator();
    let image_id = {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        guard.insert_image(ec2::state::ImageSpec {
            name: "fis-empty-resolution".to_string(),
            ..ec2::state::ImageSpec::default()
        })
    };
    let resolving = launch_tagged(
        &router,
        "resolves",
        REGION,
        &image_id,
        "t3.micro",
        &[("Stage", "test")],
    )
    .await;

    // The rows differ in the one thing the mode decides on: whether the target resolved to
    // a resource. Both carry the same mode, the documented default `fail`.
    for (case, tag_value, resolves) in [
        ("a target that resolved to a resource", "test", true),
        ("a target that resolved to nothing", "staging", false),
    ] {
        let row = Row {
            case,
            name: "tagged",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: vec![("Stage", tag_value)],
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: if resolves {
                vec![resolving.clone()]
            } else {
                Vec::new()
            },
        };
        let experiment_id = experiment(&router, case, std::slice::from_ref(&row)).await;
        let arns: Vec<String> = resolved(&router, &experiment_id, None)
            .await
            .iter()
            .map(|entry| entry_arn(case, entry))
            .collect();
        assert_eq!(
            arns, row.expected,
            "case {case:?}: expected the resolution {:?}, got {arns:?}",
            row.expected
        );

        let ended = poll_until_final(&router, &experiment_id, TERMINAL_BUDGET).await;
        let state = ended["state"].clone();
        if resolves {
            assert_eq!(
                state["status"].as_str(),
                Some("completed"),
                "case {case:?}: expected a run whose target resolved to run to completion, \
                 got {state:?}"
            );
        } else {
            assert_eq!(
                state["status"].as_str(),
                Some("failed"),
                "case {case:?}: expected the empty resolution to fail the run, got {state:?}"
            );
            assert_eq!(
                state["error"]["location"].as_str(),
                Some("targets.tagged"),
                "case {case:?}: expected the error to locate the target that resolved to \
                 nothing, got {state:?}"
            );
        }
    }
}

/// RULE (read directly off the resolution the start computed): the tags a resource carries
/// are the ones its owning service holds for it, so a target's `resourceTags` describe the
/// very resource `ec2:CreateTags` wrote the tags onto.
#[tokio::test]
async fn the_tags_a_target_matches_are_the_ones_the_owning_service_holds() {
    let (router, engine) = emulator();
    let image_id = {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        guard.insert_image(ec2::state::ImageSpec {
            name: "fis-owning-store".to_string(),
            ..ec2::state::ImageSpec::default()
        })
    };
    let arn = launch_tagged(
        &router,
        "owning store",
        REGION,
        &image_id,
        "t3.micro",
        &[("Stage", "test"), ("Team", "chaos")],
    )
    .await;
    let instance_id = arn
        .rsplit('/')
        .next()
        .expect("the ARN names the instance")
        .to_string();

    let stored: BTreeMap<String, String> = {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let guard = store.lock();
        guard
            .instances
            .get(&instance_id)
            .expect("the launched instance is in the EC2 store")
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    };
    assert_eq!(
        stored,
        BTreeMap::from([
            ("Stage".to_string(), "test".to_string()),
            ("Team".to_string(), "chaos".to_string()),
        ]),
        "the EC2 store must hold the tags CreateTags wrote, got {stored:?}"
    );

    // Every tag pair the instance carries describes it, and one it does not carry describes
    // nothing — read through the resolution a start computed.
    for (case, tags, expected) in [
        ("one tag it carries", vec![("Stage", "test")], vec![arn.clone()]),
        (
            "both tags it carries",
            vec![("Stage", "test"), ("Team", "chaos")],
            vec![arn.clone()],
        ),
        (
            "a tag key it does not carry",
            vec![("Owner", "chaos")],
            Vec::new(),
        ),
        (
            "one tag it carries beside one it does not",
            vec![("Stage", "test"), ("Owner", "chaos")],
            Vec::new(),
        ),
    ] {
        let row = Row {
            case,
            name: "tagged",
            resource_type: "aws:ec2:instance",
            resource_arns: Vec::new(),
            resource_tags: tags,
            filters: Vec::new(),
            selection_mode: "ALL",
            expected: expected.clone(),
        };
        let experiment_id = experiment(&router, case, std::slice::from_ref(&row)).await;
        let arns: Vec<String> = resolved(&router, &experiment_id, None)
            .await
            .iter()
            .map(|entry| entry_arn(case, entry))
            .collect();
        assert_eq!(
            arns, expected,
            "case {case:?}: expected the resolution {expected:?}, got {arns:?}"
        );
    }
}
