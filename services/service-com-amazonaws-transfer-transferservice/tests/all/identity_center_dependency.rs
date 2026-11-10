// SPDX-License-Identifier: CC0-1.0

//! The IAM Identity Center dependency `CreateWebApp` has.
//!
//! RULE under test: a web app's identity provider is an IAM Identity Center
//! application, which `CreateWebApp` registers on the caller's behalf
//! (`sso:CreateApplication`) in the instance the request names — so an instance the
//! account's Identity Center does not hold has no application namespace to register one
//! in, the dependent call is denied, and the create is refused with the modelled
//! `AccessDeniedException` carrying the standard IAM authorization-failure sentence: the
//! CALLING PRINCIPAL, the action, the Identity Center application resource of the named
//! instance, and that no resource-based policy allows that action. Nothing is created,
//! and a create naming an instance the account DOES hold is served.
//!
//! The rows differ along what the rule generalises over: the form of the instance id the
//! modelled `InstanceArn` pattern admits, whether the instance is one the account holds,
//! which principal signs the call (the account's own credential, and an IAM user), and
//! what else the create asks for (a VPC endpoint, tags, a custom access endpoint) —
//! which the dependency cannot depend on.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

use crate::harness::{member_of, Emulator, ROLE, IDENTITY_CENTER_INSTANCES};

/// The action `CreateWebApp` performs on the caller's behalf, as IAM names it.
const ACTION: &str = "sso:CreateApplication";

/// A policy document that decides nothing, so a seeded principal is the only thing the
/// row varies.
const SILENT_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// `InstanceArn`s satisfying the modelled pattern
/// `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$` that name no instance of
/// this account: both spellings of the instance id the pattern admits, one carrying the
/// punctuation it allows, and one in another partition.
const NOT_HELD: [&str; 4] = [
    "arn:aws:sso:::instance/ssoins-0000000000000000",
    "arn:aws:sso:::instance/ins-ffffffffffffffff",
    "arn:aws:sso:::instance/ssoins-z.y-x.w-v.u-t1s2",
    "arn:aws-us-gov:sso:::instance/ssoins-1111111111111111",
];

/// The identity provider details a create sends: `IdentityCenterConfig` is the union's
/// only member and the union itself is `@required`.
fn identity_center(instance_arn: Option<&str>) -> Value {
    let mut config = json!({"Role": ROLE});
    if let Some(instance_arn) = instance_arn {
        config["InstanceArn"] = json!(instance_arn);
    }
    json!({"IdentityCenterConfig": config})
}

/// The instance id inside an `InstanceArn`.
fn instance_id(instance_arn: &str) -> &str {
    instance_arn
        .rsplit('/')
        .next()
        .expect("an instance ARN names an instance")
}

/// The sentence IAM states for a denied cross-service action, written here from the rule
/// rather than from the implementation: the principal, the action, the application
/// resource of the named instance, and that no resource-based policy allows it.
fn expected_message(principal: &str, instance_arn: &str) -> String {
    let partition = instance_arn
        .split(':')
        .nth(1)
        .expect("an ARN carries a partition");
    format!(
        "User: {principal} is not authorized to perform: {ACTION} on resource: \
         arn:{partition}:sso::*:application/{}/* because no resource-based policy allows \
         the {ACTION} action",
        instance_id(instance_arn)
    )
}

