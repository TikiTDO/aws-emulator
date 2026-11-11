// SPDX-License-Identifier: CC0-1.0

//! The managed resources another service REGISTERED with ARC, driven end to end over the
//! wire.
//!
//! RULE: ARC models no operation that registers a managed resource — a resource becomes
//! one through the service that owns it, which turns zonal-shift integration on for it —
//! so every resource that service has published to the shared registry is a resource ARC
//! reports: `GetManagedResource` answers 200 for its ARN with one `appliedWeights` entry
//! per Availability Zone the resource is deployed in, `ListManagedResources` lists it, and
//! a zonal shift can be started, updated and read back for it. A resource the owner
//! deregisters — its integration turned off, or the resource itself gone — is a resource
//! ARC no longer reports.
//!
//! This suite drives the ARC side of that hand-off: the registry is written here the way
//! the owning service writes it (one shared store, keyed by type in the shared engine), and
//! every assertion below is made through real `restJson1` requests to this service. The
//! cross-service half — an Elastic Load Balancing load balancer whose
//! `zonal_shift.config.enabled` attribute does the registering, over EC2's own subnets —
//! is `core/router/tests/all/elbv2_zonal_shift_managed_resource.rs`.
//!
//! The rows differ along what the rule generalises over: the KIND of resource registered
//! (Network and Application Load Balancer, Auto Scaling group, EKS cluster), how MANY
//! Availability Zones it spans (one, two, three and a Region's full six), and the zone
//! traffic is then moved away from.

use std::collections::BTreeMap;

use service_common::arc_managed_resources::ManagedResourceRegistry;

use super::zonal_shift_lifecycle::{call, emulator, encoded, error_code, seeded, start_body};

/// Register `arn` with ARC the way the service that OWNS the resource does: by writing the
/// ONE shared registry in the shared engine.
pub(crate) fn register(engine: &engine::InMemoryEngine, arn: &str, name: &str, zones: &[&str]) {
    engine
        .state::<ManagedResourceRegistry>()
        .lock()
        .register(arn, name, zones.iter());
}

/// Withdraw a registration the way the owning service does when the integration is turned
/// off or the resource is deleted.
fn deregister(engine: &engine::InMemoryEngine, arn: &str) {
    engine
        .state::<ManagedResourceRegistry>()
        .lock()
        .deregister(arn);
}

/// The `appliedWeights` of a resource view, as a sorted map.
fn weights(resource_view: &serde_json::Value) -> BTreeMap<String, f64> {
    resource_view["appliedWeights"]
        .as_object()
        .unwrap_or_else(|| panic!("appliedWeights is required, got {resource_view}"))
        .iter()
        .map(|(zone, weight)| (zone.clone(), weight.as_f64().unwrap_or(f64::NAN)))
        .collect()
}

/// The weights a resource deployed in `zones` reports while traffic is moved away from
/// `away_from`: 0 there, 1 everywhere else.
fn expected_weights(zones: &[&str], away_from: Option<&str>) -> BTreeMap<String, f64> {
    zones
        .iter()
        .map(|zone| {
            let weight = if Some(*zone) == away_from { 0.0 } else { 1.0 };
            ((*zone).to_string(), weight)
        })
        .collect()
}

/// Read one managed resource over the wire.
async fn get_managed_resource(
    router: &smithy_server_runtime::MultiServiceRouter,
    arn: &str,
) -> (u16, serde_json::Value) {
    call(
        router,
        "GET",
        &format!("/managedresources/{}", encoded(arn)),
        "",
    )
    .await
}

/// Every `arn` `ListManagedResources` reports, walking the pages it hands out.
async fn listed_arns(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let mut arns = Vec::new();
    let mut query = String::new();
    loop {
        let (status, body) = call(router, "GET", &format!("/managedresources{query}"), "").await;
        assert_eq!(
            status, 200,
            "ListManagedResources{query}: expected 200, got {status} {body}"
        );
        for item in body["items"].as_array().cloned().unwrap_or_default() {
            arns.push(item["arn"].as_str().unwrap_or_default().to_string());
        }
        match body["nextToken"].as_str() {
            Some(token) => query = format!("?nextToken={}", encoded(token)),
            None => return arns,
        }
    }
}

/// One registered resource: the ARN its owner names it by, its name, and the zones it is
/// deployed in.
struct Registered {
    case: &'static str,
    arn: &'static str,
    name: &'static str,
    zones: &'static [&'static str],
}

