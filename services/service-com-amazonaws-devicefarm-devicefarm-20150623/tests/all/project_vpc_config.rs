// SPDX-License-Identifier: CC0-1.0

//! The EC2 resources a project's `vpcConfig` names are resolved against Amazon EC2 — the
//! service that owns them — before any project state is written.
//!
//! RULE: `vpcConfig`'s three required members name resources of another service (an Amazon
//! VPC, its subnets, its security groups), so a `CreateProject` or `UpdateProject` carrying
//! one resolves every id it names against EC2 at call time; a request naming an id EC2
//! answers no such resource for is refused with the modelled `ArgumentException` (HTTP
//! 400) relaying what EC2 answered — EC2's own sentence, which names the offending id,
//! followed by the service, the status, the error code and the request id of the call
//! Device Farm made on the caller's behalf — and writes NOTHING, so the project keeps the
//! configuration it already had.
//!
//! The other half of the rule: a refusal is GROUNDED in an answer. Device Farm only gates
//! on existence here, keeping nothing of the network it resolved, so an emulator with no
//! EC2 mounted learns nothing to refuse a request on and serves the same call.
//!
//! EC2 is mounted beside Device Farm on ONE engine and every VPC, subnet and group an
//! accepted row uses exists only because EC2 minted it, so the resolution under test runs
//! over the same in-process callback as in production and every refusal is EC2's own
//! answer.
//!
//! The rows differ along what the rule generalises over: WHICH member carries the
//! unresolvable value, HOW MANY ids the request names (one, several, and the modelled
//! maxima of 8 subnets and 5 security groups), WHERE in a list the unresolvable id sits,
//! the FORM of the value (EC2's 17-hex id form, its older 8-hex form, upper-case hex, and
//! an id of the WRONG KIND that EC2 really minted), and which operation — the create or
//! the update — carries it.
//!
//! Error source: `smithy_query(query="operation", operation="CreateProject,UpdateProject")`
//! on the devicefarm model — `ArgumentException`, `errorKind` client, `httpError` 400; and
//! `smithy_query(query="shape", shape="VpcConfig")` for the three required members and the
//! `@length` bounds on `subnetIds` (1..8) and `securityGroupIds` (1..5).

use serde_json::{json, Value};

use crate::projects::{error_code, Emulator};

/// The region every request below is signed for — the one the EC2 client here answers for,
/// so the resources an accepted row names are the ones Device Farm resolves. It is the
/// region [`crate::projects::REGION`] names, which is what the rest of this crate's suites
/// sign for.
const REGION: &str = "us-west-2";

/// The service a relayed refusal names as the one that answered the read.
const EC2_SERVICE: &str = "AmazonEC2";

/// The status EC2 answers a client refusal with, which the relayed clause states.
const EC2_REFUSAL_STATUS: u16 = 400;

/// The EC2 error code for a subnet id the account holds no subnet for.
const SUBNET_NOT_FOUND: &str = "InvalidSubnetID.NotFound";

/// The EC2 error code for a security group id the account holds no group for.
const GROUP_NOT_FOUND: &str = "InvalidGroup.NotFound";

/// The EC2 error code for a VPC id the account holds no VPC for.
const VPC_NOT_FOUND: &str = "InvalidVpcID.NotFound";

/// The modelled maximum number of subnet ids one `vpcConfig` carries (`VpcSubnetIds`,
/// `@length` 1..8).
const MAX_SUBNET_IDS: usize = 8;

/// The modelled maximum number of security group ids one `vpcConfig` carries
/// (`VpcSecurityGroupIds`, `@length` 1..5).
const MAX_SECURITY_GROUP_IDS: usize = 5;

/// Device Farm mounted beside the OWNER of the resources its `vpcConfig` names, on one
/// shared engine ([`crate::projects::Emulator::with_ec2`], this crate's one wire harness),
/// plus an EC2 client over that router's own in-process callback — the very seam the
/// resolution reaches EC2 through.
struct Wired {
    emulator: Emulator,
    ec2: ::aws_sdk_ec2::Client,
}

