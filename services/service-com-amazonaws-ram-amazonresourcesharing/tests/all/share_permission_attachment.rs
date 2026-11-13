// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `AssociateResourceSharePermission` and what it makes
//! `ListResourceSharePermissions` report.
//!
//! The rule the rows below pin: a managed permission attached to a resource share
//! becomes part of that share's permission set — a share is governed by the permissions
//! it was created with, the ones attached to it afterwards, and the default permission
//! of every resource type it shares that none of those applies to — and a read of the
//! share's permissions reports each of them at the version the share attached, with
//! `status` reporting the attachment to that share rather than the version's own
//! attachability.
//!
//! So the rows vary along what the rule generalises over:
//!
//! * WHICH permission is attached: a customer managed one a create minted, an AWS
//!   managed one that is not its resource type's default, and several of different
//!   resource types;
//! * WHAT the share already carries: nothing, a permission the create named, the derived
//!   default of a type it shares, and a shared resource type the catalog publishes no
//!   permission for at all;
//! * HOW MANY attachments one page holds: a page size below, at and above the number
//!   attached;
//! * WHICH VERSION of the permission the share is governed by: the one it attached, which
//!   a later version of the permission does not move, and the one a replacing attach
//!   updates it to;
//! * WHAT an attach that cannot be made answers: a value that is not an ARN of the
//!   resource it names, a share or permission nothing answers to, a deleted share, a
//!   version the permission does not carry or that is not its default, a second
//!   permission for a resource type the share already has one for, and a `clientToken`
//!   reused for a different request.
//!
//! Writing a version of a permission is what makes "the version the share attached"
//! observable at all, so this suite drives `CreatePermissionVersion` too (its own stub
//! issue: 20261103T040218Z-ram-createpermissionversion-stub.md).

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    aws_managed_permission_arn, FIRST_PERMISSION_VERSION, MAX_ENTITIES_PER_CALL,
    MAX_PERMISSION_VERSIONS, PERMISSION_FEATURE_SET_STANDARD, PERMISSION_TYPE_CUSTOMER_MANAGED,
    SHARE_PERMISSION_STATUS_ASSOCIATED,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// The resource type the recorded lifecycle shared a customer managed permission for.
/// `ListResourceTypes` reports it (`RESOURCE_TYPES`) and the AWS managed catalog
/// publishes no permission for it, so a share of one carries no derived default.
const COMPONENT_TYPE: &str = "imagebuilder:Component";
/// An Image Builder component: a resource of [`COMPONENT_TYPE`].
const COMPONENT: &str = "arn:aws:imagebuilder:us-west-2:000000000000:component/hardening/1.0.0/1";
/// A Glue database, a resource type the catalog publishes two permissions for.
const GLUE_DATABASE: &str = "arn:aws:glue:us-west-2:000000000000:database/analytics";

async fn attach(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/associateresourcesharepermission",
        Some(body),
    )
    .await
}

async fn list_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listresourcesharepermissions", Some(body)).await
}

async fn create_permission_version(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermissionversion", Some(body)).await
}

async fn get_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getpermission", Some(body)).await
}

/// A policy template allowing `action`, as both permission writes take one.
fn policy_template(action: &str) -> String {
    json!({"Effect": "ALLOW", "Action": [action]}).to_string()
}

/// The ARN of the AWS managed permission of this name.
fn managed(name: &str) -> String {
    aws_managed_permission_arn(name)
}

/// The ARN a customer managed permission of this name is minted under.
fn customer(name: &str) -> String {
    format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/{name}")
}

