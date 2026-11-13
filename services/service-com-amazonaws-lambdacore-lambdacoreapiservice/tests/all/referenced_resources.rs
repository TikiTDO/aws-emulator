// SPDX-License-Identifier: CC0-1.0

//! The resources a network connector is WIRED TO are resolved against the services that
//! own them, synchronously, before anything is written.
//!
//! The rule under test: a member of a create or an update whose value names a resource of
//! ANOTHER service — an EC2 subnet, an EC2 security group, the IAM role Lambda assumes to
//! manage the connector's network interfaces — is resolved against that service at call
//! time, and a value the owner answers for as a resource this account cannot use is
//! refused with the modelled `InvalidParameterValueException` (HTTP 400) having written
//! nothing. The refusal carries what the owner answered: for an EC2 read, the call, the
//! EC2 error code and the EC2 sentence naming the identifier at fault.
//!
//! The other half of the rule: a refusal is GROUNDED in an answer. An emulator with no
//! EC2 and no IAM mounted knows nothing about a subnet or a role, and nothing is what it
//! refuses on — so the same request is served there.
//!
//! EC2 and IAM are mounted beside this service on ONE engine and every subnet, group and
//! role a row uses exists only because EC2 or IAM answered for it, so the resolution under
//! test runs over the same in-process callback as in production.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambdacore_lambdacoreapiservice::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::network_connector_lifecycle::{
    at, create, get, list, text, update, vpc_egress, wait_for_the_clock, OPERATOR_ROLE,
};

/// The region every case here signs for, which is the one
/// [`crate::network_connector_lifecycle::call`] signs. EC2 answers for the account's
/// default network in it, so the subnets and groups the rows use are real ones.
const REGION: &str = "eu-west-1";

/// The modelled error a request naming an unusable reference is refused with.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The service principal a connector's `OperatorRole` must trust: the role is the one
/// "Lambda assumes to manage elastic network interfaces in your VPC".
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// A service principal that is NOT Lambda, for the role that exists and trusts someone
/// else.
const OTHER_PRINCIPAL: &str = "states.amazonaws.com";

/// The EC2 actions a usable `OperatorRole` grants, from `CreateNetworkConnector`'s own
/// documentation of `OperatorRole` ("this role must have permissions for
/// ec2:CreateNetworkInterface, ec2:DeleteNetworkInterface, and related describe
/// operations").
const ENI_ACTIONS: [&str; 2] = ["ec2:CreateNetworkInterface", "ec2:DeleteNetworkInterface"];

/// This service mounted beside the OWNERS of the resources it resolves, on one shared
/// engine, plus an SDK client for each over the router's own in-process callback — the
/// very seam this service's resolution reaches them through.
struct Wired {
    router: MultiServiceRouter,
    ec2: ::aws_sdk_ec2::Client,
    iam: ::aws_sdk_iam::Client,
}

