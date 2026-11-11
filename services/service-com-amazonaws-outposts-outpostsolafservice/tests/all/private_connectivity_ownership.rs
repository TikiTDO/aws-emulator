// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHERE an Outpost's private connectivity configuration
//! lives, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses) —
//! `MultiServiceRouter::dispatch` with a restJson1 request, exactly as a signed
//! `aws outposts` call arrives.
//!
//! [`crate::private_connectivity`] states what the two operations report; this suite
//! states the ownership rule behind it, which only a configuration computed from
//! PERSISTED state can satisfy:
//!
//! 1. The configuration is an attribute OF the Outpost record, not of the call that
//!    created it: it keeps answering what was stored across every later change to
//!    that Outpost that the API does not say touches it — a rename, a description, a
//!    hardware type, a tag, and the Outpost's own clock-owed lifecycle transition.
//! 2. The configuration belongs to that one record, so deleting the Outpost takes it
//!    with it: both routes then answer 404 `NotFoundException`, and an Outpost minted
//!    afterwards on the same site under the same name starts unconfigured and can be
//!    configured in its turn — the configuration is keyed to the record, never to the
//!    site or the name.

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{
    install_hardware, orderable_site, segment, text, Outposts,
};

/// The route both operations are bound to:
/// `(GET|POST) /outposts/{OutpostId}/privateConnectivity`, from
/// `smithy_query(query="operation",operation="GetPrivateConnectivityConfig,CreatePrivateConnectivityConfig")`.
fn route(outpost_id: &str) -> String {
    format!("/outposts/{outpost_id}/privateConnectivity")
}

/// The `LifeCycleStatus` an Outpost settles in once the hardware ordered for it has
/// been installed, as [`crate::site_and_outpost_projection`] states it.
const LIFE_CYCLE_ACTIVE: &str = "ACTIVE";

/// A `CreatePrivateConnectivityConfig` payload naming one VPC and one subnet. The
/// modelled `VpcInformationList` is `@length(min: 1, max: 1)`, so a valid request
/// carries exactly one entry.
fn request(vpc_id: &str, subnet_id: &str) -> Value {
    json!({
        "VpcInformationList": [{
            "VpcId": vpc_id,
            "SubnetIds": [subnet_id],
            "VpcEndpointId": "vpce-0a1b2c3d4e5f",
        }]
    })
}

