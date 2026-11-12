// SPDX-License-Identifier: CC0-1.0

//! The sentence EVERY ARN-addressed Device Farm operation refuses an ARN that resolves to
//! no stored resource with, end to end over the awsJson1_1 wire.
//!
//! The rule under test: an operation whose input carries an ARN naming the resource type
//! that member expects, and that resolves to no such resource, answers HTTP 400
//! `NotFoundException` whose message is that resource type's own fixed text followed by
//! the ARN echoed byte for byte from the request's own ARN member. Device Farm words that
//! sentence PER RESOURCE TYPE — `No networkprofile found for arn <arn>` and `No upload
//! found for arn <arn>` name the type by the very token its ARN carries, while a project
//! is worded `No project was found for arn <arn>` — so the wording belongs to the resource
//! type and not to the individual handler: every operation that reports the same type
//! missing words it identically, and none may normalise, re-case or re-order the ARN on
//! the way into it.
//!
//! The templates are spelled out here as literals, deliberately: this suite is what pins
//! them, so it may not ask the implementation (or a helper the implementation shares) what
//! the wording is.
//!
//! The rows differ along what the rule generalises over — which operation is called, which
//! input member carries the ARN, which resource the operation names, and the FORM of the
//! absent ARN (region, account, the shape and case of the resource id, both ends of the
//! modelled length range, and an ARN this emulator really minted and then released) — so no
//! single request is the specification.
//!
//! The wire harness is the one [`crate::projects`] owns and every suite in this crate
//! shares.

use serde_json::json;

use crate::projects::{arn_of, error_code, Emulator, REGION};

/// The resource type a project ARN carries, as it appears in the ARN's resource field.
const PROJECT_TYPE: &str = "project";

/// The resource type a network-profile ARN carries.
const NETWORK_PROFILE_TYPE: &str = "networkprofile";

/// The modelled bounds on every Device Farm ARN member, from
/// `smithy_query(query="operation", operation="GetProject")`: `AmazonResourceName`,
/// `@length` min 32 max 1011, `@pattern ^arn:aws:devicefarm:.+$`. An ARN at either bound
/// reaches the handler, so both are rows below.
const ARN_MIN_LENGTH: usize = 32;

/// The other end of that range.
const ARN_MAX_LENGTH: usize = 1011;

/// The resource type a device-pool ARN carries.
const DEVICE_POOL_TYPE: &str = "devicepool";

/// The resource type an upload ARN carries.
const UPLOAD_TYPE: &str = "upload";

/// One operation addressed by a Device Farm ARN: which member carries it, which resource
/// type that member names, the fixed text the refusal puts before the ARN, and the other
/// members the request must carry so the ARN is the only thing that can refuse it.
struct ArnCall {
    op: &'static str,
    member: &'static str,
    resource_type: &'static str,
    /// Everything the refusal's message says before the ARN, as a literal.
    absence_prefix: &'static str,
    extra: &'static [(&'static str, &'static str)],
}

/// The fixed text Device Farm puts before the ARN when a project ARN resolves to nothing.
const PROJECT_ABSENCE: &str = "No project was found for arn ";

/// The same, for a network profile: the ARN's own resource-type token, and no `was`.
const NETWORK_PROFILE_ABSENCE: &str = "No networkprofile found for arn ";

/// The same, for a device pool.
const DEVICE_POOL_ABSENCE: &str = "No devicepool found for arn ";

/// The same, for an upload.
const UPLOAD_ABSENCE: &str = "No upload found for arn ";