/// Mint a customer managed permission for `resource_type` and answer its ARN.
async fn mint(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    resource_type: &str,
    action: &str,
) -> String {
    let (status, minted) = call(
        router,
        "POST",
        "/createpermission",
        Some(&json!({
            "name": name,
            "resourceType": resource_type,
            "policyTemplate": policy_template(action),
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the permission create, got {status} with {minted:?}"
    );
    customer(name)
}

/// Create a share and answer its ARN.
async fn share_with(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let created = create_ok(router, case, request).await;
    arn_of(case, &created)
}

/// The permissions a read reported.
fn permissions_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permissions, got {body:?}"))
}

/// A string member of a reported permission.
fn field(case: &str, permission: &Value, member: &str) -> String {
    permission
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
        .to_string()
}

/// A boolean member of a reported permission.
fn flag(case: &str, permission: &Value, member: &str) -> bool {
    permission
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} member, got {permission:?}"))
}

/// Read every permission of `share_arn`, asserting HTTP 200 and that each one reports
/// its attachment to the share, and answer them in the order they were reported.
async fn listed(router: &MultiServiceRouter, case: &str, share_arn: &str) -> Vec<Value> {
    let (status, body) = list_permissions(router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body:?}"
    );
    let reported = permissions_of(case, &body);
    for permission in &reported {
        assert_eq!(
            field(case, permission, "status"),
            SHARE_PERMISSION_STATUS_ASSOCIATED,
            "case {case:?}: a permission the share holds reports its attachment, got \
             {permission:?}"
        );
    }
    reported
}

/// The ARNs of every permission `share_arn` reports.
async fn listed_arns(router: &MultiServiceRouter, case: &str, share_arn: &str) -> Vec<String> {
    listed(router, case, share_arn)
        .await
        .iter()
        .map(|permission| field(case, permission, "arn"))
        .collect()
}