/// The resources an owning service registers, spanning the resource kinds ARC documents
/// for zonal shift — "Amazon EC2 Auto Scaling groups[,] Amazon Elastic Kubernetes
/// Service[,] Application Load Balancer[,] Network Load Balancer" (the
/// `resourceIdentifier` documentation of `GetManagedResourceRequest`) — and the number of
/// Availability Zones one can span, from a single zone to a Region's six.
const REGISTERED: &[Registered] = &[
    Registered {
        case: "a Network Load Balancer in two zones",
        arn: "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/net/registered-nlb/0123456789abcdef",
        name: "registered-nlb",
        zones: &["usw2-az1", "usw2-az2"],
    },
    Registered {
        case: "an Application Load Balancer in three zones",
        arn: "arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/registered-alb/fedcba9876543210",
        name: "registered-alb",
        zones: &["usw2-az1", "usw2-az2", "usw2-az3"],
    },
    Registered {
        case: "an Auto Scaling group in one zone",
        arn: "arn:aws:autoscaling:us-west-2:000000000000:autoScalingGroup:11111111-2222-3333-4444-555555555555:autoScalingGroupName/registered-asg",
        name: "registered-asg",
        zones: &["usw2-az4"],
    },
    Registered {
        case: "an EKS cluster across a Region's six zones",
        arn: "arn:aws:eks:us-east-1:000000000000:cluster/registered-eks",
        name: "registered-eks",
        zones: &[
            "use1-az1", "use1-az2", "use1-az3", "use1-az4", "use1-az5", "use1-az6",
        ],
    },
];

#[tokio::test]
async fn a_resource_its_owner_registered_is_reported_as_a_managed_resource() {
    let (router, engine) = emulator();
    // Every resource ARC holds before anything is registered: the registrations must ADD
    // to them rather than replace them.
    let seeded_arns: Vec<String> = seeded(&engine).into_iter().map(|(arn, _)| arn).collect();

    for resource in REGISTERED {
        let case = resource.case;
        // Before the owner registers it, ARC holds no record of the ARN at all.
        let (status, body) = get_managed_resource(&router, resource.arn).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case}: an unregistered resource is not found, got {status} {body}"
        );

        register(&engine, resource.arn, resource.name, resource.zones);

        let (status, view) = get_managed_resource(&router, resource.arn).await;
        assert_eq!(
            status, 200,
            "case {case}: a registered resource answers 200, got {status} {view}"
        );
        assert_eq!(
            (view["arn"].as_str(), view["name"].as_str()),
            (Some(resource.arn), Some(resource.name)),
            "case {case}: expected the registered ARN and name, got {view}"
        );
        assert_eq!(
            weights(&view),
            expected_weights(resource.zones, None),
            "case {case}: expected weight 1 in each of {:?}, got {:?}",
            resource.zones,
            weights(&view)
        );
        assert_eq!(
            view["zonalShifts"].as_array().map(Vec::len),
            Some(0),
            "case {case}: a resource with no shift in progress lists none, got {view}"
        );

        let listed = listed_arns(&router).await;
        assert!(
            listed.contains(&resource.arn.to_string()),
            "case {case}: expected ListManagedResources to report the registered resource, \
             got {listed:?}"
        );
        for seeded_arn in &seeded_arns {
            assert!(
                listed.contains(seeded_arn),
                "case {case}: expected the resources ARC already held to stay listed, \
                 {seeded_arn} is missing from {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_shift_runs_through_its_whole_life_on_a_registered_resource() {
    for resource in REGISTERED {
        let case = resource.case;
        // One emulator per row, so each row's assertions are about its own store.
        let (router, engine) = emulator();
        register(&engine, resource.arn, resource.name, resource.zones);

        // Traffic is moved away from the LAST zone the resource spans, so the row with one
        // zone and the row with six each name a zone of their own.
        let away_from = resource.zones[resource.zones.len() - 1];
        let (status, started) = call(
            &router,
            "POST",
            "/zonalshifts",
            &start_body(resource.arn, away_from, "1h", "a shift on a registered resource"),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case}: StartZonalShift answers 201, got {status} {started}"
        );
        let shift_id = started["zonalShiftId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no zonalShiftId in {started}"))
            .to_string();
        let first_expiry = started["expiryTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: no expiryTime in {started}"));

        // The resource read reflects it: the zone traffic moved away from carries 0.
        let (status, view) = get_managed_resource(&router, resource.arn).await;
        assert_eq!(
            status, 200,
            "case {case}: GetManagedResource answers 200, got {status} {view}"
        );
        assert_eq!(
            weights(&view),
            expected_weights(resource.zones, Some(away_from)),
            "case {case}: expected weight 0 for {away_from} and 1 elsewhere, got {:?}",
            weights(&view)
        );

        // An update extends the expiry and overwrites the comment, and the resource read
        // shows both — the shift is reported through the resource ARC did not create.
        let updated_comment = "extended on a registered resource";
        let (status, updated) = call(
            &router,
            "PATCH",
            &format!("/zonalshifts/{shift_id}"),
            &serde_json::json!({ "comment": updated_comment, "expiresIn": "3h" }).to_string(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateZonalShift answers 200, got {status} {updated}"
        );
        let second_expiry = updated["expiryTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: no expiryTime in {updated}"));
        assert_eq!(
            (updated["status"].as_str(), updated["comment"].as_str()),
            (Some("ACTIVE"), Some(updated_comment)),
            "case {case}: an updated shift stays ACTIVE and carries the new comment, \
             got {updated}"
        );
        assert!(
            second_expiry > first_expiry,
            "case {case}: expected an expiryTime past {first_expiry}, got {second_expiry}"
        );

        let (status, view) = get_managed_resource(&router, resource.arn).await;
        assert_eq!(
            status, 200,
            "case {case}: GetManagedResource answers 200, got {status} {view}"
        );
        let in_resource = view["zonalShifts"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: zonalShifts is required, got {view}"));
        assert_eq!(
            in_resource.len(),
            1,
            "case {case}: exactly the one shift in progress is listed, got {in_resource:?}"
        );
        assert_eq!(
            (
                in_resource[0]["zonalShiftId"].as_str(),
                in_resource[0]["comment"].as_str(),
                in_resource[0]["expiryTime"].as_f64(),
                in_resource[0]["appliedStatus"].as_str(),
            ),
            (
                Some(shift_id.as_str()),
                Some(updated_comment),
                Some(second_expiry),
                Some("APPLIED"),
            ),
            "case {case}: expected the updated shift reported through the resource, \
             got {in_resource:?}"
        );

        // Cancelled, the shift moves no weight any more, and the resource stays managed.
        let (status, canceled) =
            call(&router, "DELETE", &format!("/zonalshifts/{shift_id}"), "").await;
        assert_eq!(
            status, 200,
            "case {case}: CancelZonalShift answers 200, got {status} {canceled}"
        );
        let (_, view) = get_managed_resource(&router, resource.arn).await;
        assert_eq!(
            weights(&view),
            expected_weights(resource.zones, None),
            "case {case}: a canceled shift moves no weight, got {:?}",
            weights(&view)
        );
    }
}

