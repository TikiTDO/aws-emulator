// SPDX-License-Identifier: CC0-1.0

//! WHICH instance profile an EC2 association is over, as every report of one names it.
//!
//! `IamInstanceProfile` carries exactly two members —
//! `Arn` ("The Amazon Resource Name (ARN) of the instance profile") and `Id` ("The ID of the
//! instance profile"), `smithy_query(query="shape",shape="IamInstanceProfile")` — and
//! neither is EC2's to mint: an instance profile is an IAM resource, so both come from the
//! account's IAM records. The rule under test:
//!
//! 1. whichever way a request spells the profile (by name, or by the ARN IAM holds it
//!    under, with or without a path), every report of the association carries the ARN IAM
//!    holds that profile under AND the unique id IAM minted for it — on the write that
//!    records the association, on the write that REPLACES it, on
//!    `DescribeIamInstanceProfileAssociations`, and on the instance's own projection;
//! 2. a reference the account's IAM records hold no profile for is reported as the request
//!    spelled it, with NO id: this service invents no id for a profile IAM has no record
//!    of.
//!
//! Every expected value here is read back from IAM's OWN response to the create, never
//! written into the table: a profile's id is IAM's to generate, so the test asserts the two
//! services agree about one profile rather than asserting a literal.
//!
//! The suite drives both services over the wire they really speak — EC2 over `ec2Query` and
//! IAM over `awsQuery`, through one `MultiServiceRouter` over ONE shared engine, as the
//! emulator mounts them.

use std::collections::BTreeMap;
use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, indexed};
use super::wire::{Param, child, child_text, document, param};

/// The Region every EC2 call here is signed for: the account's default VPC and subnets are
/// seeded for the emulator's default Region, and the instances below launch into one.
const REGION: &str = "us-east-1";

/// The IAM API version a client sends in an awsQuery body.
const IAM_API_VERSION: &str = "2010-05-08";

/// The access key id every call here is presented under: the account's own published
/// credential, the identity that provisions the profiles the rows name.
const ACCOUNT_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// The AMI every launch here names: which image an instance came from has nothing to do
/// with the profile it runs as, so one serves every row.
const AMI: &str = "ami-0123456789abcdef0";

/// The profile every row REPLACES its first profile with, held by IAM in every row — so the
/// replacement path is exercised whether or not the row's first reference resolved.
const REPLACEMENT_PROFILE: &str = "replacement-profile";

/// The IAM path a profile created without one carries (`CreateInstanceProfile$Path`
/// defaults to `/`).
const DEFAULT_PATH: &str = "/";

/// How a row's request spells the profile it names.
#[derive(Debug, Clone)]
enum Spelling {
    /// `IamInstanceProfile.Name=<name>`.
    Name(&'static str),
    /// `IamInstanceProfile.Arn=` the ARN IAM holds the profile of this name under.
    HeldArn(&'static str),
    /// `IamInstanceProfile.Arn=` this value, whatever IAM holds.
    LiteralArn(String),
}

/// What every report of the association must then say the profile is.
#[derive(Debug, Clone)]
enum Expected {
    /// The ARN and the id IAM holds the profile of this name under.
    Held(&'static str),
    /// This ARN and no id: the account's IAM records hold no such profile.
    ArnWithoutId(String),
}

/// One row: the profiles IAM is to hold as `(name, path)`, how the request spells the one it
/// names, and what the reports must carry for it.
type Row = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Spelling,
    Expected,
);

/// The profile as a report of an association names it: `(arn, id)`.
type ReportedProfile = (Option<String>, Option<String>);

/// The emulator the rows drive: EC2 beside IAM on ONE shared engine, so the profiles a row
/// creates in IAM are the records EC2 resolves its references against.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(iam::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// One EC2 `action` call carrying `params`.
async fn ec2_call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One IAM `action` call carrying `params`, on the awsQuery wire IAM speaks, made by the
/// account's own published credential — the identity that provisions the profiles below.
///
/// The request goes through the harness every suite shares, so this suite carries no second
/// copy of the query envelope or of its encoding.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    super::ec2_query::query_call_as(
        router,
        REGION,
        "iam",
        IAM_API_VERSION,
        ACCOUNT_KEY_ID,
        action,
        params,
    )
    .await
}