/// Attach `permission_arn` to `share_arn`, asserting the attach succeeded.
async fn attach_ok(router: &MultiServiceRouter, case: &str, share_arn: &str, permission_arn: &str) {
    let (status, answered) = attach(
        router,
        &json!({"resourceShareArn": share_arn, "permissionArn": permission_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 attaching {permission_arn}, got {status} with \
         {answered:?}"
    );
    assert_eq!(
        answered.get("returnValue").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: a successful attach reports returnValue true, got {answered:?}"
    );
    assert!(
        answered
            .get("clientToken")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.is_empty()),
        "case {case:?}: an attach reports the idempotency token it used, got {answered:?}"
    );
}

/// One row of the attachment table.
struct AttachRow {
    case: &'static str,
    /// The resources the share shares, so which defaults it derives varies.
    resource_arns: Vec<&'static str>,
    /// The permissions the create request named.
    created_with: Vec<String>,
    /// The permissions attached after creation, in order.
    attached: Vec<String>,
    /// Every permission ARN the share must report afterwards.
    expected: Vec<String>,
}

#[tokio::test]
async fn an_attached_managed_permission_joins_the_shares_permission_set() {
    let router = emulator();
    let component_reader = mint(
        &router,
        "mint a component permission",
        "ComponentReader",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;

    let rows = [
        AttachRow {
            case: "a share that shares nothing takes a customer managed permission",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![component_reader.clone()],
            expected: vec![component_reader.clone()],
        },
        AttachRow {
            case: "a share takes an AWS managed permission that is not a type default",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
            expected: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
        },
        AttachRow {
            case: "the permission attached for a shared type stands in for that type's default",
            resource_arns: vec![GLUE_DATABASE],
            created_with: vec![],
            attached: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
            expected: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
        },
        AttachRow {
            case: "an attachment beside the derived default of another type",
            resource_arns: vec![LOCAL_RESOURCE],
            created_with: vec![],
            attached: vec![component_reader.clone()],
            expected: vec![
                managed("AWSRAMDefaultPermissionSubnet"),
                component_reader.clone(),
            ],
        },
        AttachRow {
            case: "a shared resource type the catalog publishes no permission for",
            resource_arns: vec![COMPONENT],
            created_with: vec![],
            attached: vec![component_reader.clone()],
            expected: vec![component_reader.clone()],
        },
        AttachRow {
            case: "two permissions of different resource types are both carried",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![
                managed("AWSRAMPermissionGlueDatabaseReadWrite"),
                managed("AWSRAMDefaultPermissionPrefixList"),
            ],
            expected: vec![
                managed("AWSRAMDefaultPermissionPrefixList"),
                managed("AWSRAMPermissionGlueDatabaseReadWrite"),
            ],
        },
        AttachRow {
            case: "the same permission attached twice is carried once",
            resource_arns: vec![],
            created_with: vec![],
            attached: vec![component_reader.clone(), component_reader.clone()],
            expected: vec![component_reader.clone()],
        },
        AttachRow {
            case: "an attachment beside a permission the create named",
            resource_arns: vec![],
            created_with: vec![managed("AWSRAMDefaultPermissionSubnet")],
            attached: vec![component_reader.clone()],
            expected: vec![
                managed("AWSRAMDefaultPermissionSubnet"),
                component_reader.clone(),
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let AttachRow {
            case,
            resource_arns,
            created_with,
            attached,
            expected,
        } = row;
        let share_arn = share_with(
            &router,
            case,
            &json!({
                "name": format!("attach-row-{index}"),
                "resourceArns": resource_arns,
                "permissionArns": created_with,
            }),
        )
        .await;
        // Before the attach the share carries only what it was created with and what it
        // derives, so the attach is what the row observes.
        let before = listed_arns(&router, case, &share_arn).await;
        for permission_arn in attached {
            attach_ok(&router, case, &share_arn, permission_arn).await;
        }
        // The permissions are reported in ARN order, so the expectation is the row's
        // set in that order.
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            listed_arns(&router, case, &share_arn).await,
            wanted,
            "case {case:?}: attaching {attached:?} to a share carrying {before:?} must leave \
             exactly {wanted:?}"
        );
    }
}

#[tokio::test]
async fn an_attached_permission_reads_back_as_the_permission_that_was_attached() {
    let router = emulator();
    let case = "a customer managed permission attached to a share";
    let permission_arn = mint(
        &router,
        case,
        "ComponentConsumer",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "component-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        Vec::<String>::new(),
        "case {case:?}: the share carries nothing before the attach"
    );

    attach_ok(&router, case, &share_arn, &permission_arn).await;

    let reported = listed(&router, case, &share_arn).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the share carries the one permission attached, got {reported:?}"
    );
    let permission = &reported[0];
    assert_eq!(
        field(case, permission, "arn"),
        permission_arn,
        "case {case:?}: expected the attached permission's ARN, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "name"),
        "ComponentConsumer",
        "case {case:?}: expected the name the create stored, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "resourceType"),
        COMPONENT_TYPE,
        "case {case:?}: expected the resource type the create named, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "permissionType"),
        PERMISSION_TYPE_CUSTOMER_MANAGED,
        "case {case:?}: a permission a caller created reports CUSTOMER_MANAGED, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "featureSet"),
        PERMISSION_FEATURE_SET_STANDARD,
        "case {case:?}: expected the STANDARD feature set, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "version"),
        FIRST_PERMISSION_VERSION.to_string(),
        "case {case:?}: the version attached is the permission's default version, got \
         {permission:?}"
    );
    assert!(
        flag(case, permission, "defaultVersion"),
        "case {case:?}: the attached version is the permission's default version, got \
         {permission:?}"
    );
    assert!(
        !flag(case, permission, "isResourceTypeDefault"),
        "case {case:?}: a created permission is not its resource type's default, got \
         {permission:?}"
    );
    assert!(
        permission.get("lastUpdatedTime").is_some_and(Value::is_number),
        "case {case:?}: a reported permission carries a lastUpdatedTime, got {permission:?}"
    );
    assert_eq!(
        permission.get("tags").and_then(Value::as_array),
        Some(&vec![]),
        "case {case:?}: a permission carrying no tags reports an empty list, got {permission:?}"
    );

    // The read changed nothing a later caller sees.
    let (first_status, first) =
        list_permissions(&router, &json!({"resourceShareArn": share_arn.clone()})).await;
    let (second_status, second) =
        list_permissions(&router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "case {case:?}: both reads expected 200, got {first_status} then {second_status}"
    );
    assert_eq!(
        first, second,
        "case {case:?}: a read reported something else the second time, got {second:?}"
    );
}

#[tokio::test]
async fn a_second_permission_for_one_resource_type_needs_the_replace_parameter() {
    let router = emulator();
    let case = "one permission per resource type";
    let first = mint(
        &router,
        case,
        "ComponentReaderOne",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let second = mint(
        &router,
        case,
        "ComponentReaderTwo",
        COMPONENT_TYPE,
        "imagebuilder:ListComponents",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "one-per-type", "resourceArns": [COMPONENT]}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &first).await;

    let (status, refused) = attach(
        &router,
        &json!({"resourceShareArn": share_arn, "permissionArn": second}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a second permission of one type, got {status} with \
         {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "Resource share {share_arn} already has permission {first} associated for resource \
             type {COMPONENT_TYPE}. Set replace to true to replace it."
        ),
        "case {case:?}: the refusal names the type, the permission and the parameter, got \
         {refused:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![first.clone()],
        "case {case:?}: a refused attach leaves the share's permission set alone"
    );

    // The same request with `replace` set does what it was refused for: the share ends
    // up governed by the second permission and no longer by the first.
    let case = "replace the permission of a resource type";
    let (status, replaced) = attach(
        &router,
        &json!({"resourceShareArn": share_arn, "permissionArn": second, "replace": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {replaced:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![second.clone()],
        "case {case:?}: a replacing attach leaves only the permission it attached"
    );

    // A permission of ANOTHER resource type collides with nothing, so it needs no
    // replace of its own.
    let case = "a permission of another resource type";
    attach_ok(
        &router,
        case,
        &share_arn,
        &managed("AWSRAMPermissionGlueDatabaseReadWrite"),
    )
    .await;
    let mut wanted = vec![
        second,
        managed("AWSRAMPermissionGlueDatabaseReadWrite"),
    ];
    wanted.sort();
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        wanted,
        "case {case:?}: the share carries one permission per resource type"
    );
}

#[tokio::test]
async fn an_attach_that_cannot_be_made_is_refused_and_attaches_nothing() {
    let router = emulator();
    let case = "a share to attach to";
    let permission_arn = mint(
        &router,
        case,
        "ComponentRefusals",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "refusal-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    let doomed = share_with(
        &router,
        case,
        &json!({"name": "deleted-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    let (status, removed) = delete(&router, &doomed, None).await;
    assert_eq!(
        status, 200,
        "case \"delete a share\": expected 200, got {status} with {removed:?}"
    );
    let unknown_share =
        format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/11111111-2222-3333-4444-555555555555");
    let unknown_permission = customer("NeverCreated");

    // Each row: the request, the modelled error class it must be refused with, and the
    // message that class carries where this service words one.
    let rows: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a resourceShareArn that is not an ARN",
            json!({"resourceShareArn": "resource-share/abc", "permissionArn": permission_arn}),
            "MalformedArnException",
            None,
        ),
        (
            "a resourceShareArn naming another service",
            json!({
                "resourceShareArn": format!("arn:aws:ec2:{REGION}:{ACCOUNT}:resource-share/abc"),
                "permissionArn": permission_arn,
            }),
            "MalformedArnException",
            None,
        ),
        (
            "a permissionArn that names a share",
            json!({"resourceShareArn": share_arn, "permissionArn": share_arn}),
            "MalformedArnException",
            None,
        ),
        (
            "a share nothing was created under",
            json!({"resourceShareArn": unknown_share, "permissionArn": permission_arn}),
            "UnknownResourceException",
            Some(format!("ResourceShare {unknown_share} could not be found.")),
        ),
        (
            "a share that was deleted",
            json!({"resourceShareArn": doomed, "permissionArn": permission_arn}),
            "UnknownResourceException",
            Some(format!("ResourceShare {doomed} could not be found.")),
        ),
        (
            "a permission nothing was created under",
            json!({"resourceShareArn": share_arn, "permissionArn": unknown_permission}),
            "UnknownResourceException",
            Some(format!(
                "Permission {unknown_permission} could not be found."
            )),
        ),
        (
            "a permission version below the first one",
            json!({
                "resourceShareArn": share_arn,
                "permissionArn": permission_arn,
                "permissionVersion": 0,
            }),
            "InvalidParameterException",
            Some(format!(
                "permissionVersion must be {FIRST_PERMISSION_VERSION} or greater"
            )),
        ),
        (
            "a permission version the permission does not carry",
            json!({
                "resourceShareArn": share_arn,
                "permissionArn": permission_arn,
                "permissionVersion": FIRST_PERMISSION_VERSION + 1,
            }),
            "UnknownResourceException",
            Some(format!(
                "Permission {permission_arn} has no version {}.",
                FIRST_PERMISSION_VERSION + 1
            )),
        ),
        (
            "an empty clientToken",
            json!({
                "resourceShareArn": share_arn,
                "permissionArn": permission_arn,
                "clientToken": "",
            }),
            "InvalidClientTokenException",
            Some("clientToken must not be empty".to_string()),
        ),
    ];

    for (case, request, expected_code, expected_message) in rows {
        let (status, refused) = attach(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );
        if let Some(message) = expected_message {
            assert_eq!(
                error_message(&refused),
                message,
                "case {case:?}: wrong refusal message, got {refused:?}"
            );
        }
    }

    // Not one of those requests attached anything: the share still carries nothing.
    assert_eq!(
        listed_arns(&router, "refused attaches change nothing", &share_arn).await,
        Vec::<String>::new(),
        "case \"refused attaches change nothing\": the share's permission set is untouched"
    );

    // A request naming the permission's own default version is not a refusal at all:
    // that is the version RAM attaches.
    let case = "the default version named explicitly";
    let (status, answered) = attach(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": permission_arn,
            "permissionVersion": FIRST_PERMISSION_VERSION,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {answered:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![permission_arn],
        "case {case:?}: the share carries the permission that was attached"
    );
}

#[tokio::test]
async fn one_client_token_attaches_once_and_refuses_another_request() {
    let router = emulator();
    let case = "an attach repeated with one client token";
    let first = mint(
        &router,
        case,
        "ComponentIdempotent",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let other = managed("AWSRAMPermissionGlueDatabaseReadWrite");
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "idempotent-share", "resourceArns": [COMPONENT]}),
    )
    .await;

    let request = json!({
        "resourceShareArn": share_arn,
        "permissionArn": first,
        "clientToken": "attach-token-1",
    });
    for attempt in 1..=2 {
        let (status, answered) = attach(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: attempt {attempt} expected HTTP 200, got {status} with {answered:?}"
        );
        assert_eq!(
            answered.get("clientToken").and_then(Value::as_str),
            Some("attach-token-1"),
            "case {case:?}: attempt {attempt} must report the token it was given, got {answered:?}"
        );
        assert_eq!(
            answered.get("returnValue").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: attempt {attempt} expected returnValue true, got {answered:?}"
        );
    }
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![first.clone()],
        "case {case:?}: a repeated attach attaches the permission once"
    );

    let case = "one token reused for another request";
    let (status, refused) = attach(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": other,
            "clientToken": "attach-token-1",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidClientTokenException",
        "case {case:?}: expected InvalidClientTokenException, got {refused:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![first],
        "case {case:?}: the refused request attached nothing"
    );

    // A token answers for the attachment the first call made, so it cannot answer for a
    // share that no longer exists: the repeat is refused like any other call naming it.
    let case = "the same token after the share is deleted";
    let (status, removed) = delete(&router, &share_arn, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the delete, got {status} with {removed:?}"
    );
    let (status, refused) = attach(&router, &request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "UnknownResourceException",
        "case {case:?}: expected UnknownResourceException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!("ResourceShare {share_arn} could not be found."),
        "case {case:?}: the refusal names the share it was asked for, got {refused:?}"
    );
}

#[tokio::test]
async fn a_page_holds_at_most_max_results_of_the_attached_permissions() {
    let router = emulator();
    let case = "page the attached permissions";
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "paged-attachments", "resourceArns": [COMPONENT]}),
    )
    .await;
    // Four permissions of four resource types, so one share carries four attachments:
    // enough for a page size below, at and above the number attached.
    let component_reader = mint(
        &router,
        case,
        "ComponentPaged",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let mut every = vec![
        component_reader,
        managed("AWSRAMPermissionGlueDatabaseReadWrite"),
        managed("AWSRAMDefaultPermissionPrefixList"),
        managed("AWSRAMDefaultPermissionSubnet"),
    ];
    for permission_arn in &every {
        attach_ok(&router, case, &share_arn, permission_arn).await;
    }
    every.sort();
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        every,
        "case {case:?}: the share reports every permission attached, in ARN order"
    );

    for page_size in [1usize, 2, 3, 4, 5, 500] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // A page of this size cannot need more than one request per permission plus the
        // last, empty one; anything beyond that is a token that does not advance.
        for _ in 0..=every.len() {
            let mut request = json!({"resourceShareArn": share_arn, "maxResults": page_size});
            if let Some(token) = token.clone() {
                request["nextToken"] = Value::String(token);
            }
            let (status, page) = list_permissions(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page:?}"
            );
            let reported: Vec<String> = permissions_of(&case, &page)
                .iter()
                .map(|permission| field(&case, permission, "arn"))
                .collect();
            assert!(
                reported.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} permissions, got {reported:?}"
            );
            walked.extend(reported);
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: the walk must end with a page carrying no nextToken"
        );
        assert_eq!(
            walked, every,
            "case {case:?}: paging reports every attached permission exactly once, in ARN order"
        );
    }
}

