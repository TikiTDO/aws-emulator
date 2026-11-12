// SPDX-License-Identifier: CC0-1.0

//! What `RunInstances` does with an `IamInstanceProfile`, and how
//! `DescribeIamInstanceProfileAssociations` refuses a filter name it does not document —
//! end to end over the wire the service speaks (`ec2Query`, through the shared
//! [`super::ec2_query`] harness).
//!
//! The rules under test:
//!
//! 1. a launch that names "the name or Amazon Resource Name (ARN) of an IAM instance
//!    profile" (`RunInstancesRequest$IamInstanceProfile`) GIVES EVERY INSTANCE IT MINTS
//!    that profile: one association per instance, each with a minted association id, the
//!    instance it is over, the profile as its ARN whichever way the request spelled it, a
//!    timestamp, and the non-terminal `associating` state the clock then moves to
//!    `associated`. The launch's own answer, `DescribeInstances` and
//!    `DescribeIamInstanceProfileAssociations` all report that one record, so none of them
//!    can disagree about one instance's profile;
//! 2. a launch that names NO profile mints no association, and its instances run as none;
//! 3. a profile reference the service cannot act on REFUSES the launch, which then stores
//!    neither an instance nor an association;
//! 4. a filter name `DescribeIamInstanceProfileAssociations` does not document is refused
//!    with EC2's `InvalidParameterValue` and the one sentence it answers for an unknown
//!    filter, carrying the name the REQUEST sent; a name it does document is served.
//!
//! Each row is a distinct input along what the rule generalises over: how the request spells
//! the profile, which characters and how many the name carries, which partition, account and
//! path the ARN names, how many instances the launch mints, which shape an unusable profile
//! reference takes, and which undocumented filter name is sent. No row is the specification.

use std::time::Duration;

use ec2::state::{
    IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX, IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED,
    IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING,
    MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for: the account's default VPC and its default
/// subnets are seeded for the emulator's default Region, and every instance below is
/// launched into one of them.
const REGION: &str = "us-east-1";

/// The AMI every launch here names. Which image an instance was launched from has nothing to
/// do with the profile it runs as, so one serves every row.
const AMI: &str = "ami-0123456789abcdef0";

/// The EC2 code for a request member whose value the service will not accept.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The EC2 code for members that are each valid alone but cannot be given together.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// The EC2 code for a member the operation needs and the request left out.
const MISSING_PARAMETER: &str = "MissingParameter";

/// One EC2 emulator over its own engine, as [`super::ec2_query::emulator`] builds it.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// `IamInstanceProfile.Name=<name>`, the request's name spelling of a profile.
fn by_name(name: &str) -> Vec<Param> {
    vec![param("IamInstanceProfile.Name", name)]
}

/// `IamInstanceProfile.Arn=<arn>`, the request's ARN spelling of a profile.
fn by_arn(arn: &str) -> Vec<Param> {
    vec![param("IamInstanceProfile.Arn", arn)]
}

/// The ARN of the instance profile this account holds under `name`, as the emulator's one
/// ARN builder renders it.
fn profile_arn(name: &str) -> String {
    engine::emulator_arn("iam", "", &format!("instance-profile/{name}"))
}

/// Launch `count` instances carrying `profile`, as the HTTP status and body.
async fn launch(
    router: &MultiServiceRouter,
    count: i32,
    profile: &[Param],
) -> (u16, String) {
    let mut params = vec![
        param("ImageId", AMI),
        param("MinCount", &count.to_string()),
        param("MaxCount", &count.to_string()),
    ];
    params.extend_from_slice(profile);
    call(router, "RunInstances", &params).await
}

/// One `DescribeIamInstanceProfileAssociations` call carrying `params`.
async fn describe_associations(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, "DescribeIamInstanceProfileAssociations", params).await
}

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One association as a response reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Association {
    id: String,
    instance_id: Option<String>,
    state: Option<String>,
    profile_arn: Option<String>,
    timestamp: Option<String>,
}

/// Every association a response carries: the elements that have an `associationId` child,
/// which is one per `item` of the describe's set.
fn associations(case: &str, body: &str) -> Vec<Association> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "associationId").is_some())
        .map(|node| Association {
            id: child_text(node, "associationId").unwrap_or_default(),
            instance_id: child_text(node, "instanceId"),
            state: child_text(node, "state"),
            profile_arn: child(node, "iamInstanceProfile")
                .and_then(|profile| child_text(profile, "arn")),
            timestamp: child_text(node, "timestamp"),
        })
        .collect()
}