/// How many web apps the account holds, read through the listing.
async fn web_app_count(emulator: &Emulator, case: &str) -> usize {
    let answer = emulator.call("ListWebApps", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListWebApps 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("WebApps")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// RULE: a create naming an Identity Center instance the account does not hold is
/// refused with the relayed `sso:CreateApplication` denial and creates nothing.
#[tokio::test]
async fn a_create_naming_an_instance_the_account_does_not_hold_is_denied() {
    let emulator = Emulator::new();

    // The principal every row below is refused as: the account's own credential signs
    // the harness's requests, and the account contract states which principal that
    // credential speaks for.
    let ambient = ::service_common::caller_principal::ambient_user_arn();

    // The other members a row carries, so the dependency can be seen not to depend on
    // them: nothing more, a VPC endpoint, and tags with a custom access endpoint.
    let shapes: Vec<(&str, Value)> = vec![
        ("the identity provider alone", json!({})),
        (
            "a VPC-hosted create",
            json!({"EndpointDetails": {"Vpc": {"VpcId": "vpc-0123456789abcdef0"}}}),
        ),
        (
            "a tagged create with a custom access endpoint",
            json!({
                "AccessEndpoint": "https://files.example.invalid",
                "Tags": [{"Key": "env", "Value": "dev"}],
                "WebAppEndpointPolicy": "FIPS",
            }),
        ),
    ];

    for instance_arn in NOT_HELD {
        for (shape, extra) in &shapes {
            let case = format!("{instance_arn} with {shape}");
            let before = web_app_count(&emulator, &case).await;

            let mut request = json!({"IdentityProviderDetails": identity_center(Some(instance_arn))});
            let object = request.as_object_mut().expect("the request is an object");
            for (member, value) in extra.as_object().expect("the row is an object") {
                object.insert(member.clone(), value.clone());
            }
            let answer = emulator.call("CreateWebApp", request).await;

            assert_eq!(
                (answer.status, answer.error_code().as_str()),
                (400, "AccessDeniedException"),
                "case {case:?}: expected 400 AccessDeniedException, got {} with body {}",
                answer.status,
                answer.body
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            let expected = expected_message(&ambient, instance_arn);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                member_of(&answer.body, "WebAppId").is_null(),
                "case {case:?}: expected the refusal to mint no WebAppId, got {}",
                answer.body
            );
            let after = web_app_count(&emulator, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: expected the refusal to create nothing, so {before} web \
                 app(s), got {after}"
            );
        }
    }
}

/// RULE (the same rule, the principal half): the sentence names the principal the
/// REQUEST was signed by, read from the caller's identity rather than fixed.
#[tokio::test]
async fn the_denial_names_the_principal_that_signed_the_call() {
    let emulator = Emulator::new();
    // An IAM user holding the access key the harness signs with, so the calling
    // principal is that user rather than the account's own credential.
    emulator.seed_calling_principal("web-app-admin", SILENT_POLICY);

    let instance_arn = NOT_HELD[0];
    let answer = emulator
        .call(
            "CreateWebApp",
            json!({"IdentityProviderDetails": identity_center(Some(instance_arn))}),
        )
        .await;
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (400, "AccessDeniedException"),
        "case \"a seeded IAM caller\": expected 400 AccessDeniedException, got {} with body {}",
        answer.status,
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let expected = expected_message(&iam::user_arn("/", "web-app-admin"), instance_arn);
    assert_eq!(
        message, expected,
        "case \"a seeded IAM caller\": expected {expected:?}, got {message:?}"
    );
}

/// RULE (the other half): a create naming an instance the account DOES hold is served,
/// and the application it registers is named inside that instance; a create naming no
/// instance at all makes no dependent call, so there is nothing to deny.
#[tokio::test]
async fn a_create_naming_an_instance_the_account_holds_is_served() {
    let emulator = Emulator::new();

    for instance_arn in IDENTITY_CENTER_INSTANCES {
        let case = format!("the held instance {instance_arn}");
        let answer = emulator
            .call(
                "CreateWebApp",
                json!({"IdentityProviderDetails": identity_center(Some(instance_arn))}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateWebApp 200, got {} with body {}",
            answer.status, answer.body
        );
        let web_app_id = answer
            .get("WebAppId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a WebAppId, got {}", answer.body))
            .to_string();

        let described = emulator
            .call("DescribeWebApp", json!({"WebAppId": &web_app_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeWebApp 200, got {} with body {}",
            described.status, described.body
        );
        let application = member_of(
            member_of(
                member_of(described.get("WebApp"), "DescribedIdentityProviderDetails"),
                "IdentityCenterConfig",
            ),
            "ApplicationArn",
        )
        .as_str()
        .unwrap_or_default()
        .to_string();
        let within = format!("application/{}/", instance_id(instance_arn));
        assert!(
            application.contains(&within),
            "case {case:?}: expected the application to be named inside the instance \
             ({within:?}), got {application:?}"
        );
    }

    // A create that names no instance has no dependent call to make.
    let answer = emulator
        .call(
            "CreateWebApp",
            json!({"IdentityProviderDetails": identity_center(None)}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case \"no instance named\": expected CreateWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
}