#[tokio::test]
async fn arc_follows_the_registry_when_the_owner_changes_or_withdraws_a_registration() {
    let (router, engine) = emulator();
    let resource = &REGISTERED[0];
    let case = resource.case;
    register(&engine, resource.arn, resource.name, resource.zones);

    // The owner records the resource again with a changed name and a changed zone set —
    // a load balancer given different subnets — and ARC reports what the owner holds now.
    let rows: &[(&str, &str, &[&str])] = &[
        ("one zone added", "registered-nlb", &["usw2-az1", "usw2-az2", "usw2-az3"]),
        ("one zone left", "registered-nlb", &["usw2-az3"]),
        ("renamed", "renamed-nlb", &["usw2-az1", "usw2-az4"]),
    ];
    for (change, name, zones) in rows {
        register(&engine, resource.arn, name, zones);
        let (status, view) = get_managed_resource(&router, resource.arn).await;
        assert_eq!(
            status, 200,
            "case {case}, {change}: expected 200, got {status} {view}"
        );
        assert_eq!(
            (weights(&view), view["name"].as_str()),
            (expected_weights(zones, None), Some(*name)),
            "case {case}, {change}: expected the owner's current facts, got {view}"
        );
    }

    // Withdrawn — the integration turned off, or the resource deleted — it is a resource
    // ARC holds no record of again, and the resources ARC already held are untouched.
    deregister(&engine, resource.arn);
    let (status, body) = get_managed_resource(&router, resource.arn).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case}: a deregistered resource is not found again, got {status} {body}"
    );
    let listed = listed_arns(&router).await;
    assert!(
        !listed.contains(&resource.arn.to_string()),
        "case {case}: expected ListManagedResources to drop it, got {listed:?}"
    );
    for (seeded_arn, zones) in seeded(&engine) {
        let (status, view) = get_managed_resource(&router, &seeded_arn).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the resources ARC already held to answer 200 for \
             {seeded_arn}, got {status} {view}"
        );
        let zones: Vec<&str> = zones.iter().map(String::as_str).collect();
        assert_eq!(
            weights(&view),
            expected_weights(&zones, None),
            "case {case}: expected {seeded_arn} to keep its own zones, got {:?}",
            weights(&view)
        );
    }

    // And a shift cannot be started for it any more: the target is one ARC holds nothing
    // for, which is refused before existence is reported.
    let (status, body) = call(
        &router,
        "POST",
        "/zonalshifts",
        &start_body(resource.arn, "usw2-az1", "1h", "after the registration was withdrawn"),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "case {case}: a shift for a deregistered resource is refused, got {status} {body}"
    );
}