impl Wired {
    fn new() -> Self {
        let (emulator, callback) = Emulator::with_ec2();
        let ec2 = ::aws_sdk_ec2::Client::from_conf(
            ::aws_sdk_ec2::Config::builder()
                .behavior_version(::aws_sdk_ec2::config::BehaviorVersion::latest())
                .region(::aws_sdk_ec2::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://ec2.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_ec2::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_ec2::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_ec2::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_ec2::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        Self { emulator, ec2 }
    }

    /// Call one Device Farm operation over its awsJson1_1 wire, signed for [`REGION`].
    async fn call(&self, op: &str, body: &Value) -> (u16, Value) {
        self.emulator.call(op, Some(REGION), body).await
    }

    /// One VPC of the account, with `subnets` subnets and `groups` security groups in it —
    /// all minted by EC2's own operations, so every id below is one EC2 handed out.
    async fn network(&self, second_octet: u8, subnets: usize, groups: usize) -> Network {
        let vpc_id = self
            .ec2
            .create_vpc()
            .cidr_block(format!("10.{second_octet}.0.0/16"))
            .send()
            .await
            .expect("setup: EC2 must create the VPC")
            .vpc()
            .and_then(|vpc| vpc.vpc_id())
            .expect("setup: EC2 must report the VPC's id")
            .to_string();
        let mut subnet_ids = Vec::new();
        for index in 0..subnets {
            subnet_ids.push(
                self.ec2
                    .create_subnet()
                    .vpc_id(&vpc_id)
                    .cidr_block(format!("10.{second_octet}.{}.0/24", index + 1))
                    .availability_zone(format!("{REGION}a"))
                    .send()
                    .await
                    .expect("setup: EC2 must create the subnet")
                    .subnet()
                    .and_then(|subnet| subnet.subnet_id())
                    .expect("setup: EC2 must report the subnet's id")
                    .to_string(),
            );
        }
        let mut group_ids = Vec::new();
        for index in 0..groups {
            group_ids.push(
                self.ec2
                    .create_security_group()
                    .vpc_id(&vpc_id)
                    .group_name(format!("devicefarm-{second_octet}-{index}"))
                    .description("a group named by a project vpcConfig")
                    .send()
                    .await
                    .expect("setup: EC2 must create the security group")
                    .group_id()
                    .expect("setup: EC2 must report the group's id")
                    .to_string(),
            );
        }
        Network {
            vpc_id,
            subnet_ids,
            group_ids,
        }
    }
}

/// One VPC of the account as EC2 minted it.
struct Network {
    vpc_id: String,
    subnet_ids: Vec<String>,
    group_ids: Vec<String>,
}

/// A `vpcConfig` naming `security_groups`, `subnets` and `vpc`.
fn vpc_config(security_groups: &[String], subnets: &[String], vpc: &str) -> Value {
    json!({
        "securityGroupIds": security_groups,
        "subnetIds": subnets,
        "vpcId": vpc,
    })
}

/// The message a Device Farm refusal relays an EC2 fault in, with the per-call request id
/// left for the caller to check: the sentence EC2 worded and the context of the call it
/// came from.
///
/// A literal on purpose: this suite is what pins the wording, so it may not ask the
/// implementation what it is. The only variable parts are EC2's own sentence (which names
/// the id the request sent), the status and the error code EC2 answered.
fn relayed_head(sentence: &str, code: &str) -> String {
    format!(
        "{sentence} (Service: {EC2_SERVICE}; Status Code: {EC2_REFUSAL_STATUS}; \
         Error Code: {code}; Request ID: "
    )
}

/// EC2's own sentence for a subnet id it holds no subnet for.
fn subnet_sentence(subnet_id: &str) -> String {
    format!("The subnet ID '{subnet_id}' does not exist")
}

/// EC2's own sentence for a security group id it holds no group for.
fn group_sentence(group_id: &str) -> String {
    format!("The security group '{group_id}' does not exist")
}

/// EC2's own sentence for a VPC id it holds no VPC for.
fn vpc_sentence(vpc_id: &str) -> String {
    format!("The vpc ID '{vpc_id}' does not exist")
}

/// Assert that `response` is the modelled refusal relaying `sentence` under `code`, and
/// answer the request id the relayed clause named.
fn relayed_refusal(case: &str, status: u16, response: &Value, sentence: &str, code: &str) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {response}"
    );
    assert_eq!(
        error_code(response),
        "ArgumentException",
        "case {case:?}: expected ArgumentException, got {response}"
    );
    let message = response["message"].as_str().unwrap_or_default();
    let head = relayed_head(sentence, code);
    let Some(tail) = message.strip_prefix(head.as_str()) else {
        panic!("case {case:?}: expected the message {head:?}<request id>), got {message:?}");
    };
    let Some(request_id) = tail.strip_suffix(')') else {
        panic!("case {case:?}: expected the relayed clause to close with ')', got {message:?}");
    };
    assert!(
        ::service_common::query_error_envelope::is_request_id(request_id),
        "case {case:?}: expected an 8-4-4-4-12 request id, got {request_id:?}"
    );
    request_id.to_string()
}