/// The one association a response carries, or a panic naming the case.
fn one_association(case: &str, body: &str) -> Association {
    let reported = associations(case, body);
    let [association] = reported.as_slice() else {
        panic!("case {case:?}: expected one association, got {reported:?} in {body}");
    };
    association.clone()
}

/// Every instance a reservation response carries, as `(instance id, the ARN of the profile
/// it reports running as)` — the instance elements being the ones that carry an
/// `instanceState`.
fn launched_profiles(case: &str, body: &str) -> Vec<(String, Option<String>)> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "instanceState").is_some())
        .map(|node| {
            (
                child_text(node, "instanceId").unwrap_or_default(),
                child(node, "iamInstanceProfile").and_then(|profile| child_text(profile, "arn")),
            )
        })
        .collect()
}

/// The ARN of the instance profile `DescribeInstances` reports `instance_id` running as, or
/// `None` when it reports the instance running as none.
async fn described_profile_arn(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeInstances",
        &[param("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the describe must succeed, got {status}: {body}"
    );
    launched_profiles(case, &body)
        .into_iter()
        .find(|(id, _)| id == instance_id)
        .unwrap_or_else(|| panic!("case {case:?}: no instance {instance_id} in {body}"))
        .1
}

/// One row of the launch table: how the request spells the profile, the ARN every report of
/// it must carry, and how many instances the launch mints.
struct LaunchRow {
    case: &'static str,
    profile: Vec<Param>,
    expected_arn: String,
    count: i32,
}

/// A launch that names a profile gives EVERY instance it mints an association of its own,
/// reported identically by the launch, by the instance and by the association describe.
#[tokio::test]
async fn a_launch_naming_a_profile_associates_every_instance_it_mints() {
    // Rows: the two spellings of a profile, over the characters and the length IAM's
    // `instanceProfileNameType` admits (len 1..128, pattern `^[\w+=,.@-]+$`), over the
    // partition, the account and the path an instance-profile ARN may carry, and over
    // batches of one, two and three instances.
    let longest_name = "p".repeat(MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS);
    let rows = vec![
        LaunchRow {
            case: "a plain profile name",
            profile: by_name("app-profile"),
            expected_arn: profile_arn("app-profile"),
            count: 1,
        },
        LaunchRow {
            case: "a name carrying the punctuation the name type admits",
            profile: by_name("app+role=1,.@-_x"),
            expected_arn: profile_arn("app+role=1,.@-_x"),
            count: 1,
        },
        LaunchRow {
            case: "a name at the published maximum length",
            profile: by_name(&longest_name),
            expected_arn: profile_arn(&longest_name),
            count: 2,
        },
        LaunchRow {
            case: "an ARN of this account",
            profile: by_arn(&profile_arn("arn-named")),
            expected_arn: profile_arn("arn-named"),
            count: 1,
        },
        LaunchRow {
            case: "an ARN of a profile under a path",
            profile: by_arn(&profile_arn("team/nested")),
            expected_arn: profile_arn("team/nested"),
            count: 3,
        },
        LaunchRow {
            case: "an ARN of another account",
            profile: by_arn("arn:aws:iam::111122223333:instance-profile/shared"),
            expected_arn: "arn:aws:iam::111122223333:instance-profile/shared".to_string(),
            count: 1,
        },
        LaunchRow {
            case: "an ARN in another partition",
            profile: by_arn("arn:aws-us-gov:iam::000000000000:instance-profile/gov"),
            expected_arn: "arn:aws-us-gov:iam::000000000000:instance-profile/gov".to_string(),
            count: 1,
        },
    ];

    for LaunchRow {
        case,
        profile,
        expected_arn,
        count,
    } in rows
    {
        let router = emulator();
        let (status, body) = launch(&router, count, &profile).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the launch must succeed, got {status}: {body}"
        );

        // The launch's own answer reports the profile on every instance it minted
        // ("the IAM instance profile associated with the instance, if applicable" —
        // `Instance$IamInstanceProfile`).
        let launched = launched_profiles(case, &body);
        assert_eq!(
            launched.len(),
            usize::try_from(count).unwrap_or_default(),
            "case {case:?}: expected {count} launched instances, got {launched:?}"
        );
        for (instance_id, reported) in &launched {
            assert_eq!(
                reported.as_deref(),
                Some(expected_arn.as_str()),
                "case {case:?}: expected the launch to report {instance_id} running as \
                 {expected_arn}, got {reported:?}"
            );
        }

        // One association per instance, each addressable by the documented `instance-id`
        // filter, each in the non-terminal state a fresh association is recorded in.
        let mut association_ids = Vec::new();
        for (instance_id, _) in &launched {
            let (status, body) =
                describe_associations(&router, &filters(&[("instance-id", &[instance_id])])).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the association describe must succeed, got {status}: {body}"
            );
            let recorded = one_association(case, &body);
            assert!(
                is_long_id(IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX, &recorded.id),
                "case {case:?}: expected a minted \
                 {IAM_INSTANCE_PROFILE_ASSOCIATION_ID_PREFIX}- id, got {:?}",
                recorded.id
            );
            assert_eq!(
                recorded.instance_id.as_deref(),
                Some(instance_id.as_str()),
                "case {case:?}: expected the association over {instance_id}, got {recorded:?}"
            );
            assert_eq!(
                recorded.profile_arn.as_deref(),
                Some(expected_arn.as_str()),
                "case {case:?}: expected the profile {expected_arn}, got {recorded:?}"
            );
            assert_eq!(
                recorded.state.as_deref(),
                Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING),
                "case {case:?}: expected a fresh association to be \
                 {IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATING}, got {recorded:?}"
            );
            assert!(
                recorded
                    .timestamp
                    .as_deref()
                    .and_then(as_timestamp)
                    .is_some(),
                "case {case:?}: expected a timestamp, got {recorded:?}"
            );
            // And the instance itself reports the same profile, off the same record.
            assert_eq!(
                described_profile_arn(case, &router, instance_id).await,
                Some(expected_arn.clone()),
                "case {case:?}: expected {instance_id} to run as {expected_arn}"
            );
            association_ids.push(recorded.id);
        }
        let mut distinct = association_ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            association_ids.len(),
            "case {case:?}: expected one association of its own per instance, got \
             {association_ids:?}"
        );

        // The clock owes every one of them the move to `associated`, and each is then
        // addressable by the id the describe reported for it.
        past_the_transition().await;
        for (position, association_id) in association_ids.iter().enumerate() {
            let (status, body) =
                describe_associations(&router, &indexed("AssociationId", &[association_id])).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the association describe must succeed, got {status}: {body}"
            );
            let settled = one_association(case, &body);
            assert_eq!(
                settled.id, *association_id,
                "case {case:?}: expected the association {association_id}, got {settled:?}"
            );
            assert_eq!(
                settled.state.as_deref(),
                Some(IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED),
                "case {case:?}: expected the association to converge to \
                 {IAM_INSTANCE_PROFILE_ASSOCIATION_STATE_ASSOCIATED}, got {settled:?}"
            );
            assert_eq!(
                settled.instance_id.as_deref(),
                Some(launched[position].0.as_str()),
                "case {case:?}: expected {association_id} over {}, got {settled:?}",
                launched[position].0
            );
        }
    }
}