/// The `PrivateConnectivityConfig` member of an output, or [`Value::Null`] when the
/// output carries none.
fn config_of(output: &Value) -> Value {
    output
        .get("PrivateConnectivityConfig")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One change made to an Outpost BETWEEN configuring its private connectivity and
/// reading that configuration back. Each variant is a call the API models against the
/// Outpost (or, for [`Change::Settle`], the install the service owes it once hardware
/// has been ordered) that says nothing about private connectivity.
#[derive(Debug)]
enum Change {
    /// Nothing at all: the baseline row.
    Nothing,
    /// `UpdateOutpost` with a new `Name`.
    Rename(&'static str),
    /// `UpdateOutpost` with a new `Description`.
    Describe(&'static str),
    /// `UpdateOutpost` with a `SupportedHardwareType` — one of the modelled enum's
    /// two values (`RACK | SERVER`, from
    /// `smithy_query(query="shape",shape="SupportedHardwareType")`).
    HardwareType(&'static str),
    /// `TagResource` on the Outpost's ARN.
    Tag(&'static str, &'static str),
    /// Hardware ordered for the Outpost and installed on it, which is what takes it
    /// from `PENDING` to `ACTIVE`.
    Settle,
}

impl Change {
    /// Apply the change, then assert the Outpost really did change: a row whose
    /// interleaved call did nothing would prove nothing about the configuration
    /// surviving it.
    async fn apply(&self, outposts: &Outposts, outpost_id: &str, outpost_arn: &str) {
        let case = self;
        match self {
            Change::Nothing => {}
            Change::Rename(name) => {
                let patched = outposts
                    .ok(
                        "PATCH",
                        &format!("/outposts/{outpost_id}"),
                        Some(&json!({ "Name": name })),
                    )
                    .await;
                let reported = text(&patched["Outpost"], "Name");
                assert_eq!(
                    reported, *name,
                    "case {case:?}: the rename must land, expected Name {name:?}, got {reported:?}"
                );
            }
            Change::Describe(description) => {
                let patched = outposts
                    .ok(
                        "PATCH",
                        &format!("/outposts/{outpost_id}"),
                        Some(&json!({ "Description": description })),
                    )
                    .await;
                let reported = text(&patched["Outpost"], "Description");
                assert_eq!(
                    reported, *description,
                    "case {case:?}: the description must land, expected {description:?}, got \
                     {reported:?}"
                );
            }
            Change::HardwareType(hardware_type) => {
                let patched = outposts
                    .ok(
                        "PATCH",
                        &format!("/outposts/{outpost_id}"),
                        Some(&json!({ "SupportedHardwareType": hardware_type })),
                    )
                    .await;
                let reported = text(&patched["Outpost"], "SupportedHardwareType");
                assert_eq!(
                    reported, *hardware_type,
                    "case {case:?}: the hardware type must land, expected {hardware_type:?}, got \
                     {reported:?}"
                );
            }
            Change::Tag(key, value) => {
                outposts
                    .no_content(
                        "POST",
                        &format!("/tags/{}", segment(outpost_arn)),
                        Some(&json!({ "Tags": { *key: *value } })),
                    )
                    .await;
                let outpost = outposts.get_outpost(outpost_id).await;
                let reported = text(&outpost["Tags"], key);
                assert_eq!(
                    reported, *value,
                    "case {case:?}: the tag must land, expected {value:?}, got {reported:?}"
                );
            }
            Change::Settle => {
                install_hardware(outposts, outpost_id).await;
                let outpost = outposts.get_outpost(outpost_id).await;
                let status = text(&outpost, "LifeCycleStatus");
                assert_eq!(
                    status, LIFE_CYCLE_ACTIVE,
                    "case {case:?}: the Outpost must have settled, expected LifeCycleStatus \
                     {LIFE_CYCLE_ACTIVE:?}, got {status:?}"
                );
            }
        }
    }
}

/// One site and one Outpost on it, as `(SiteId, OutpostId, OutpostArn)`.
async fn site_and_outpost(outposts: &Outposts, name: &str) -> (String, String, String) {
    let site = outposts
        .create_site(&orderable_site(&format!("{name} site")))
        .await;
    let site_id = text(&site, "SiteId");
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": site_id }))
        .await;
    (
        text(&site, "SiteId"),
        text(&outpost, "OutpostId"),
        text(&outpost, "OutpostArn"),
    )
}

/// Rule 1: the stored configuration is an attribute of the Outpost record, so every
/// change the API models against that Outpost leaves it reported exactly as it was.
///
/// The rows differ along WHICH change happens in between — none, each member
/// `UpdateOutpost` can write, a tag, and the lifecycle transition the clock owes the
/// Outpost itself.
#[tokio::test]
async fn the_stored_configuration_survives_every_other_change_to_its_outpost() {
    let cases = [
        ("no change in between", Change::Nothing),
        ("the Outpost renamed", Change::Rename("renamed outpost")),
        (
            "a description set on the Outpost",
            Change::Describe("configured for private connectivity"),
        ),
        (
            "the Outpost's hardware type set",
            Change::HardwareType("SERVER"),
        ),
        ("the Outpost tagged", Change::Tag("Env", "prod")),
        ("the Outpost settled to ACTIVE", Change::Settle),
    ];

    for (case, change) in &cases {
        let outposts = Outposts::new();
        let (_, outpost_id, outpost_arn) = site_and_outpost(&outposts, "connectivity owner").await;

        let created = outposts
            .ok(
                "POST",
                &route(&outpost_id),
                Some(&request("vpc-0a1b2c3d", "subnet-00001111")),
            )
            .await;
        let config = config_of(&created);
        assert_ne!(
            config,
            Value::Null,
            "case {case:?}: the create must report a configuration, got {created:?}"
        );

        change.apply(&outposts, &outpost_id, &outpost_arn).await;

        let read = config_of(&outposts.ok("GET", &route(&outpost_id), None).await);
        assert_eq!(
            read, config,
            "case {case:?}: the configuration must be reported unchanged afterwards, expected \
             {config:?}, got {read:?}"
        );
    }
}

/// Rule 2: the configuration goes away with the Outpost it belongs to, and a new
/// Outpost on the same site under the same name starts unconfigured.
///
/// The rows differ along which identifier form the delete named the Outpost with —
/// the two forms the modelled `OutpostId` pattern accepts.
#[tokio::test]
async fn deleting_the_outpost_takes_its_configuration_with_it() {
    let cases = [
        ("deleted by bare id", false),
        ("deleted by Outpost ARN", true),
    ];

    for (case, delete_by_arn) in cases {
        let outposts = Outposts::new();
        let name = "recreated outpost";
        let site = outposts
            .create_site(&json!({ "Name": format!("{name} site") }))
            .await;
        let site_id = text(&site, "SiteId");
        let first = outposts
            .create_outpost(&json!({ "Name": name, "SiteId": site_id.clone() }))
            .await;
        let (first_id, first_arn) = (text(&first, "OutpostId"), text(&first, "OutpostArn"));

        let config = config_of(
            &outposts
                .ok(
                    "POST",
                    &route(&first_id),
                    Some(&request("vpc-11112222", "subnet-22223333")),
                )
                .await,
        );
        let read = config_of(&outposts.ok("GET", &route(&first_id), None).await);
        assert_eq!(
            read, config,
            "case {case:?}: the configuration must be readable before the delete, got {read:?}"
        );

        let deleted_by = match delete_by_arn {
            true => segment(&first_arn),
            false => first_id.clone(),
        };
        let reply = outposts
            .call("DELETE", &format!("/outposts/{deleted_by}"), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the delete must succeed, got {} {}",
            reply.status, reply.body
        );

        // Nothing holds that Outpost any more, so neither route can answer for it.
        for (method, body) in [
            ("GET", None),
            ("POST", Some(request("vpc-33334444", "subnet-44445555"))),
        ] {
            let reply = outposts.call(method, &route(&first_id), body.as_ref()).await;
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: {method} on a deleted Outpost's privateConnectivity must be 404 \
                 NotFoundException, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }

        // A new Outpost on the SAME site under the SAME name is a different record,
        // so it carries none of the deleted one's configuration — and can be
        // configured itself rather than conflicting with a configuration that is gone.
        let second = outposts
            .create_outpost(&json!({ "Name": name, "SiteId": site_id }))
            .await;
        let second_id = text(&second, "OutpostId");
        assert_ne!(
            second_id, first_id,
            "case {case:?}: the replacement Outpost must be a new record, got {second_id:?}"
        );
        let output = outposts.ok("GET", &route(&second_id), None).await;
        assert_eq!(
            config_of(&output),
            Value::Null,
            "case {case:?}: the replacement Outpost must start unconfigured, got {output:?}"
        );

        let fresh = request("vpc-55556666", "subnet-66667777");
        let created = outposts.ok("POST", &route(&second_id), Some(&fresh)).await;
        assert_eq!(
            config_of(&created).get("VpcInformationList"),
            fresh.get("VpcInformationList"),
            "case {case:?}: the replacement Outpost must take its own configuration, got \
             {created:?}"
        );
        let read = config_of(&outposts.ok("GET", &route(&second_id), None).await);
        assert_eq!(
            read,
            config_of(&created),
            "case {case:?}: the replacement Outpost reports what was stored for it, got {read:?}"
        );
    }
}