/// Create the instance profile `name` under `path` in IAM and answer the `(arn, id)` IAM
/// reports for it — the values EVERY assertion below is taken against, read from the owning
/// service rather than written into the table.
async fn create_profile(
    case: &str,
    router: &MultiServiceRouter,
    name: &str,
    path: &str,
) -> (String, String) {
    let (status, body) = iam_call(
        router,
        "CreateInstanceProfile",
        &[("InstanceProfileName", name), ("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: IAM must hold the profile {name:?}, got {status}: {body}"
    );
    let arn = element(&body, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: IAM reported no Arn for {name:?}: {body}"));
    let id = element(&body, "InstanceProfileId").unwrap_or_else(|| {
        panic!("case {case:?}: IAM reported no InstanceProfileId for {name:?}: {body}")
    });
    assert!(
        !arn.is_empty() && !id.is_empty(),
        "case {case:?}: IAM must report both an Arn and an id for {name:?}, got \
         {arn:?}/{id:?}"
    );
    (arn, id)
}

/// Launch one instance and answer its id.
async fn launch(case: &str, router: &MultiServiceRouter) -> String {
    let (status, body) = ec2_call(
        router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the launch must succeed, got {status}: {body}"
    );
    let launched = elements(&body, "instanceId");
    let [instance_id] = launched.as_slice() else {
        panic!("case {case:?}: one launched instance, got {launched:?}");
    };
    instance_id.clone()
}

/// The profile the ONE association a response reports is over, as `(arn, id)`.
fn reported_profile(case: &str, body: &str) -> ReportedProfile {
    let parsed = document(case, body);
    let association = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "associationId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no association in {body}"));
    let profile = child(association, "iamInstanceProfile")
        .unwrap_or_else(|| panic!("case {case:?}: the association reports no profile: {body}"));
    (
        child_text(profile, "arn"),
        child_text(profile, "id"),
    )
}

/// The association id the ONE association a response reports carries.
fn reported_association_id(case: &str, body: &str) -> String {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element())
        .find_map(|node| child_text(node, "associationId"))
        .unwrap_or_else(|| panic!("case {case:?}: no associationId in {body}"))
}

