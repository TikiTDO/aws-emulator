// SPDX-License-Identifier: CC0-1.0

//! The launch rules `CreateWorkspaceInstance` applies to a `ManagedInstance`, driven
//! over the wire this service really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.CreateWorkspaceInstance`).
//!
//! The rule under test, stated once and asserted over a SET of inputs rather than one
//! request:
//!
//! > a create whose `ManagedInstance` does not describe one launch this service can
//! > carry out — no image or no instance type, an instance type the service's own
//! > catalog does not publish, a tenancy or billing mode that type publishes no
//! > supported configuration for, or a subnet named both on the instance and on its
//! > network interfaces — is refused with the modelled `ValidationException`
//! > (HTTP 400) naming the offending member, and stores nothing; every other
//! > configuration is launched and reported back out of the store.
//!
//! The rows differ along what that rule generalises over: which member is missing,
//! which part of an instance-type name is off the catalog (an unpublished family, and
//! a published family at a size it is not offered in), where in the catalog the
//! accepted type sits (its first, middle and last rows, so the types the issue never
//! cited are covered), every `TenancyEnum` value against a shared-only family and a
//! family offered on dedicated hardware, every `BillingMode` and the absent default,
//! and where a subnet is named.
//!
//! Nothing here can pass against a canned answer: an accepted create is only counted
//! once `GetWorkspaceInstance` resolves the minted id out of the store, and a refusal
//! is only counted once the store is seen to hold exactly what it held before.

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{
    call, error_code, harness, launchable_managed_instance, ok, TEST_IMAGE_ID, TEST_INSTANCE_TYPE,
};

/// The `ValidationExceptionReason` a single bad member is reported with, as the
/// modelled enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// How many instances the service's own store holds — the engine the router was built
/// from, read the one legal way.
fn stored_instances(engine: &engine::InMemoryEngine) -> usize {
    wsinst::state::service_state(engine)
        .expect("the service's store is reachable from the engine the router was built from")
        .lock()
        .instances
        .len()
}

/// Assert `body` is refused with the modelled `ValidationException`, that the refusal
/// names `member` both in its wording and in `FieldList`, and that the store is
/// untouched.
async fn assert_refused(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    member: &str,
    body: Value,
) {
    let held = stored_instances(engine);
    let (status, payload) = call(router, "CreateWorkspaceInstance", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {payload}"
    );
    assert_eq!(
        payload["Reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case:?}: expected the single-member reason, got {payload}"
    );
    let named: Vec<&str> = payload["FieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no FieldList in {payload}"))
        .iter()
        .map(|field| field["Name"].as_str().unwrap_or("<no Name>"))
        .collect();
    assert_eq!(
        named,
        vec![member],
        "case {case:?}: expected the refusal to name {member:?}, got {payload}"
    );
    let message = payload["Message"]
        .as_str()
        .or_else(|| payload["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: no Message in {payload}"));
    assert!(
        message.contains(member),
        "case {case:?}: expected the wording to name {member:?}, got {message:?}"
    );
    assert_eq!(
        stored_instances(engine),
        held,
        "case {case:?}: a refused create must store nothing"
    );
}

/// Assert `body` launches an instance, proving it reached the store by reading the
/// minted id back through `GetWorkspaceInstance`: the instance is in one of the
/// allocation states the clock moves it through, and carries neither error list,
/// because this service launched exactly the configuration it accepted.
async fn assert_created(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    body: Value,
) -> String {
    let held = stored_instances(engine);
    let (status, payload) = call(router, "CreateWorkspaceInstance", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {payload}"
    );
    let id = payload["WorkspaceInstanceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstanceId in {payload}"))
        .to_string();
    assert_eq!(
        stored_instances(engine),
        held + 1,
        "case {case:?}: an accepted create must store one instance"
    );

    let (status, got) = call(
        router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": &id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the minted {id:?} must resolve in the store, got {status} with {got}"
    );
    assert_eq!(
        got["WorkspaceInstanceId"].as_str(),
        Some(id.as_str()),
        "case {case:?}: expected the stored id {id:?}, got {got}"
    );
    let state = got["ProvisionState"].as_str().unwrap_or("<none>");
    assert!(
        [
            wsinst::state::PROVISION_STATE_ALLOCATING,
            wsinst::state::PROVISION_STATE_ALLOCATED,
        ]
        .contains(&state),
        "case {case:?}: expected an allocating or allocated instance, got {state:?} in {got}"
    );
    assert!(
        got.get("WorkspaceInstanceErrors").is_none() && got.get("EC2InstanceErrors").is_none(),
        "case {case:?}: a launched instance carries no lifecycle error, got {got}"
    );
    id
}