/// Every implemented operation of this service whose input carries an ARN member and that
/// models `NotFoundException` (`OPERATIONS.md`, the members typed `AmazonResourceName`).
/// `ListProjects`' `arn` is a filter naming one project, so it answers like `GetProject`;
/// `CreateNetworkProfile` names the owning PROJECT, which is why its wording is the
/// project one even though the operation is a network-profile call.
const ARN_CALLS: &[ArnCall] = &[
    ArnCall {
        op: "GetProject",
        member: "arn",
        resource_type: PROJECT_TYPE,
        absence_prefix: PROJECT_ABSENCE,
        extra: &[],
    },
    ArnCall {
        op: "ListProjects",
        member: "arn",
        resource_type: PROJECT_TYPE,
        absence_prefix: PROJECT_ABSENCE,
        extra: &[],
    },
    ArnCall {
        op: "UpdateProject",
        member: "arn",
        resource_type: PROJECT_TYPE,
        absence_prefix: PROJECT_ABSENCE,
        extra: &[("name", "renamed-by-the-refusal-row")],
    },
    ArnCall {
        op: "DeleteProject",
        member: "arn",
        resource_type: PROJECT_TYPE,
        absence_prefix: PROJECT_ABSENCE,
        extra: &[],
    },
    ArnCall {
        op: "CreateNetworkProfile",
        member: "projectArn",
        resource_type: PROJECT_TYPE,
        absence_prefix: PROJECT_ABSENCE,
        extra: &[("name", "profile-for-an-absent-project")],
    },
    ArnCall {
        op: "GetNetworkProfile",
        member: "arn",
        resource_type: NETWORK_PROFILE_TYPE,
        absence_prefix: NETWORK_PROFILE_ABSENCE,
        extra: &[],
    },
    ArnCall {
        op: "UpdateNetworkProfile",
        member: "arn",
        resource_type: NETWORK_PROFILE_TYPE,
        absence_prefix: NETWORK_PROFILE_ABSENCE,
        extra: &[("name", "renamed-by-the-refusal-row")],
    },
    ArnCall {
        op: "DeleteNetworkProfile",
        member: "arn",
        resource_type: NETWORK_PROFILE_TYPE,
        absence_prefix: NETWORK_PROFILE_ABSENCE,
        extra: &[],
    },
    ArnCall {
        op: "GetDevicePool",
        member: "arn",
        resource_type: DEVICE_POOL_TYPE,
        absence_prefix: DEVICE_POOL_ABSENCE,
        extra: &[],
    },
    ArnCall {
        op: "UpdateDevicePool",
        member: "arn",
        resource_type: DEVICE_POOL_TYPE,
        absence_prefix: DEVICE_POOL_ABSENCE,
        extra: &[("name", "renamed-by-the-refusal-row")],
    },
    ArnCall {
        op: "GetUpload",
        member: "arn",
        resource_type: UPLOAD_TYPE,
        absence_prefix: UPLOAD_ABSENCE,
        extra: &[],
    },
    ArnCall {
        op: "DeleteUpload",
        member: "arn",
        resource_type: UPLOAD_TYPE,
        absence_prefix: UPLOAD_ABSENCE,
        extra: &[],
    },
];

/// Device Farm's `NotFoundException` message for an ARN that resolves to no resource: the
/// resource type's own fixed text with the ARN the request supplied appended.
fn expected_absence_message(absence_prefix: &str, arn: &str) -> String {
    format!("{absence_prefix}{arn}")
}

/// A Device Farm ARN of `resource_type` whose resource id is `id`, in `region` under
/// `account`.
fn devicefarm_arn(region: &str, account: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:devicefarm:{region}:{account}:{resource_type}:{id}")
}