/// A launch that names no profile associates nothing: its instances run as none, whatever
/// else the launch carries.
#[tokio::test]
async fn a_launch_naming_no_profile_associates_nothing() {
    // Rows: a bare launch, a batch, and a launch carrying another optional member entirely —
    // nothing but the profile member mints an association.
    let rows: Vec<(&str, i32, Vec<Param>)> = vec![
        ("a bare launch", 1, Vec::new()),
        ("a batch of two", 2, Vec::new()),
        (
            "a launch carrying a key pair and a type but no profile",
            1,
            vec![param("KeyName", "developer"), param("InstanceType", "t3.micro")],
        ),
    ];

    for (case, count, extra) in rows {
        let router = emulator();
        let (status, body) = launch(&router, count, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the launch must succeed, got {status}: {body}"
        );
        for (instance_id, reported) in launched_profiles(case, &body) {
            assert_eq!(
                reported, None,
                "case {case:?}: expected the launch to report {instance_id} running as no \
                 profile, got {reported:?}"
            );
            assert_eq!(
                described_profile_arn(case, &router, &instance_id).await,
                None,
                "case {case:?}: expected {instance_id} to run as no profile"
            );
        }
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the association describe must succeed, got {status}: {body}"
        );
        assert_eq!(
            associations(case, &body),
            Vec::new(),
            "case {case:?}: a launch naming no profile must record no association, got {body}"
        );
    }
}