/// Assert `body` is ACCEPTED and stored, and that the launch it asks for is one Amazon
/// EC2 itself refuses: the WorkSpace Instance reaches the terminal `ERROR_ALLOCATING`,
/// reports EC2's own error code through the modelled `EC2InstanceErrors`, and reports no
/// managed instance, there being none.
///
/// This service's own rules decide which configurations it STORES; the service that owns
/// instances decides which launches it carries out, so a configuration past one of ITS
/// rules is reported this way rather than as a refusal of the create.
async fn assert_launch_refused_by_ec2(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    body: Value,
    expected_code: &str,
) {
    let held = stored_instances(engine);
    let (status, payload) = call(router, "CreateWorkspaceInstance", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create to be accepted, got {status} with {payload}"
    );
    let id = payload["WorkspaceInstanceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no WorkspaceInstanceId in {payload}"))
        .to_string();
    assert_eq!(
        stored_instances(engine),
        held + 1,
        "case {case:?}: an accepted create must store one instance"
    );

    let got = ok(
        router,
        "GetWorkspaceInstance",
        json!({ "WorkspaceInstanceId": &id }),
    )
    .await;
    assert_eq!(
        got["ProvisionState"].as_str(),
        Some(wsinst::state::PROVISION_STATE_ERROR_ALLOCATING),
        "case {case:?}: expected the failed allocation to be reported, got {got}"
    );
    assert_eq!(
        got["EC2InstanceErrors"][0]["EC2ErrorCode"].as_str(),
        Some(expected_code),
        "case {case:?}: expected EC2's own error code, got {got}"
    );
    assert!(
        got["EC2ManagedInstance"].is_null(),
        "case {case:?}: a launch that was refused has no managed instance, got {got}"
    );
}

/// A `ManagedInstance` with `members` merged onto the minimal launchable one.
fn configuration(members: Value) -> Value {
    let mut managed_instance = launchable_managed_instance();
    for (member, value) in members
        .as_object()
        .expect("a configuration row is an object")
        .clone()
    {
        managed_instance[member] = value;
    }
    managed_instance
}

#[tokio::test]
async fn a_launch_needs_the_image_it_boots_and_the_instance_type_it_runs_on() {
    let (router, engine) = harness();

    // Rows that name neither, or only one, of the two members a launch cannot be made
    // without — each with a different amount of OTHER configuration around them, so the
    // refusal turns on the missing member and not on the request's size or shape.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("no configuration at all", "ManagedInstance.ImageId", json!({})),
        (
            "an instance type but no image",
            "ManagedInstance.ImageId",
            json!({ "InstanceType": TEST_INSTANCE_TYPE }),
        ),
        (
            "every other member set, but no image",
            "ManagedInstance.ImageId",
            json!({
                "InstanceType": "m5.large",
                "SubnetId": "subnet-0a1b2c3d",
                "SecurityGroupIds": ["sg-0000000a"],
                "KeyName": "my-key",
                "EbsOptimized": true,
                "Placement": { "AvailabilityZone": "us-east-1a", "Tenancy": "default" },
            }),
        ),
        (
            "an image but no instance type",
            "ManagedInstance.InstanceType",
            json!({ "ImageId": TEST_IMAGE_ID }),
        ),
        (
            "an image with nested configuration, but no instance type",
            "ManagedInstance.InstanceType",
            json!({
                "ImageId": TEST_IMAGE_ID,
                "CpuOptions": { "CoreCount": 2, "ThreadsPerCore": 2 },
                "MetadataOptions": { "HttpTokens": "required" },
                "NetworkInterfaces": [{ "DeviceIndex": 0, "SubnetId": "subnet-0a1b2c3d" }],
            }),
        ),
    ];
    for (case, member, managed_instance) in rows {
        assert_refused(
            &router,
            &engine,
            case,
            member,
            json!({ "ManagedInstance": managed_instance }),
        )
        .await;
    }

    // Both members together are a launch, with or without the rest of the
    // configuration around them.
    for (case, managed_instance) in [
        ("both members alone", launchable_managed_instance()),
        (
            "both members with the rest of a configuration",
            configuration(json!({
                "SubnetId": "subnet-0a1b2c3d",
                "SecurityGroupIds": ["sg-0000000a", "sg-0000000b"],
                "UserData": "echo hello",
                "BlockDeviceMappings": [
                    { "DeviceName": "/dev/sda1", "Ebs": { "VolumeType": "gp3", "VolumeSize": 100 } }
                ],
            })),
        ),
    ] {
        assert_created(
            &router,
            &engine,
            case,
            json!({ "ManagedInstance": managed_instance }),
        )
        .await;
    }
}