impl Wired {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
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
        let iam = ::aws_sdk_iam::Client::from_conf(
            ::aws_sdk_iam::Config::builder()
                .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
                .region(::aws_sdk_iam::config::Region::new(REGION.to_string()))
                .endpoint_url(format!("https://iam.{REGION}.amazonaws.com"))
                .credentials_provider(callback.aws_credentials_provider())
                .http_client(callback.http_client())
                .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
                .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled(),
                )
                .build(),
        );
        Self { router, ec2, iam }
    }

    /// The subnets EC2 answers for in this Region, as EC2 reported them.
    async fn subnets(&self) -> Vec<String> {
        self.ec2
            .describe_subnets()
            .send()
            .await
            .expect("setup: EC2 must report the account's subnets")
            .subnets()
            .iter()
            .filter_map(|subnet| subnet.subnet_id().map(str::to_string))
            .collect()
    }

    /// The security groups EC2 answers for in this Region, as EC2 reported them.
    async fn security_groups(&self) -> Vec<String> {
        self.ec2
            .describe_security_groups()
            .send()
            .await
            .expect("setup: EC2 must report the account's security groups")
            .security_groups()
            .iter()
            .filter_map(|group| group.group_id().map(str::to_string))
            .collect()
    }

    /// Create a role through IAM itself, with `trust_document` and the inline policy
    /// `grant` (when one is given), and answer the ARN IAM minted for it.
    async fn role(
        &self,
        name: &str,
        path: &str,
        trust_document: &str,
        grant: Option<Value>,
    ) -> String {
        let arn = self
            .iam
            .create_role()
            .role_name(name)
            .path(path)
            .assume_role_policy_document(trust_document)
            .send()
            .await
            .unwrap_or_else(|error| panic!("setup: IAM must create the role {name}: {error:?}"))
            .role()
            .map(|role| role.arn().to_string())
            .unwrap_or_else(|| panic!("setup: IAM must report an ARN for {name}"));
        if let Some(grant) = grant {
            self.iam
                .put_role_policy()
                .role_name(name)
                .policy_name("operator")
                .policy_document(grant.to_string())
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("setup: IAM must attach the inline policy to {name}: {error:?}")
                });
        }
        arn
    }

    /// A role IAM proves usable for a connector: it trusts Lambda and grants the
    /// interface actions, so a create or update naming it is gated on its OTHER
    /// references alone.
    async fn usable_role(&self, name: &str) -> String {
        self.role(name, "/", &trust(LAMBDA_PRINCIPAL), Some(allowing(&ENI_ACTIONS)))
            .await
    }

    /// Create a customer-managed policy carrying `document` and ATTACH it to the role
    /// `role_name`, through IAM itself.
    async fn attach_managed(&self, policy_name: &str, role_name: &str, document: Value) {
        let arn = self
            .iam
            .create_policy()
            .policy_name(policy_name)
            .policy_document(document.to_string())
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setup: IAM must create the policy {policy_name}: {error:?}")
            })
            .policy()
            .and_then(|policy| policy.arn().map(str::to_string))
            .unwrap_or_else(|| panic!("setup: IAM must report an ARN for {policy_name}"));
        self.iam
            .attach_role_policy()
            .role_name(role_name)
            .policy_arn(&arn)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("setup: IAM must attach {arn} to {role_name}: {error:?}")
            });
    }
}

/// A trust policy admitting `principal` to assume the role.
fn trust(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity policy allowing `actions` on every resource.
fn allowing(actions: &[&str]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": "*" }],
    })
}

/// The sentence this service reports an EC2 read's failure with.
fn ec2_failure(call: &str, code: &str, message: &str) -> String {
    format!("Error occurred while {call}. EC2 Error Code: {code}. EC2 Error Message: {message}")
}

/// Assert that `answer` is the modelled 400 carrying exactly `message`.
fn assert_refused(case: &str, answer: &crate::network_connector_lifecycle::Answer, message: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    let code = text(&answer.body, "/__type");
    assert!(
        code.ends_with(INVALID_PARAMETER),
        "case {case:?}: expected {INVALID_PARAMETER}, got {code:?} with {:?}",
        answer.body
    );
    assert_eq!(
        // `InvalidParameterValueException`'s message member is modelled `message`.
        text(&answer.body, "/message"),
        message,
        "case {case:?}: expected the refusal to state {message:?}, got {:?}",
        at(&answer.body, "/message")
    );
}