/// A profile reference the service cannot act on refuses the LAUNCH, which then stores
/// neither an instance nor an association.
#[tokio::test]
async fn a_profile_reference_the_service_cannot_act_on_refuses_the_launch() {
    // Rows: the two spellings together and neither of them, then every shape of value that
    // names no instance profile — a name outside the character set `instanceProfileNameType`
    // admits, a name past its published maximum, an ARN of another IAM resource kind, an ARN
    // carrying a Region (which IAM's global ARNs do not), an ARN with no name after the
    // resource type, and a value that is not an ARN at all.
    let too_long_name = "p".repeat(MAX_IAM_INSTANCE_PROFILE_NAME_CHARACTERS + 1);
    let rows: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "both a name and an ARN",
            vec![
                param("IamInstanceProfile.Name", "app-profile"),
                param("IamInstanceProfile.Arn", &profile_arn("app-profile")),
            ],
            INVALID_PARAMETER_COMBINATION,
        ),
        (
            "an empty name, which names no profile",
            by_name(""),
            MISSING_PARAMETER,
        ),
        (
            "a name carrying a character the name type does not admit",
            by_name("team/app"),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a name past the published maximum length",
            by_name(&too_long_name),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an ARN naming an IAM role rather than an instance profile",
            by_arn(&engine::emulator_arn("iam", "", "role/app")),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an ARN carrying a Region, which an IAM ARN does not",
            by_arn("arn:aws:iam:us-east-1:000000000000:instance-profile/app"),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an ARN with no profile name after the resource type",
            by_arn(&profile_arn("")),
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a value that is not an ARN at all",
            by_arn("instance-profile/app"),
            INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, profile, expected_code) in rows {
        let router = emulator();
        let (status, body) = launch(&router, 1, &profile).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the launch to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // A refused launch stores nothing — neither the instance nor an association.
        let (status, body) = call(&router, "DescribeInstances", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the instances back must succeed, got {status}: {body}"
        );
        assert_eq!(
            elements(&body, "instanceId"),
            Vec::<String>::new(),
            "case {case:?}: a refused launch must store no instance, got {body}"
        );
        let (status, body) = describe_associations(&router, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the association describe must succeed, got {status}: {body}"
        );
        assert_eq!(
            associations(case, &body),
            Vec::new(),
            "case {case:?}: a refused launch must record no association, got {body}"
        );
    }
}

/// The association describe REFUSES a filter name it does not document, naming that name in
/// the one sentence EC2 answers for an unknown filter — and serves every name it does
/// document.
#[tokio::test]
async fn the_association_describe_refuses_a_filter_name_it_does_not_document() {
    let router = emulator();
    let (status, body) = launch(&router, 1, &by_name("app-profile")).await;
    assert_eq!(
        status, 200,
        "the launch must succeed, got {status}: {body}"
    );

    // Rows: a name of no EC2 filter, the member-cased and the title-cased spellings of the
    // two names this operation DOES document ("filter names are case-sensitive" —
    // `Filter$Name`), a name documented for a different operation of this service
    // (`DescribeInstancesRequest$Filters`), a name of the `tag:` family this operation does
    // not carry, and a name whose characters the template must carry through verbatim.
    let undocumented = [
        "profile-name",
        "instance-id-x",
        "InstanceId",
        "Instance-Id",
        "State",
        "iam-instance-profile.arn",
        "tag:Name",
        "a name with spaces",
    ];
    for name in undocumented {
        let (status, body) = describe_associations(&router, &filters(&[(name, &["x"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: an undocumented filter must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_VALUE,
            "case {name:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("An unknown filter '{name}' was given"),
            "case {name:?}: expected the unknown-filter sentence naming the filter the \
             request sent, got {body}"
        );
    }

    // And the names it documents are served rather than refused
    // ("instance-id - The ID of the instance. state - The state of the association" —
    // `DescribeIamInstanceProfileAssociationsRequest$Filters`). The `instance-id` rows name
    // the instance the launch above minted: the values of that filter are read as instance
    // ids, so an id this service never issued is refused for its VALUE (which
    // `iam_instance_profile_associations.rs` covers) and would say nothing about the NAME.
    let (status, body) = describe_associations(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the association describe must succeed, got {status}: {body}"
    );
    let launched = elements(&body, "instanceId");
    let instance_id = launched
        .first()
        .unwrap_or_else(|| panic!("the launch must have recorded an association: {body}"))
        .clone();
    for (case, params) in [
        (
            "instance-id",
            filters(&[("instance-id", &[instance_id.as_str()])]),
        ),
        ("state", filters(&[("state", &["associating"])])),
        (
            "both documented names",
            filters(&[
                ("instance-id", &[instance_id.as_str()]),
                ("state", &["associated"]),
            ]),
        ),
    ] {
        let (status, body) = describe_associations(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a documented filter must be served, got {status}: {body}"
        );
    }
}