#[tokio::test]
async fn the_instance_type_must_be_one_the_catalog_publishes() {
    let (router, engine) = harness();

    // Names the modelled `InstanceType` pattern `^([a-z0-9-]+)\.([a-z0-9]+)$` admits
    // and `ListInstanceTypes` publishes nowhere: an unpublished family, a family at a
    // size it is not offered in (`c5` steps through 9xlarge/18xlarge, not
    // 8xlarge/16xlarge), the bare family name used as a size, and a size alone.
    let unpublished = [
        ("a family that exists nowhere", "zz9.plural"),
        ("another unpublished family", "qq7.large"),
        ("a burstable family at an unoffered size", "t3.gigantic"),
        ("c5 at a size it is not offered in", "c5.8xlarge"),
        ("a published size under no family", "nosuchfamily.medium"),
    ];
    for (case, instance_type) in unpublished {
        assert!(
            wsinst::state::instance_type_spec(instance_type).is_none(),
            "case {case:?}: {instance_type:?} must be absent from the catalog this test \
             reads, got a row for it"
        );
        assert_refused(
            &router,
            &engine,
            case,
            "ManagedInstance.InstanceType",
            json!({ "ManagedInstance": configuration(json!({ "InstanceType": instance_type })) }),
        )
        .await;
    }

    // And rows the catalog DOES publish are launched. Taken from the catalog itself —
    // its first, middle and last rows — so the types this issue never cited are
    // covered alongside the cited one.
    let catalog = wsinst::state::instance_type_catalog();
    assert!(
        catalog.len() > 2,
        "case catalog: the published catalog must have rows to sample, got {}",
        catalog.len()
    );
    let sampled = [
        ("the first published type", &catalog[0]),
        ("a middle published type", &catalog[catalog.len() / 2]),
        (
            "the last published type",
            catalog.last().expect("the catalog is not empty"),
        ),
    ];
    for (case, spec) in sampled {
        assert_created(
            &router,
            &engine,
            &format!("{case} ({})", spec.instance_type),
            json!({
                "ManagedInstance": configuration(json!({ "InstanceType": spec.instance_type })),
            }),
        )
        .await;
    }
}

#[tokio::test]
async fn the_tenancy_and_billing_mode_must_be_ones_the_named_type_publishes() {
    let (router, engine) = harness();

    // Two catalogued families: one offered on shared hardware only, and one offered on
    // dedicated hardware too — so the same rows are accepted for one and refused for
    // the other, and the expectation comes from the catalog row rather than from a
    // list written out here.
    let instance_types = ["t3.medium", "m5.large"];
    // Every `TenancyEnum` value the model declares, plus the absent member (EC2 places
    // an instance on shared hardware when the request names no tenancy).
    let mut placement_tenancies: Vec<Option<&str>> = vec![None];
    placement_tenancies.extend(
        wsinst::ops::types::TENANCY_ENUM_VALUES
            .iter()
            .map(|tenancy| Some(*tenancy)),
    );
    // Every `BillingMode` the model declares, plus the absent member (the documented
    // hourly default).
    let mut billing_modes: Vec<Option<&str>> = vec![None];
    billing_modes.extend(
        wsinst::ops::types::BILLING_MODE_VALUES
            .iter()
            .map(|mode| Some(*mode)),
    );

    for instance_type in instance_types {
        let spec = wsinst::state::instance_type_spec(instance_type)
            .unwrap_or_else(|| panic!("case {instance_type}: the type must be catalogued"));
        for placement_tenancy in &placement_tenancies {
            let tenancy = wsinst::state::configuration_tenancy(
                placement_tenancy.unwrap_or(wsinst::state::PLACEMENT_TENANCY_DEFAULT),
            );
            for billing_mode in &billing_modes {
                let mode = billing_mode.unwrap_or(wsinst::state::DEFAULT_BILLING_MODE);
                let case = format!(
                    "{instance_type} with Tenancy {placement_tenancy:?} and BillingMode \
                     {billing_mode:?}"
                );

                let mut managed_instance = configuration(json!({ "InstanceType": instance_type }));
                if let Some(placement_tenancy) = placement_tenancy {
                    managed_instance["Placement"] = json!({ "Tenancy": placement_tenancy });
                }
                let mut body = json!({ "ManagedInstance": managed_instance });
                if let Some(billing_mode) = billing_mode {
                    body["BillingConfiguration"] = json!({ "BillingMode": billing_mode });
                }

                if !spec.supports_tenancy(tenancy) {
                    assert_refused(
                        &router,
                        &engine,
                        &case,
                        "ManagedInstance.Placement.Tenancy",
                        body,
                    )
                    .await;
                } else if !spec.supports_billing_mode(mode, tenancy) {
                    assert_refused(
                        &router,
                        &engine,
                        &case,
                        "BillingConfiguration.BillingMode",
                        body,
                    )
                    .await;
                } else {
                    assert_created(&router, &engine, &case, body).await;
                }
            }
        }
    }
}