/// RULE, on the create: every subnet and security group a `Configuration` names is
/// resolved against EC2 before the connector is written, and one the account does not hold
/// refuses the call with the EC2 failure that named it.
///
/// The rows differ along what the rule generalises over: which member carries the bad
/// value, how many values the member carries, WHERE in the list the bad one sits, and what
/// shape the bad value has (a well-formed id of the right form, and a value that is not an
/// id at all).
#[tokio::test]
async fn a_create_naming_an_ec2_resource_the_account_does_not_hold_is_refused() {
    let wired = Wired::new();
    let operator_role = wired.usable_role("ec2-rows-operator").await;
    let subnets = wired.subnets().await;
    let groups = wired.security_groups().await;
    assert!(
        subnets.len() >= 2 && !groups.is_empty(),
        "setup: EC2 must answer for at least two subnets and one security group, got \
         {subnets:?} and {groups:?}"
    );
    // Well-formed identifiers of EC2's own id form that name nothing: the account holds no
    // subnet or group carrying them, which is what makes a request naming one refusable.
    let absent_subnet = "subnet-00000000000000fff";
    let absent_group = "sg-00000000000000fff";

    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "one subnet EC2 answers for",
            vpc_egress(&[&subnets[0]], &[], None, None),
            None,
        ),
        (
            "two subnets and a group EC2 answers for",
            vpc_egress(
                &[&subnets[0], &subnets[1]],
                &[&groups[0]],
                Some("DualStack"),
                Some("MicroVm"),
            ),
            None,
        ),
        (
            "a single absent subnet",
            vpc_egress(&[absent_subnet], &[], None, None),
            Some(ec2_failure(
                "DescribeSubnets",
                "InvalidSubnetID.NotFound",
                &format!("The subnet ID '{absent_subnet}' does not exist"),
            )),
        ),
        (
            "an absent subnet LAST in a list that starts with a real one",
            vpc_egress(&[&subnets[0], absent_subnet], &[], None, None),
            Some(ec2_failure(
                "DescribeSubnets",
                "InvalidSubnetID.NotFound",
                &format!("The subnet ID '{absent_subnet}' does not exist"),
            )),
        ),
        (
            // `NetworkConnectorSubnetIds`' element pattern is `subnet-[0-9a-z]*`, so a
            // shorter id than EC2 mints is still a value the handler has to resolve.
            "a pattern-valid id shorter than any EC2 mints",
            vpc_egress(&["subnet-0"], &[], None, None),
            Some(ec2_failure(
                "DescribeSubnets",
                "InvalidSubnetID.NotFound",
                "The subnet ID 'subnet-0' does not exist",
            )),
        ),
        (
            "a real subnet and an absent security group",
            vpc_egress(&[&subnets[0]], &[absent_group], None, None),
            Some(ec2_failure(
                "DescribeSecurityGroups",
                "InvalidGroup.NotFound",
                &format!("The security group '{absent_group}' does not exist"),
            )),
        ),
        (
            "a real group and an absent one beside it",
            vpc_egress(&[&subnets[0]], &[&groups[0], absent_group], None, None),
            Some(ec2_failure(
                "DescribeSecurityGroups",
                "InvalidGroup.NotFound",
                &format!("The security group '{absent_group}' does not exist"),
            )),
        ),
        (
            // Neither set resolves: the security groups are the ones the service resolves
            // first, so theirs is the failure it reports.
            "an absent subnet AND an absent security group",
            vpc_egress(&[absent_subnet], &[absent_group], None, None),
            Some(ec2_failure(
                "DescribeSecurityGroups",
                "InvalidGroup.NotFound",
                &format!("The security group '{absent_group}' does not exist"),
            )),
        ),
    ];

    for (index, (case, configuration, refusal)) in rows.iter().enumerate() {
        let name = format!("ec2-row-{index}");
        let answer = create(
            &wired.router,
            json!({
                "Name": name,
                "Configuration": configuration,
                "OperatorRole": operator_role,
            }),
        )
        .await;
        match refusal {
            None => assert_eq!(
                answer.status, 202,
                "case {case:?}: every reference resolves, so expected HTTP 202, got {} with {:?}",
                answer.status, answer.body
            ),
            Some(message) => {
                assert_refused(case, &answer, message);
                // Nothing was written: the refusal came before any state did.
                let fetched = get(&wired.router, &name).await;
                assert_eq!(
                    fetched.status, 404,
                    "case {case:?}: a refused create must persist no connector, got {} with {:?}",
                    fetched.status, fetched.body
                );
            }
        }
    }

    // And the store holds exactly the connectors the accepted rows created.
    let accepted = rows.iter().filter(|(_, _, refusal)| refusal.is_none()).count();
    let listed = list(&wired.router, "").await;
    assert_eq!(
        listed.body["NetworkConnectors"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        accepted,
        "only the rows whose references resolved may be enumerated, got {:?}",
        listed.body
    );
}