/// The `vpcConfig` a `GetProject` reports for the project `arn` names.
async fn stored_config(wired: &Wired, case: &str, arn: &str) -> Value {
    let (status, response) = wired.call("GetProject", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetProject, got {status} with {response}"
    );
    response["project"]["vpcConfig"].clone()
}

/// How many projects the account holds, read through `ListProjects`.
async fn project_count(wired: &Wired, case: &str) -> usize {
    let (status, response) = wired.call("ListProjects", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListProjects, got {status} with {response}"
    );
    response["projects"].as_array().map_or(0, Vec::len)
}

/// Every configuration whose ids EC2 holds is accepted and read back; every one naming an
/// id EC2 resolves nothing for is refused with EC2's answer relayed, and leaves the stored
/// configuration untouched.
#[tokio::test]
async fn a_project_vpc_config_is_resolved_against_ec2_before_it_is_stored() {
    let wired = Wired::new();
    let first = wired
        .network(90, MAX_SUBNET_IDS, MAX_SECURITY_GROUP_IDS)
        .await;
    let second = wired.network(91, 2, 1).await;

    // The accepted side of the rule, over the counts the modelled bounds allow: each
    // configuration names what EC2 itself minted, so each is accepted — and the project
    // reads back carrying exactly it.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "one subnet and one group",
            vpc_config(
                &first.group_ids[..1],
                &first.subnet_ids[..1],
                &first.vpc_id,
            ),
        ),
        (
            "two subnets and two groups",
            vpc_config(
                &first.group_ids[..2],
                &first.subnet_ids[..2],
                &first.vpc_id,
            ),
        ),
        (
            "the modelled maxima: 8 subnets and 5 groups",
            vpc_config(&first.group_ids, &first.subnet_ids, &first.vpc_id),
        ),
        (
            "another VPC's own subnets and group",
            vpc_config(&second.group_ids, &second.subnet_ids, &second.vpc_id),
        ),
    ];

    for (case, config) in &accepted {
        let (status, response) = wired
            .call(
                "CreateProject",
                &json!({ "name": format!("created-with-{case}"), "vpcConfig": config }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a create naming EC2's own resources must be accepted, got {status} \
             with {response}"
        );
        let arn = response["project"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the create reported no arn: {response}"))
            .to_string();
        assert_eq!(
            &stored_config(&wired, case, &arn).await,
            config,
            "case {case:?}: expected GetProject to report the configuration the create stored"
        );
    }

    // The same configurations through the UPDATE path, on one project, each read back.
    let (status, response) = wired
        .call("CreateProject", &json!({ "name": "updated-in-place" }))
        .await;
    assert_eq!(
        status, 200,
        "setup: expected 200 from CreateProject, got {status} with {response}"
    );
    let project_arn = response["project"]["arn"]
        .as_str()
        .expect("setup: the create must report an arn")
        .to_string();
    for (case, config) in &accepted {
        let case = format!("UpdateProject with {case}");
        let (status, response) = wired
            .call(
                "UpdateProject",
                &json!({ "arn": project_arn, "vpcConfig": config }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an update naming EC2's own resources must be accepted, got {status} \
             with {response}"
        );
        assert_eq!(
            &stored_config(&wired, &case, &project_arn).await,
            config,
            "case {case:?}: expected GetProject to report the configuration the update stored"
        );
    }

    // The configuration the project now holds, which no refusal below may disturb.
    let held = stored_config(&wired, "the configuration before the refusals", &project_arn).await;
    let held_count = project_count(&wired, "the project count before the refusals").await;

    // The refusal side. One row per way a member names no resource of the account: each
    // row has exactly ONE unresolvable value, in one of the forms EC2 answers for, and
    // states the sentence and the code EC2 answers it with.
    //
    // (case, the vpcConfig, EC2's sentence, EC2's error code)
    let mut rows: Vec<(String, Value, String, &str)> = Vec::new();
    for (form, unknown) in [
        ("EC2's 17-hex id form", "subnet-0f1e2d3c4b5a69780"),
        ("EC2's older 8-hex id form", "subnet-1a2b3c4d"),
        (
            "UPPER-CASE hex, which no normalisation may fold",
            "subnet-ABCDEF0123456789A",
        ),
    ] {
        rows.push((
            format!("one subnet EC2 does not hold, in {form}"),
            vpc_config(
                &first.group_ids[..1],
                &[unknown.to_string()],
                &first.vpc_id,
            ),
            subnet_sentence(unknown),
            SUBNET_NOT_FOUND,
        ));
        rows.push((
            format!("the LAST of three subnets EC2 does not hold, in {form}"),
            vpc_config(
                &first.group_ids[..1],
                &[
                    first.subnet_ids[0].clone(),
                    first.subnet_ids[1].clone(),
                    unknown.to_string(),
                ],
                &first.vpc_id,
            ),
            subnet_sentence(unknown),
            SUBNET_NOT_FOUND,
        ));
        rows.push((
            format!("the FIRST of two subnets EC2 does not hold, in {form}"),
            vpc_config(
                &first.group_ids[..1],
                &[unknown.to_string(), first.subnet_ids[0].clone()],
                &first.vpc_id,
            ),
            subnet_sentence(unknown),
            SUBNET_NOT_FOUND,
        ));
    }
    for (form, unknown) in [
        ("EC2's 17-hex id form", "sg-0f1e2d3c4b5a69780"),
        ("EC2's older 8-hex id form", "sg-1a2b3c4d"),
        (
            "UPPER-CASE hex, which no normalisation may fold",
            "sg-ABCDEF0123456789A",
        ),
    ] {
        rows.push((
            format!("one security group EC2 does not hold, in {form}"),
            vpc_config(
                &[unknown.to_string()],
                &first.subnet_ids[..1],
                &first.vpc_id,
            ),
            group_sentence(unknown),
            GROUP_NOT_FOUND,
        ));
        rows.push((
            format!("the LAST of two security groups EC2 does not hold, in {form}"),
            vpc_config(
                &[first.group_ids[0].clone(), unknown.to_string()],
                &first.subnet_ids[..1],
                &first.vpc_id,
            ),
            group_sentence(unknown),
            GROUP_NOT_FOUND,
        ));
    }
    for (form, unknown) in [
        ("EC2's 17-hex id form", "vpc-0f1e2d3c4b5a69780"),
        ("EC2's older 8-hex id form", "vpc-1a2b3c4d"),
    ] {
        rows.push((
            format!("a VPC EC2 does not hold, in {form}"),
            vpc_config(&first.group_ids[..1], &first.subnet_ids[..1], unknown),
            vpc_sentence(unknown),
            VPC_NOT_FOUND,
        ));
    }
    // An id of the WRONG KIND that EC2 really minted: `vpcId` is modelled as a
    // `NonEmptyString`, so a subnet id reaches the handler there — and it names no VPC of
    // the account. (`subnetIds` and `securityGroupIds` are modelled with the patterns
    // `^subnet-[0-9a-fA-F]{8,}$` and `^sg-[0-9a-fA-F]{8,}$`, so a value of another kind is
    // refused by the modelled constraint before any handler sees it.)
    rows.push((
        "a real subnet id used as the VPC id".to_string(),
        vpc_config(
            &first.group_ids[..1],
            &first.subnet_ids[..1],
            &first.subnet_ids[0],
        ),
        vpc_sentence(&first.subnet_ids[0]),
        VPC_NOT_FOUND,
    ));

    let mut request_ids: Vec<String> = Vec::new();
    for (case, config, sentence, code) in &rows {
        // The UPDATE path: refused, and the stored configuration survives untouched.
        let update_case = format!("UpdateProject with {case}");
        let (status, response) = wired
            .call(
                "UpdateProject",
                &json!({ "arn": project_arn, "vpcConfig": config }),
            )
            .await;
        request_ids.push(relayed_refusal(
            &update_case,
            status,
            &response,
            sentence,
            code,
        ));
        assert_eq!(
            stored_config(&wired, &update_case, &project_arn).await,
            held,
            "case {update_case:?}: a refused update must leave the stored vpcConfig alone"
        );

        // The CREATE path: refused the same way, and no project is minted for it.
        let create_case = format!("CreateProject with {case}");
        let (status, response) = wired
            .call(
                "CreateProject",
                &json!({ "name": "refused", "vpcConfig": config }),
            )
            .await;
        request_ids.push(relayed_refusal(
            &create_case,
            status,
            &response,
            sentence,
            code,
        ));
        assert_eq!(
            project_count(&wired, &create_case).await,
            held_count,
            "case {create_case:?}: a refused create must mint no project"
        );
    }

    // One id per call: the relayed clause names the EC2 read that was refused, so no two
    // of these refusals carry the same id.
    let mut sorted = request_ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        request_ids.len(),
        "each relayed refusal must name its own EC2 call, got {request_ids:?}"
    );

    // And the emulator still serves an update naming EC2's own resources: the refusals
    // consumed nothing.
    let (status, response) = wired
        .call(
            "UpdateProject",
            &json!({ "arn": project_arn, "vpcConfig": accepted[0].1 }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a valid update after the refusals\": expected 200, got {status} with {response}"
    );
    assert_eq!(
        stored_config(&wired, "a valid update after the refusals", &project_arn).await,
        accepted[0].1,
        "case \"a valid update after the refusals\": expected the new configuration stored"
    );
}

/// With no EC2 to ask, this service learned nothing to refuse a request on: the same
/// configurations are served, because a refusal is grounded in what EC2 answered rather
/// than assumed from silence.
#[tokio::test]
async fn nothing_is_refused_where_there_was_no_ec2_to_ask() {
    let emu = crate::projects::Emulator::new();
    let config = vpc_config(
        &["sg-0f1e2d3c4b5a69780".to_string()],
        &["subnet-0f1e2d3c4b5a69780".to_string()],
        "vpc-0f1e2d3c4b5a69780",
    );

    let created = emu
        .create(&json!({ "name": "no-ec2-to-ask", "vpcConfig": config }))
        .await;
    let arn = created["arn"].as_str().expect("the create reports an arn");
    assert_eq!(
        created["vpcConfig"], config,
        "case \"a create with no EC2 mounted\": expected the configuration served, got {created}"
    );

    let (status, response) = emu
        .call(
            "UpdateProject",
            crate::projects::REGION,
            &json!({ "arn": arn, "vpcConfig": config }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"an update with no EC2 mounted\": expected 200, got {status} with {response}"
    );
    assert_eq!(
        response["project"]["vpcConfig"], config,
        "case \"an update with no EC2 mounted\": expected the configuration served, got {response}"
    );
}