#[tokio::test]
async fn a_launch_names_its_subnet_in_one_place() {
    let (router, engine) = harness();

    // A subnet on the instance AND on a network interface describes two placements for
    // one instance, whichever interface carries it.
    for (case, interfaces) in [
        (
            "one interface, with a subnet",
            json!([{ "DeviceIndex": 0, "SubnetId": "subnet-0a1b2c3d" }]),
        ),
        (
            "two interfaces, the second with a subnet",
            json!([
                { "DeviceIndex": 0 },
                { "DeviceIndex": 1, "SubnetId": "subnet-0a1b2c3d" },
            ]),
        ),
    ] {
        assert_refused(
            &router,
            &engine,
            case,
            "ManagedInstance.SubnetId",
            json!({
                "ManagedInstance": configuration(json!({
                    "SubnetId": "subnet-0e1f2a3b",
                    "NetworkInterfaces": interfaces,
                })),
            }),
        )
        .await;
    }

    // Either place alone is one placement, and an interface that names no subnet
    // leaves the instance-level one the only subnet in the request.
    for (case, members) in [
        (
            "a subnet on the instance alone",
            json!({ "SubnetId": "subnet-0e1f2a3b" }),
        ),
        (
            "a subnet inside the network interfaces alone",
            json!({
                "NetworkInterfaces": [{ "DeviceIndex": 0, "SubnetId": "subnet-0a1b2c3d" }],
            }),
        ),
        ("no subnet at all", json!({})),
    ] {
        assert_created(
            &router,
            &engine,
            case,
            json!({ "ManagedInstance": configuration(members) }),
        )
        .await;
    }

    // An instance-level subnet BESIDE network interfaces passes this service's rule —
    // the interface names no subnet, so the request names one placement — and is stored.
    // The launch itself is Amazon EC2's, and EC2 refuses that combination outright ("if
    // you specify a network interface, you must specify any subnets as part of the
    // network interface instead of using this parameter", `RunInstances$SubnetId`), so
    // the WorkSpace Instance reports EC2's refusal rather than an allocation nothing
    // carried out.
    assert_launch_refused_by_ec2(
        &router,
        &engine,
        "an instance subnet beside an interface that names none",
        json!({
            "ManagedInstance": configuration(json!({
                "SubnetId": "subnet-0e1f2a3b",
                "NetworkInterfaces": [{ "DeviceIndex": 0, "Groups": ["sg-0000000a"] }],
            })),
        }),
        "InvalidParameterCombination",
    )
    .await;
}

#[tokio::test]
async fn a_refused_configuration_neither_replays_a_token_nor_spends_quota() {
    let (router, engine) = harness();
    let token = "tok-refused-configuration";

    // A refused create is refused before the store is read, so the token it carried
    // names no instance afterwards: the next LAUNCHABLE create with that token mints
    // one.
    assert_refused(
        &router,
        &engine,
        "a refused create carrying an idempotency token",
        "ManagedInstance.ImageId",
        json!({ "ClientToken": token, "ManagedInstance": {} }),
    )
    .await;

    let id = assert_created(
        &router,
        &engine,
        "the token after a refusal",
        json!({
            "ClientToken": token,
            "ManagedInstance": launchable_managed_instance(),
        }),
    )
    .await;

    // And the refusal is still a refusal once the token names an instance: a
    // configuration this service cannot launch is never answered with the stored id.
    assert_refused(
        &router,
        &engine,
        "a refused create carrying a token that now names an instance",
        "ManagedInstance.InstanceType",
        json!({
            "ClientToken": token,
            "ManagedInstance": configuration(json!({ "InstanceType": "zz9.plural" })),
        }),
    )
    .await;

    assert_eq!(
        stored_instances(&engine),
        1,
        "case the whole sequence: exactly the one launchable create is stored beside {id:?}"
    );
}