/// RULE, on the update: the same resolution, ahead of the version append — and a refused
/// update leaves the connector exactly as it was.
///
/// The rows differ along which member carries the unusable value and whether the request
/// also changes something else, and each one is followed by a read of the connector.
#[tokio::test]
async fn an_update_naming_an_ec2_resource_the_account_does_not_hold_changes_nothing() {
    let wired = Wired::new();
    let operator_role = wired.usable_role("update-rows-operator").await;
    let subnets = wired.subnets().await;
    let groups = wired.security_groups().await;
    let absent_subnet = "subnet-00000000000000eee";
    let absent_group = "sg-00000000000000eee";

    let original = vpc_egress(&[&subnets[0]], &[&groups[0]], Some("IPv4"), None);
    let created = create(
        &wired.router,
        json!({
            "Name": "updated-connector",
            "Configuration": original,
            "OperatorRole": operator_role,
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the create must succeed, got {} with {:?}",
        created.status, created.body
    );
    // An update needs an ACTIVE connector, which the clock owes the new one.
    wait_for_the_clock();

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "an absent subnet",
            vpc_egress(&[absent_subnet], &[&groups[0]], Some("IPv4"), None),
            ec2_failure(
                "DescribeSubnets",
                "InvalidSubnetID.NotFound",
                &format!("The subnet ID '{absent_subnet}' does not exist"),
            ),
        ),
        (
            "an absent subnet beside a real one",
            vpc_egress(
                &[&subnets[0], absent_subnet],
                &[&groups[0]],
                Some("DualStack"),
                None,
            ),
            ec2_failure(
                "DescribeSubnets",
                "InvalidSubnetID.NotFound",
                &format!("The subnet ID '{absent_subnet}' does not exist"),
            ),
        ),
        (
            "an absent security group",
            vpc_egress(&[&subnets[0]], &[absent_group], Some("IPv4"), None),
            ec2_failure(
                "DescribeSecurityGroups",
                "InvalidGroup.NotFound",
                &format!("The security group '{absent_group}' does not exist"),
            ),
        ),
    ];

    for (case, configuration, message) in &rows {
        let refused = update(
            &wired.router,
            "updated-connector",
            json!({ "Configuration": configuration }),
        )
        .await;
        assert_refused(case, &refused, message);

        let fetched = get(&wired.router, "updated-connector").await;
        assert_eq!(
            at(&fetched.body, "/Configuration"),
            original,
            "case {case:?}: a refused update must leave the previous Configuration standing, \
             got {:?}",
            at(&fetched.body, "/Configuration")
        );
        assert_eq!(
            text(&fetched.body, "/State"),
            "ACTIVE",
            "case {case:?}: a refused update must leave State ACTIVE, got {:?}",
            at(&fetched.body, "/State")
        );
        assert_eq!(
            at(&fetched.body, "/LastUpdateStatus"),
            Value::Null,
            "case {case:?}: a request that never reached the asynchronous update path reports \
             no LastUpdateStatus, got {:?}",
            at(&fetched.body, "/LastUpdateStatus")
        );
        assert_eq!(
            at(&fetched.body, "/LatestVersion"),
            json!(1),
            "case {case:?}: a refused update appends no version, got {:?}",
            at(&fetched.body, "/LatestVersion")
        );
    }

    // The positive control: the same update naming references EC2 answers for is served,
    // and it is the one that appends a version.
    let moved = vpc_egress(&[&subnets[1]], &[&groups[0]], Some("IPv4"), None);
    let accepted = update(
        &wired.router,
        "updated-connector",
        json!({ "Configuration": moved }),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "an update whose references resolve must be served, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        at(&accepted.body, "/Version"),
        json!(2),
        "the served update appends version 2, got {:?}",
        at(&accepted.body, "/Version")
    );
}