/// Well-formed ARNs of `resource_type` that no stored resource has, one per form the rule
/// generalises over.
///
/// Every id has the FORM an ARN of that type carries: a resource that lives inside a
/// project embeds the owning project's id ahead of its own, a project's id stands alone.
/// An ARN whose id is not of its type's form names no resource of that type at all, so it
/// is refused as a bad argument rather than reported missing — the subject of
/// [`crate::reference_checks`], not of this suite.
fn absent_arns(resource_type: &str) -> Vec<(String, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    // The owning project's id a project-scoped type's resource field carries, and the
    // empty prefix a top-level type's carries.
    let owner = if resource_type == PROJECT_TYPE {
        String::new()
    } else {
        "0000000000000f0f-absent/".to_string()
    };
    let id = |core: &str| format!("{owner}{core}");
    let mut rows = vec![
        (
            "a uuid-shaped id in the request's own region".to_string(),
            devicefarm_arn(
                "us-west-2",
                account,
                resource_type,
                &id("11111111-2222-3333-4444-555555555555"),
            ),
        ),
        (
            "another region".to_string(),
            devicefarm_arn(
                "eu-central-1",
                account,
                resource_type,
                &id("0123456789abcdef-absent"),
            ),
        ),
        (
            "another account".to_string(),
            devicefarm_arn(
                "us-west-2",
                "210987654321",
                resource_type,
                &id("0123456789abcdef-absent"),
            ),
        ),
        (
            "an UPPER-CASE id, which no normalisation may fold".to_string(),
            devicefarm_arn(
                "us-west-2",
                account,
                resource_type,
                &id("ABCDEF0123456789-ABSENT"),
            ),
        ),
        (
            if owner.is_empty() {
                "an id of the shape this emulator's own minting gives it".to_string()
            } else {
                "an id under ANOTHER absent owning resource".to_string()
            },
            devicefarm_arn(
                "us-west-2",
                account,
                resource_type,
                &if owner.is_empty() {
                    "000000000000000f-absent".to_string()
                } else {
                    "00000000000000cc-absent/00000000000000aa-absent".to_string()
                },
            ),
        ),
    ];

    // Both ends of the modelled length range, padded to land exactly on the bound where
    // the frame leaves room for it.
    let frame = devicefarm_arn("us-west-2", account, resource_type, &id("")).len();
    let shortest = ARN_MIN_LENGTH.saturating_sub(frame).max(1);
    rows.push((
        format!("the shortest id the modelled minimum length of {ARN_MIN_LENGTH} leaves room for"),
        devicefarm_arn("us-west-2", account, resource_type, &id(&"a".repeat(shortest))),
    ));
    rows.push((
        format!("an ARN exactly at the modelled maximum length of {ARN_MAX_LENGTH}"),
        devicefarm_arn(
            "us-west-2",
            account,
            resource_type,
            &id(&"b".repeat(ARN_MAX_LENGTH - frame)),
        ),
    ));
    rows
}

/// Every ARN-addressed operation words a resolvable-to-nothing ARN the way Device Farm
/// words that resource type's absence, over every form of ARN the model lets reach a
/// handler — including an ARN this emulator minted and then released.
#[tokio::test]
async fn absent_arns_are_refused_with_the_device_farm_not_found_sentence() {
    let emu = Emulator::new();

    // A live project and a live profile, so no refusal below can be "not found" merely
    // because the store is empty.
    let live_project = arn_of(&emu.create(&json!({ "name": "bystander" })).await);
    let live_profile = {
        let (status, response) = emu
            .call(
                "CreateNetworkProfile",
                REGION,
                &json!({ "projectArn": live_project, "name": "bystander-profile" }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case the live profile: expected CreateNetworkProfile to answer 200, got {status} \
             with {response}"
        );
        arn_of(&response["networkProfile"])
    };

    // An ARN this emulator really minted, then released: the store holds no record for it,
    // and its text is the emulator's own rather than this suite's.
    let released_project = arn_of(&emu.create(&json!({ "name": "released" })).await);
    let (status, response) = emu
        .call("DeleteProject", REGION, &json!({ "arn": released_project }))
        .await;
    assert_eq!(
        status, 200,
        "case releasing a project: expected DeleteProject to answer 200, got {status} with \
         {response}"
    );

    for call in ARN_CALLS {
        let mut arns = absent_arns(call.resource_type);
        if call.resource_type == PROJECT_TYPE {
            arns.push((
                "a project ARN this emulator minted and then released".to_string(),
                released_project.clone(),
            ));
        }

        for (form, arn) in &arns {
            let mut body = json!({});
            body[call.member] = json!(arn);
            for (member, value) in call.extra {
                body[*member] = json!(value);
            }
            let case = format!("{} with {form}", call.op);

            let (status, response) = emu.call(call.op, REGION, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {response}"
            );
            let expected = expected_absence_message(call.absence_prefix, arn);
            assert_eq!(
                response["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected the message {expected:?}, got {:?}",
                response["message"]
            );
        }
    }

    // None of the refusals disturbed the live records.
    for (op, arn, member) in [
        ("GetProject", &live_project, "project"),
        ("GetNetworkProfile", &live_profile, "networkProfile"),
    ] {
        let (status, response) = emu.call(op, REGION, &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case the live record after every refusal ({op}): expected 200, got {status} with \
             {response}"
        );
        assert_eq!(
            response[member]["arn"].as_str(),
            Some(arn.as_str()),
            "case the live record after every refusal ({op}): expected arn {arn:?}, got {:?}",
            response[member]["arn"]
        );
    }
}