/// The detail a permission write or read reported.
fn detail_of(case: &str, body: &Value) -> Value {
    body.get("permission")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permission, got {body:?}"))
}

#[tokio::test]
async fn a_create_naming_more_permissions_than_one_call_accepts_is_refused() {
    let router = emulator();
    // Every permission ARN a create names becomes an attachment on the stored share, so
    // the list is bounded: at the bound the create is served, one past it refused.
    for (case, count, expected_status) in [
        ("at the bound", MAX_ENTITIES_PER_CALL, 200),
        ("one past the bound", MAX_ENTITIES_PER_CALL + 1, 400),
    ] {
        let permission_arns: Vec<String> = (0..count)
            .map(|index| customer(&format!("NeverCreated{index}")))
            .collect();
        let (status, answered) = create(
            &router,
            &json!({
                "name": format!("permission-count-{count}"),
                "permissionArns": permission_arns,
            }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: a create naming {count} permissions expected HTTP \
             {expected_status}, got {status} with {answered:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&answered),
                "ResourceShareLimitExceededException",
                "case {case:?}: expected ResourceShareLimitExceededException, got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_share_reports_the_version_it_attached_while_the_permissions_default_moves_on() {
    let router = emulator();
    let case = "the version a share is governed by";
    let permission_arn = mint(
        &router,
        case,
        "ComponentVersioned",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "versioned-share", "resourceArns": [COMPONENT]}),
    )
    .await;
    attach_ok(&router, case, &share_arn, &permission_arn).await;

    let attached = listed(&router, case, &share_arn).await;
    assert_eq!(
        (
            field(case, &attached[0], "version"),
            flag(case, &attached[0], "defaultVersion")
        ),
        (FIRST_PERMISSION_VERSION.to_string(), true),
        "case {case:?}: the share attached the permission's only version, which is its \
         default, got {attached:?}"
    );

    // A new version of the permission "is automatically set as the default version",
    // and "existing resource shares continue to use their original permission
    // versions" (CreatePermissionVersion's own documentation).
    let case = "a second version of the permission";
    let second_template = policy_template("imagebuilder:ListComponents");
    let (status, written) = create_permission_version(
        &router,
        &json!({"permissionArn": permission_arn, "policyTemplate": second_template}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {written:?}"
    );
    let detail = detail_of(case, &written);
    assert_eq!(
        (
            field(case, &detail, "version"),
            flag(case, &detail, "defaultVersion"),
            field(case, &detail, "permission"),
        ),
        (
            (FIRST_PERMISSION_VERSION + 1).to_string(),
            true,
            second_template.clone()
        ),
        "case {case:?}: the write reports the version it stored, as the new default, got \
         {detail:?}"
    );

    // Read back through GetPermission: a read that names no version reports the new
    // default version and the policy that version was written with.
    let (status, read) = get_permission(&router, &json!({"permissionArn": permission_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the read-back, got {status} with {read:?}"
    );
    let read_detail = detail_of(case, &read);
    assert_eq!(
        (
            field(case, &read_detail, "version"),
            field(case, &read_detail, "permission"),
        ),
        ((FIRST_PERMISSION_VERSION + 1).to_string(), second_template),
        "case {case:?}: the permission reads back at its new default version, got \
         {read_detail:?}"
    );

    // The share is still governed by the version it attached, which is no longer the
    // permission's default.
    let outdated = listed(&router, case, &share_arn).await;
    assert_eq!(
        (
            field(case, &outdated[0], "version"),
            flag(case, &outdated[0], "defaultVersion")
        ),
        (FIRST_PERMISSION_VERSION.to_string(), false),
        "case {case:?}: the share keeps the version it attached, and reports that it is no \
         longer the default, got {outdated:?}"
    );

    // A replacing attach "updates an outdated version of the permission to the current
    // default version" (AssociateResourceSharePermission$permissionVersion).
    let case = "an outdated version named without replace";
    let (status, refused) = attach(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": permission_arn,
            "permissionVersion": FIRST_PERMISSION_VERSION,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParameterException",
        "case {case:?}: expected InvalidParameterException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "permissionVersion {FIRST_PERMISSION_VERSION} is not the default version of \
             permission {permission_arn}, which is version {}. Specify the default version, or \
             set replace to true to update the resource share to it.",
            FIRST_PERMISSION_VERSION + 1
        ),
        "case {case:?}: the refusal names the version asked for, the default and the parameter, \
         got {refused:?}"
    );

    let case = "the share updated to the current default version";
    let (status, updated) = attach(
        &router,
        &json!({
            "resourceShareArn": share_arn,
            "permissionArn": permission_arn,
            "permissionVersion": FIRST_PERMISSION_VERSION + 1,
            "replace": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {updated:?}"
    );
    let current = listed(&router, case, &share_arn).await;
    assert_eq!(
        (
            field(case, &current[0], "version"),
            flag(case, &current[0], "defaultVersion")
        ),
        ((FIRST_PERMISSION_VERSION + 1).to_string(), true),
        "case {case:?}: the share now reports the version it was updated to, got {current:?}"
    );
}

#[tokio::test]
async fn a_permission_version_that_cannot_be_written_is_refused() {
    let router = emulator();
    let case = "a permission to version";
    let permission_arn = mint(
        &router,
        case,
        "ComponentVersionRefusals",
        COMPONENT_TYPE,
        "imagebuilder:GetComponent",
    )
    .await;
    let template = policy_template("imagebuilder:ListComponents");
    let unknown = customer("NeverCreated");
    let aws_managed = managed("AWSRAMDefaultPermissionSubnet");

    let rows: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a permissionArn that is not an ARN",
            json!({"permissionArn": "permission/abc", "policyTemplate": template}),
            "MalformedArnException",
            None,
        ),
        (
            "a permissionArn that names a resource share",
            json!({
                "permissionArn":
                    format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/abc"),
                "policyTemplate": template,
            }),
            "MalformedArnException",
            None,
        ),
        (
            "a permission nothing was created under",
            json!({"permissionArn": unknown, "policyTemplate": template}),
            "UnknownResourceException",
            Some(format!("Permission {unknown} could not be found.")),
        ),
        (
            "a permission Amazon Web Services manages",
            json!({"permissionArn": aws_managed, "policyTemplate": template}),
            "InvalidParameterException",
            Some(format!(
                "permissionArn '{aws_managed}' names a permission that Amazon Web Services \
                 manages, so no new version of it can be created"
            )),
        ),
        (
            "a policyTemplate that is not JSON",
            json!({"permissionArn": permission_arn, "policyTemplate": "not json"}),
            "MalformedPolicyTemplateException",
            None,
        ),
        (
            "a policyTemplate whose action belongs to another service",
            json!({
                "permissionArn": permission_arn,
                "policyTemplate": policy_template("ec2:DescribeSubnets"),
            }),
            "InvalidPolicyException",
            None,
        ),
        (
            "a policyTemplate that does not allow",
            json!({
                "permissionArn": permission_arn,
                "policyTemplate":
                    json!({"Effect": "DENY", "Action": ["imagebuilder:GetComponent"]}).to_string(),
            }),
            "InvalidPolicyException",
            None,
        ),
        (
            "an empty clientToken",
            json!({
                "permissionArn": permission_arn,
                "policyTemplate": template,
                "clientToken": "",
            }),
            "InvalidClientTokenException",
            Some("clientToken must not be empty".to_string()),
        ),
    ];

    for (case, request, expected_code, expected_message) in rows {
        let (status, refused) = create_permission_version(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused:?}"
        );
        if let Some(message) = expected_message {
            assert_eq!(
                error_message(&refused),
                message,
                "case {case:?}: wrong refusal message, got {refused:?}"
            );
        }
    }

    // Not one of those requests wrote a version: the permission still has only the one
    // it was created with.
    let (status, read) = get_permission(&router, &json!({"permissionArn": permission_arn})).await;
    assert_eq!(
        (status, field("refused writes change nothing", &detail_of("refused writes change nothing", &read), "version")),
        (200, FIRST_PERMISSION_VERSION.to_string()),
        "case \"refused writes change nothing\": the permission is still at its first version, \
         got {read:?}"
    );

    // The published cap: a permission carries at most MAX_PERMISSION_VERSIONS versions,
    // and the write past it is refused rather than stored.
    for version in (FIRST_PERMISSION_VERSION + 1)..=(MAX_PERMISSION_VERSIONS as i32) {
        let case = format!("version {version}");
        let (status, written) = create_permission_version(
            &router,
            &json!({
                "permissionArn": permission_arn,
                "policyTemplate": policy_template(&format!("imagebuilder:GetComponent{version}")),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {written:?}"
        );
        assert_eq!(
            field(&case, &detail_of(&case, &written), "version"),
            version.to_string(),
            "case {case:?}: the write reports the version it stored, got {written:?}"
        );
    }
    let case = format!("version {}", MAX_PERMISSION_VERSIONS as i32 + 1);
    let (status, refused) = create_permission_version(
        &router,
        &json!({
            "permissionArn": permission_arn,
            "policyTemplate": policy_template("imagebuilder:ListComponentBuildVersions"),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 past the cap, got {status} with {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "PermissionVersionsLimitExceededException",
        "case {case:?}: expected PermissionVersionsLimitExceededException, got {refused:?}"
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "Permission {permission_arn} already has the maximum of {MAX_PERMISSION_VERSIONS} \
             versions. Delete one of the existing versions before creating a new one."
        ),
        "case {case:?}: the refusal names the permission and the cap, got {refused:?}"
    );
}