/// RULE, for the IAM half: the `OperatorRole` is proved usable against the account's IAM
/// records at call time, in the two stages the service proves it in — Lambda can ASSUME
/// it, and it GRANTS the interface operations Lambda performs under it — each with its own
/// refusal.
///
/// The rows differ along what the value names: a role that is usable (through an inline
/// policy, through an attached managed policy, and under a path), a role that exists but
/// trusts another principal, a role that trusts Lambda and grants too little (nothing, a
/// different EC2 action, one of the two actions, and a grant an explicit `Deny` overrides),
/// a well-formed ARN naming no role of this account, and a role of ANOTHER account, which
/// this emulator's IAM cannot answer for and so grounds no refusal.
#[tokio::test]
async fn a_create_is_refused_unless_iam_proves_the_operator_role_usable() {
    let wired = Wired::new();
    let subnets = wired.subnets().await;
    let configuration = vpc_egress(&[&subnets[0]], &[], None, None);

    let unassumable = "The service is unable to assume the provided \
                       NetworkConnectorOperatorRole. Please verify the trust policy on the \
                       role.";
    let unauthorized = "Encountered unauthorized operation while calling EC2 due to invalid \
                        ConnectorOperatorRole permissions";

    let inline_grant = wired
        .role(
            "operator-inline",
            "/",
            &trust(LAMBDA_PRINCIPAL),
            Some(allowing(&ENI_ACTIONS)),
        )
        .await;
    let under_path = wired
        .role(
            "operator-under-path",
            "/service-role/",
            &trust(LAMBDA_PRINCIPAL),
            Some(allowing(&ENI_ACTIONS)),
        )
        .await;
    let managed_grant = wired
        .role("operator-managed", "/", &trust(LAMBDA_PRINCIPAL), None)
        .await;
    wired
        .attach_managed(
            "EniOperations",
            "operator-managed",
            allowing(&ENI_ACTIONS),
        )
        .await;
    let untrusted = wired
        .role(
            "operator-untrusted",
            "/",
            &trust(OTHER_PRINCIPAL),
            Some(allowing(&ENI_ACTIONS)),
        )
        .await;
    let ungranted = wired
        .role("operator-ungranted", "/", &trust(LAMBDA_PRINCIPAL), None)
        .await;
    let wrong_action = wired
        .role(
            "operator-wrong-action",
            "/",
            &trust(LAMBDA_PRINCIPAL),
            Some(allowing(&["ec2:DescribeSubnets"])),
        )
        .await;
    let half_granted = wired
        .role(
            "operator-half",
            "/",
            &trust(LAMBDA_PRINCIPAL),
            Some(allowing(&[ENI_ACTIONS[0]])),
        )
        .await;
    let denied = wired
        .role(
            "operator-denied",
            "/",
            &trust(LAMBDA_PRINCIPAL),
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": ENI_ACTIONS, "Resource": "*" },
                    { "Effect": "Deny", "Action": ENI_ACTIONS[1], "Resource": "*" },
                ],
            })),
        )
        .await;

    let rows: Vec<(&str, String, Option<&str>)> = vec![
        ("a role trusting Lambda with an inline grant", inline_grant, None),
        ("the same grant through an attached managed policy", managed_grant, None),
        ("a usable role under a path", under_path, None),
        (
            "a role of this account that trusts another service",
            untrusted,
            Some(unassumable),
        ),
        (
            "a well-formed ARN naming no role of this account",
            "arn:aws:iam::000000000000:role/never-created".to_string(),
            Some(unassumable),
        ),
        (
            "a role that trusts Lambda and grants nothing",
            ungranted,
            Some(unauthorized),
        ),
        (
            "a role granting a different EC2 action",
            wrong_action,
            Some(unauthorized),
        ),
        (
            "a role granting one of the two interface actions",
            half_granted,
            Some(unauthorized),
        ),
        (
            "a role whose grant an explicit Deny overrides",
            denied,
            Some(unauthorized),
        ),
        (
            // Another account's IAM is not this emulator's to answer for, so nothing was
            // learned about the role and nothing is refused.
            "a role of another account",
            "arn:aws:iam::123456789012:role/foreign".to_string(),
            None,
        ),
    ];

    for (index, (case, operator_role, refusal)) in rows.iter().enumerate() {
        let name = format!("role-row-{index}");
        let answer = create(
            &wired.router,
            json!({
                "Name": name,
                "Configuration": configuration,
                "OperatorRole": operator_role,
            }),
        )
        .await;
        match refusal {
            None => assert_eq!(
                answer.status, 202,
                "case {case:?}: expected HTTP 202, got {} with {:?}",
                answer.status, answer.body
            ),
            Some(message) => {
                assert_refused(case, &answer, message);
                let fetched = get(&wired.router, &name).await;
                assert_eq!(
                    fetched.status, 404,
                    "case {case:?}: a refused create must persist no connector, got {} with {:?}",
                    fetched.status, fetched.body
                );
            }
        }
    }
}