/// The profile `DescribeInstances` reports `instance_id` running as, as `(arn, id)`.
async fn instance_profile_projection(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
) -> ReportedProfile {
    let (status, body) = ec2_call(
        router,
        "DescribeInstances",
        &[param("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the instance describe must succeed, got {status}: {body}"
    );
    let parsed = document(case, &body);
    let instance = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "instanceId").as_deref() == Some(instance_id)
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no instance {instance_id} in {body}"));
    let profile = child(instance, "iamInstanceProfile")
        .unwrap_or_else(|| panic!("case {case:?}: {instance_id} reports no profile: {body}"));
    (
        child_text(profile, "arn"),
        child_text(profile, "id"),
    )
}

#[tokio::test]
async fn every_report_of_an_association_names_the_profile_iam_holds() {
    // Each row differs along what the rule generalises over: how the request spells the
    // profile (a name, the ARN IAM holds it under, a literal ARN), the PATH IAM holds it
    // under, and whether IAM holds it at all — including an ARN of this account whose path
    // names no profile, which resolves to nothing rather than to the profile of that name
    // under another path.
    let rows: Vec<Row> = vec![
        (
            "a profile IAM holds at the default path, named by name",
            vec![("web-tier", DEFAULT_PATH)],
            Spelling::Name("web-tier"),
            Expected::Held("web-tier"),
        ),
        (
            "a profile IAM holds at the default path, named by its ARN",
            vec![("api-tier", DEFAULT_PATH)],
            Spelling::HeldArn("api-tier"),
            Expected::Held("api-tier"),
        ),
        (
            "a profile IAM holds under a path, named by name",
            vec![("nested", "/team/")],
            Spelling::Name("nested"),
            Expected::Held("nested"),
        ),
        (
            "a profile IAM holds under a path, named by that path's ARN",
            vec![("deep", "/team/app/")],
            Spelling::HeldArn("deep"),
            Expected::Held("deep"),
        ),
        (
            "a name IAM holds no profile under",
            Vec::new(),
            Spelling::Name("never-created"),
            Expected::ArnWithoutId(profile_arn("never-created")),
        ),
        (
            "an ARN of another account",
            Vec::new(),
            Spelling::LiteralArn(
                "arn:aws:iam::111122223333:instance-profile/shared".to_string(),
            ),
            Expected::ArnWithoutId(
                "arn:aws:iam::111122223333:instance-profile/shared".to_string(),
            ),
        ),
        (
            "an ARN of this account whose path names no profile IAM holds",
            vec![("pathless", DEFAULT_PATH)],
            Spelling::LiteralArn(profile_arn("other/pathless")),
            Expected::ArnWithoutId(profile_arn("other/pathless")),
        ),
    ];

    for (case, created, spelling, expected) in rows {
        let router = emulator();
        let mut held: BTreeMap<&str, (String, String)> = BTreeMap::new();
        for (name, path) in &created {
            held.insert(name, create_profile(case, &router, name, path).await);
        }
        // The profile the replacement names, held by IAM in every row.
        let replacement = create_profile(case, &router, REPLACEMENT_PROFILE, DEFAULT_PATH).await;

        let profile_params = match &spelling {
            Spelling::Name(name) => vec![param("IamInstanceProfile.Name", name)],
            Spelling::HeldArn(name) => {
                let (arn, _) = held
                    .get(name)
                    .unwrap_or_else(|| panic!("case {case:?}: the row must create {name:?}"));
                vec![param("IamInstanceProfile.Arn", arn)]
            }
            Spelling::LiteralArn(arn) => vec![param("IamInstanceProfile.Arn", arn)],
        };
        let expected_profile: ReportedProfile = match &expected {
            Expected::Held(name) => {
                let (arn, id) = held
                    .get(name)
                    .unwrap_or_else(|| panic!("case {case:?}: the row must create {name:?}"));
                (Some(arn.clone()), Some(id.clone()))
            }
            Expected::ArnWithoutId(arn) => (Some(arn.clone()), None),
        };

        // The WRITE that records the association reports the profile IAM holds.
        let instance_id = launch(case, &router).await;
        let mut params = vec![param("InstanceId", &instance_id)];
        params.extend_from_slice(&profile_params);
        let (status, body) = ec2_call(&router, "AssociateIamInstanceProfile", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the associate must succeed, got {status}: {body}"
        );
        let association_id = reported_association_id(case, &body);
        assert_eq!(
            reported_profile(case, &body),
            expected_profile,
            "case {case:?}: the associate must report the profile IAM holds"
        );

        // The READ reports the same two values, off the same record.
        let (status, body) = ec2_call(
            &router,
            "DescribeIamInstanceProfileAssociations",
            &indexed("AssociationId", &[association_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status}: {body}"
        );
        assert_eq!(
            reported_profile(case, &body),
            expected_profile,
            "case {case:?}: the describe must report the profile IAM holds"
        );

        // And so does the INSTANCE's own projection of the profile it runs as.
        assert_eq!(
            instance_profile_projection(case, &router, &instance_id).await,
            expected_profile,
            "case {case:?}: the instance must be reported running as the profile IAM holds"
        );

        // The REPLACEMENT resolves its own reference the same way: the new association
        // carries the replacement profile's ARN and IAM's id for it.
        let mut params = vec![param("AssociationId", &association_id)];
        params.push(param("IamInstanceProfile.Name", REPLACEMENT_PROFILE));
        let (status, body) =
            ec2_call(&router, "ReplaceIamInstanceProfileAssociation", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replacement must succeed, got {status}: {body}"
        );
        let replaced_id = reported_association_id(case, &body);
        let expected_replacement: ReportedProfile =
            (Some(replacement.0.clone()), Some(replacement.1.clone()));
        assert_eq!(
            reported_profile(case, &body),
            expected_replacement,
            "case {case:?}: the replacement must report the profile IAM holds"
        );

        // Read back through the describe and through the instance projection, so the
        // replacement's profile is what the STORE holds rather than what one response said.
        let (status, body) = ec2_call(
            &router,
            "DescribeIamInstanceProfileAssociations",
            &indexed("AssociationId", &[replaced_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status}: {body}"
        );
        assert_eq!(
            reported_profile(case, &body),
            expected_replacement,
            "case {case:?}: the describe must report the replacement's profile"
        );
        assert_eq!(
            instance_profile_projection(case, &router, &instance_id).await,
            expected_replacement,
            "case {case:?}: the instance must be reported running as the replacement's profile"
        );
    }
}

/// The ARN this account's IAM records would hold the profile `name` under at the default
/// path, as the emulator's one ARN builder renders it.
fn profile_arn(name: &str) -> String {
    engine::emulator_arn("iam", "", &format!("instance-profile/{name}"))
}