/// RULE, on the update: the same two stages gate the `OperatorRole` an update hands the
/// connector, and a refused update changes nothing.
#[tokio::test]
async fn an_update_is_refused_unless_iam_proves_the_new_operator_role_usable() {
    let wired = Wired::new();
    let subnets = wired.subnets().await;
    let configuration = vpc_egress(&[&subnets[0]], &[], None, None);
    let usable = wired.usable_role("update-operator").await;
    let ungranted = wired
        .role("update-ungranted", "/", &trust(LAMBDA_PRINCIPAL), None)
        .await;

    let created = create(
        &wired.router,
        json!({
            "Name": "role-updated",
            "Configuration": configuration,
            "OperatorRole": usable,
        }),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the create must succeed, got {} with {:?}",
        created.status, created.body
    );
    wait_for_the_clock();

    let refused = update(
        &wired.router,
        "role-updated",
        json!({ "OperatorRole": ungranted }),
    )
    .await;
    assert_refused(
        "an update handing the connector a role that grants nothing",
        &refused,
        "Encountered unauthorized operation while calling EC2 due to invalid \
         ConnectorOperatorRole permissions",
    );
    let fetched = get(&wired.router, "role-updated").await;
    assert_eq!(
        text(&fetched.body, "/OperatorRole"),
        usable,
        "a refused update must leave the previous OperatorRole standing, got {:?}",
        at(&fetched.body, "/OperatorRole")
    );
    assert_eq!(
        at(&fetched.body, "/LatestVersion"),
        json!(1),
        "a refused update appends no version, got {:?}",
        at(&fetched.body, "/LatestVersion")
    );
}

/// RULE, the other half: a refusal is grounded in what the owning service ANSWERED, so an
/// emulator with no EC2 and no IAM mounted refuses neither a subnet nor a role it could
/// not ask about.
#[tokio::test]
async fn nothing_is_refused_on_a_reference_no_service_was_asked_about() {
    let router = crate::network_connector_lifecycle::emulator();
    let rows = [
        (
            "a subnet nothing can answer for",
            vpc_egress(&["subnet-00000000000000fff"], &[], None, None),
            OPERATOR_ROLE.to_string(),
        ),
        (
            "a security group nothing can answer for",
            vpc_egress(
                &["subnet-00000000000000fff"],
                &["sg-00000000000000fff"],
                None,
                None,
            ),
            OPERATOR_ROLE.to_string(),
        ),
        (
            "a role nothing can answer for",
            vpc_egress(&["subnet-00000000000000fff"], &[], None, None),
            "arn:aws:iam::000000000000:role/never-created".to_string(),
        ),
    ];

    for (index, (case, configuration, operator_role)) in rows.iter().enumerate() {
        let answer = create(
            &router,
            json!({
                "Name": format!("ungrounded-{index}"),
                "Configuration": configuration,
                "OperatorRole": operator_role,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 202,
            "case {case:?}: with no service to ask, nothing is refused, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
