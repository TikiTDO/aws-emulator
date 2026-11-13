// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListResourceSharePermissions`.
//!
//! The rule the rows below pin: the permissions a resource share carries are the managed
//! permissions the share was created with, plus — for every resource type the share
//! shares that none of those applies to — the ONE permission Amazon Web Services
//! publishes as that resource type's default; a read reports each of them once, in
//! permission ARN order, as the `ResourceSharePermissionSummary` of its default version,
//! and pages them with an opaque `nextToken`.
//!
//! So the rows vary along what the rule generalises over:
//!
//! * WHICH resource types a share shares, and so which defaults it carries: every
//!   default the catalog publishes is driven, not only the one the recording named,
//!   including resource types whose service code carries a hyphen, a resource type the
//!   catalog publishes no permission for at all, and a resource ARN that names no
//!   resource type;
//! * WHETHER the share names a permission itself: none, one that applies to a type it
//!   shares (so no default is added for that type), one that applies to a type it does
//!   not share, a customer managed permission a create minted, a permission no store
//!   entry answers to, and the same permission named twice;
//! * HOW MANY permissions one page holds: a page size below, at and above the number
//!   attached, a token that walks the whole list, and a token this service never issued;
//! * WHAT a read of something that is not a readable share answers: a value that is not
//!   a share ARN, a share ARN nothing was created under, and a share that was deleted.

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    aws_managed_permission_arn, default_aws_managed_permission, resource_type_matches,
    AWS_MANAGED_PERMISSIONS, FIRST_PERMISSION_VERSION, PERMISSION_FEATURE_SET_STANDARD,
    PERMISSION_TYPE_AWS_MANAGED, PERMISSION_TYPE_CUSTOMER_MANAGED, RESOURCE_TYPES,
    SHARE_PERMISSION_STATUS_ASSOCIATED,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// A managed prefix list, the resource the recording shared.
const PREFIX_LIST: &str = "arn:aws:ec2:us-west-2:000000000000:prefix-list/pl-0a1b2c3d4e5f60718";
/// A transit gateway: a second `ec2` type, so one share can carry two defaults.
const TRANSIT_GATEWAY: &str = "arn:aws:ec2:us-west-2:000000000000:transit-gateway/tgw-0a1b2c3d4e";
/// A Glue database: the one resource type the catalog publishes TWO permissions for,
/// only one of which is its default.
const GLUE_DATABASE: &str = "arn:aws:glue:us-west-2:000000000000:database/analytics";
/// A license configuration: a resource type whose service code carries a hyphen.
const LICENSE_CONFIGURATION: &str =
    "arn:aws:license-manager:us-west-2:000000000000:license-configuration/lic-0a1b2c3d";
/// A CodeBuild project: the resource type the customer managed permission below is for.
const CODEBUILD_PROJECT: &str = "arn:aws:codebuild:us-west-2:000000000000:project/builder";
/// A customer-owned IP pool: a resource type `ListResourceTypes` reports
/// (`RESOURCE_TYPES`) that the AWS managed catalog publishes no permission for.
const COIP_POOL: &str = "arn:aws:ec2:us-west-2:000000000000:coip-pool/ipv4pool-coip-0a1b2c3d";
/// An ARN whose resource field carries no separator, so it names no resource type at
/// all (`Resource$type`).
const TYPELESS_RESOURCE: &str = "arn:aws:s3:::shared-bucket";

async fn list_permissions(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/listresourcesharepermissions", Some(body)).await
}

async fn create_permission(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createpermission", Some(body)).await
}

/// The permissions a read reported.
fn permissions_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("permissions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no permissions, got {body:?}"))
}

/// The ARNs a read reported, in the order it reported them.
fn arns_of(case: &str, body: &Value) -> Vec<String> {
    permissions_of(case, body)
        .iter()
        .map(|permission| field(case, permission, "arn"))
        .collect()
}

/// A member of a reported permission, as a string.
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

/// The `nextToken` a read reported, or `None` when it reported none.
fn next_token_of(body: &Value) -> Option<String> {
    body.get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Create a share and answer its ARN.
async fn share_with(router: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let created = create_ok(router, case, request).await;
    arn_of(case, &created)
}

/// Read every permission of `share_arn`, asserting HTTP 200, and answer the ARNs in the
/// order they were reported.
async fn listed_arns(router: &MultiServiceRouter, case: &str, share_arn: &str) -> Vec<String> {
    let (status, listed) = list_permissions(router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {listed:?}"
    );
    assert_eq!(
        next_token_of(&listed),
        None,
        "case {case:?}: a page that holds every permission carries no nextToken, got {listed:?}"
    );
    arns_of(case, &listed)
}

/// One row of the attachment table.
struct AttachCase {
    case: &'static str,
    permission_arns: Vec<String>,
    resource_arns: Vec<&'static str>,
    expected: Vec<String>,
}

/// The ARN of the AWS managed permission of this name, so a row reads as the table it is.
fn managed(name: &str) -> String {
    aws_managed_permission_arn(name)
}

#[tokio::test]
async fn a_share_reports_the_permissions_it_was_created_with_and_the_defaults_of_what_it_shares() {
    let router = emulator();

    // A customer managed permission for `codebuild:Project`, minted through the API so
    // the row below names a permission this store really holds.
    let (status, minted) = create_permission(
        &router,
        &json!({
            "name": "ProjectReaderForShares",
            "resourceType": "codebuild:Project",
            "policyTemplate": r#"{"Effect":"ALLOW","Action":["codebuild:BatchGetProjects"]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"mint a customer managed permission\": expected 200, got {status} with {minted:?}"
    );
    let customer_permission = format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/ProjectReaderForShares");

    let cases = vec![
        AttachCase {
            case: "a prefix list and no permission named",
            permission_arns: vec![],
            resource_arns: vec![PREFIX_LIST],
            expected: vec![managed("AWSRAMDefaultPermissionPrefixList")],
        },
        AttachCase {
            case: "two resource types, each bringing its own default",
            permission_arns: vec![],
            resource_arns: vec![PREFIX_LIST, TRANSIT_GATEWAY],
            expected: vec![
                managed("AWSRAMDefaultPermissionPrefixList"),
                managed("AWSRAMDefaultPermissionTransitGateway"),
            ],
        },
        AttachCase {
            case: "two resources of one type bring that default once",
            permission_arns: vec![],
            resource_arns: vec![LOCAL_RESOURCE, LOCAL_RESOURCE_TWO],
            expected: vec![managed("AWSRAMDefaultPermissionSubnet")],
        },
        AttachCase {
            case: "a resource type whose service code carries a hyphen",
            permission_arns: vec![],
            resource_arns: vec![LICENSE_CONFIGURATION],
            expected: vec![managed("AWSRAMDefaultPermissionLicenseConfiguration")],
        },
        AttachCase {
            case: "a type the catalog publishes two permissions for brings only its default",
            permission_arns: vec![],
            resource_arns: vec![GLUE_DATABASE],
            expected: vec![managed("AWSRAMDefaultPermissionGlueDatabase")],
        },
        AttachCase {
            case: "the non-default permission of a type the share shares replaces its default",
            permission_arns: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
            resource_arns: vec![GLUE_DATABASE],
            expected: vec![managed("AWSRAMPermissionGlueDatabaseReadWrite")],
        },
        AttachCase {
            case: "a permission for a type the share does not share, beside a default",
            permission_arns: vec![managed("AWSRAMDefaultPermissionSubnet")],
            resource_arns: vec![PREFIX_LIST],
            expected: vec![
                managed("AWSRAMDefaultPermissionPrefixList"),
                managed("AWSRAMDefaultPermissionSubnet"),
            ],
        },
        AttachCase {
            case: "a customer managed permission for the type the share shares",
            permission_arns: vec![customer_permission.clone()],
            resource_arns: vec![CODEBUILD_PROJECT],
            expected: vec![customer_permission.clone()],
        },
        AttachCase {
            case: "the same permission named twice is carried once",
            permission_arns: vec![
                managed("AWSRAMDefaultPermissionSubnet"),
                managed("AWSRAMDefaultPermissionSubnet"),
            ],
            resource_arns: vec![],
            expected: vec![managed("AWSRAMDefaultPermissionSubnet")],
        },
        AttachCase {
            case: "a resource type the catalog publishes no permission for",
            permission_arns: vec![],
            resource_arns: vec![COIP_POOL],
            expected: vec![],
        },
        AttachCase {
            case: "a resource ARN that names no resource type",
            permission_arns: vec![],
            resource_arns: vec![TYPELESS_RESOURCE],
            expected: vec![],
        },
        AttachCase {
            case: "a permission ARN no permission answers to",
            permission_arns: vec![managed("AWSRAMDefaultPermissionNeverPublished")],
            resource_arns: vec![],
            expected: vec![],
        },
        AttachCase {
            case: "a share that shares nothing and names nothing",
            permission_arns: vec![],
            resource_arns: vec![],
            expected: vec![],
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let AttachCase {
            case,
            permission_arns,
            resource_arns,
            expected,
        } = row;
        let share_arn = share_with(
            &router,
            case,
            &json!({
                "name": format!("permission-row-{index}"),
                "resourceArns": resource_arns,
                "permissionArns": permission_arns,
            }),
        )
        .await;
        assert_eq!(
            &listed_arns(&router, case, &share_arn).await,
            expected,
            "case {case:?}: wrong permissions for resources {resource_arns:?} and permissions \
             {permission_arns:?}"
        );
    }
}

#[tokio::test]
async fn every_default_the_catalog_publishes_attaches_to_a_share_of_its_resource_type() {
    let router = emulator();
    let defaults: Vec<&_> = AWS_MANAGED_PERMISSIONS
        .iter()
        .filter(|entry| entry.is_resource_type_default)
        .collect();
    assert!(
        defaults.len() > 1,
        "the catalog must publish more than the one default this issue cited, got {}",
        defaults.len()
    );

    for (index, entry) in defaults.iter().enumerate() {
        let case = entry.name;
        // A resource ARN of that resource type: the service code is the ARN's service
        // field and the resource code its resource field, which is how `Resource$type`
        // is read back off it.
        let (service, code) = entry
            .resource_type
            .split_once(':')
            .unwrap_or_else(|| panic!("case {case:?}: {:?} is not a resource type", entry.resource_type));
        let resource_arn = format!(
            "arn:aws:{service}:{REGION}:{ACCOUNT}:{}/shared-{index}",
            code.to_ascii_lowercase()
        );
        let share_arn = share_with(
            &router,
            case,
            &json!({
                "name": format!("catalog-default-{index}"),
                "resourceArns": [resource_arn.clone()],
            }),
        )
        .await;
        assert_eq!(
            listed_arns(&router, case, &share_arn).await,
            vec![aws_managed_permission_arn(entry.name)],
            "case {case:?}: a share of {resource_arn:?} carries that resource type's default \
             permission and nothing else"
        );
    }
}

#[tokio::test]
async fn a_reported_permission_is_the_catalog_entry_at_its_default_version() {
    let router = emulator();
    let case = "AWSRAMDefaultPermissionPrefixList";
    let entry = AWS_MANAGED_PERMISSIONS
        .iter()
        .find(|entry| entry.name == case)
        .expect("the catalog publishes the prefix list permission");
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "prefix-list-share", "resourceArns": [PREFIX_LIST]}),
    )
    .await;

    let (status, listed) = list_permissions(&router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {listed:?}"
    );
    let reported = permissions_of(case, &listed);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one permission, got {listed:?}"
    );
    let permission = &reported[0];
    assert_eq!(
        field(case, permission, "arn"),
        aws_managed_permission_arn(entry.name),
        "case {case:?}: expected the catalog entry's ARN, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "name"),
        entry.name,
        "case {case:?}: expected the catalog entry's name, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "resourceType"),
        entry.resource_type,
        "case {case:?}: expected the catalog entry's resourceType, got {permission:?}"
    );
    assert_eq!(
        field(case, permission, "permissionType"),
        PERMISSION_TYPE_AWS_MANAGED,
        "case {case:?}: a permission Amazon Web Services manages reports AWS_MANAGED, got \
         {permission:?}"
    );
    assert_eq!(
        field(case, permission, "featureSet"),
        PERMISSION_FEATURE_SET_STANDARD,
        "case {case:?}: expected the STANDARD feature set, got {permission:?}"
    );
    // Read through a SHARE, `status` reports where this permission's attachment to that
    // share has got to, not the version's own `PermissionStatus` — which is what a read
    // of the permission in its own right (`GetPermission`) reports.
    assert_eq!(
        field(case, permission, "status"),
        SHARE_PERMISSION_STATUS_ASSOCIATED,
        "case {case:?}: a permission attached to the share reports its attachment, got \
         {permission:?}"
    );
    assert_eq!(
        field(case, permission, "version"),
        FIRST_PERMISSION_VERSION.to_string(),
        "case {case:?}: expected the version the catalog ships, got {permission:?}"
    );
    assert!(
        flag(case, permission, "defaultVersion"),
        "case {case:?}: a read that names no version reports the default version, got \
         {permission:?}"
    );
    assert!(
        flag(case, permission, "isResourceTypeDefault"),
        "case {case:?}: that permission is its resource type's default, got {permission:?}"
    );
    assert_eq!(
        permission.get("tags").and_then(Value::as_array),
        Some(&vec![]),
        "case {case:?}: a permission carrying no tags reports an empty list, got {permission:?}"
    );
    // A summary is not a detail: the policy document belongs to `GetPermission`.
    assert_eq!(
        permission.get("permission"),
        None,
        "case {case:?}: a summary carries no policy document, got {permission:?}"
    );

    // The read changed nothing a later caller sees.
    let (second_status, second) =
        list_permissions(&router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        (status, second_status),
        (200, 200),
        "case {case:?}: both reads expected 200, got {status} then {second_status}"
    );
    assert_eq!(
        listed, second,
        "case {case:?}: a read reported something else the second time, got {second:?}"
    );
}

#[tokio::test]
async fn a_permission_minted_for_the_shares_resource_type_reads_back_through_the_share() {
    let router = emulator();
    let case = "customer managed permission attached to a share";
    // A create of a customer managed permission, read back through the share it is
    // attached to: what the create stored is what this read reports.
    let (status, minted) = create_permission(
        &router,
        &json!({
            "name": "GlueTableReader",
            "resourceType": "glue:Table",
            "policyTemplate": r#"{"Effect":"ALLOW","Action":["glue:GetTable"]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the create, got {status} with {minted:?}"
    );
    let permission_arn = format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/GlueTableReader");
    let share_arn = share_with(
        &router,
        case,
        &json!({
            "name": "glue-table-share",
            "resourceArns": ["arn:aws:glue:us-west-2:000000000000:table/analytics/events"],
            "permissionArns": [permission_arn.clone()],
        }),
    )
    .await;

    let (status, listed) = list_permissions(&router, &json!({"resourceShareArn": share_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {listed:?}"
    );
    let reported = permissions_of(case, &listed);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: the share carries the one permission it named, got {listed:?}"
    );
    assert_eq!(
        field(case, &reported[0], "arn"),
        permission_arn,
        "case {case:?}: expected the minted permission's ARN, got {listed:?}"
    );
    assert_eq!(
        field(case, &reported[0], "permissionType"),
        PERMISSION_TYPE_CUSTOMER_MANAGED,
        "case {case:?}: a permission a caller created reports CUSTOMER_MANAGED, got {listed:?}"
    );
    assert_eq!(
        field(case, &reported[0], "resourceType"),
        "glue:Table",
        "case {case:?}: expected the resource type the create named, got {listed:?}"
    );
    assert!(
        !flag(case, &reported[0], "isResourceTypeDefault"),
        "case {case:?}: a created permission is not its resource type's default, got {listed:?}"
    );
}

#[tokio::test]
async fn a_resource_associated_later_brings_its_default_and_one_removed_keeps_it() {
    let router = emulator();
    let case = "associate a second resource type";
    let share_arn = share_with(
        &router,
        case,
        &json!({"name": "growing-share", "resourceArns": [PREFIX_LIST]}),
    )
    .await;
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![managed("AWSRAMDefaultPermissionPrefixList")],
        "case {case:?}: the share starts with the default of the type it was created with"
    );

    let (status, associated) = associate(
        &router,
        &json!({"resourceShareArn": share_arn, "resourceArns": [GLUE_DATABASE]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the associate, got {status} with {associated:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![
            managed("AWSRAMDefaultPermissionGlueDatabase"),
            managed("AWSRAMDefaultPermissionPrefixList"),
        ],
        "case {case:?}: sharing a resource of a second type brings that type's default too"
    );

    // Removing the resource does not detach the permission: a share keeps the managed
    // permissions attached to it until the permission itself is disassociated.
    let case = "disassociate the resource again";
    let (status, removed) = disassociate(
        &router,
        &json!({"resourceShareArn": share_arn, "resourceArns": [GLUE_DATABASE]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the disassociate, got {status} with {removed:?}"
    );
    assert_eq!(
        listed_arns(&router, case, &share_arn).await,
        vec![
            managed("AWSRAMDefaultPermissionGlueDatabase"),
            managed("AWSRAMDefaultPermissionPrefixList"),
        ],
        "case {case:?}: a share keeps a permission it was given when the resource goes"
    );
}

#[tokio::test]
async fn a_page_holds_at_most_max_results_permissions_and_a_token_walks_the_rest() {
    let router = emulator();
    let case = "page a share with four permissions";
    // Four resource types, so four defaults: enough for a page size below, at and above
    // the number attached.
    let share_arn = share_with(
        &router,
        case,
        &json!({
            "name": "paged-share",
            "resourceArns": [PREFIX_LIST, TRANSIT_GATEWAY, GLUE_DATABASE, LICENSE_CONFIGURATION],
        }),
    )
    .await;
    let every = listed_arns(&router, case, &share_arn).await;
    assert_eq!(
        every.len(),
        4,
        "case {case:?}: expected the four defaults of the four types shared, got {every:?}"
    );
    let mut sorted = every.clone();
    sorted.sort();
    assert_eq!(
        every, sorted,
        "case {case:?}: permissions are reported in ARN order, got {every:?}"
    );

    for page_size in [1usize, 2, 3, 4, 5, 500] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // A page of this size cannot need more than one request per permission plus the
        // last, empty one; anything beyond that is a token that does not advance.
        for _ in 0..=every.len() {
            let mut request = json!({
                "resourceShareArn": share_arn,
                "maxResults": page_size,
            });
            if let Some(token) = token.clone() {
                request["nextToken"] = Value::String(token);
            }
            let (status, page) = list_permissions(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {page:?}"
            );
            let reported = arns_of(&case, &page);
            assert!(
                reported.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} permissions, got {reported:?}"
            );
            walked.extend(reported);
            token = next_token_of(&page);
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
            "case {case:?}: paging reports every permission exactly once, in ARN order"
        );
    }
}

#[tokio::test]
async fn a_read_of_something_that_is_not_a_readable_share_is_refused() {
    let router = emulator();
    let live = share_with(
        &router,
        "a share to delete",
        &json!({"name": "doomed-share", "resourceArns": [PREFIX_LIST]}),
    )
    .await;
    let deleted = share_with(
        &router,
        "a share to delete",
        &json!({"name": "deleted-share", "resourceArns": [PREFIX_LIST]}),
    )
    .await;
    let (status, removed) = delete(&router, &deleted, None).await;
    assert_eq!(
        status, 200,
        "case \"delete a share\": expected 200, got {status} with {removed:?}"
    );

    // A value that is not a resource share ARN is MALFORMED, whatever the store holds;
    // one that is, but names no live share, is an UNKNOWN resource.
    let unknown = format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/11111111-2222-3333-4444-555555555555");
    let malformed: Vec<(&str, String)> = vec![
        ("not an ARN at all", "resource-share/abc".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:resource-share/abc"),
        ),
        (
            "this service's other resource",
            format!("arn:aws:ram:{REGION}:{ACCOUNT}:permission/AWSRAMDefaultPermissionSubnet"),
        ),
        (
            "a share ARN with no id",
            format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/"),
        ),
    ];
    for (case, arn) in malformed {
        let (status, refused) = list_permissions(&router, &json!({"resourceShareArn": arn})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "MalformedArnException",
            "case {case:?}: expected MalformedArnException, got {refused:?}"
        );
    }

    for (case, arn) in [("a share never created", &unknown), ("a deleted share", &deleted)] {
        let (status, refused) = list_permissions(&router, &json!({"resourceShareArn": arn})).await;
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
            format!("ResourceShare {arn} could not be found."),
            "case {case:?}: the refusal names the share it was asked for, got {refused:?}"
        );
    }

    // A `nextToken` this service never issued is refused, and the live share it names
    // is left readable. One encoder serves every read that pages over an ARN; what
    // differs between them is which ARN a token may name, so a token naming the share
    // itself is not one this read issued.
    let share_token = hex::encode(&live);
    for (case, token) in [
        ("a token that is not hex", "not-a-token"),
        ("hex that decodes to no ARN", "6162636465"),
        (
            "a token naming a share rather than a permission",
            share_token.as_str(),
        ),
    ] {
        let (status, refused) = list_permissions(
            &router,
            &json!({"resourceShareArn": live, "nextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidNextTokenException",
            "case {case:?}: expected InvalidNextTokenException, got {refused:?}"
        );
    }
    assert_eq!(
        listed_arns(&router, "a refused token leaves the share readable", &live).await,
        vec![managed("AWSRAMDefaultPermissionPrefixList")],
        "case \"a refused token leaves the share readable\": the share still reports its \
         permission"
    );
}

#[tokio::test]
async fn one_resource_type_name_matches_itself_and_no_other() {
    // The invariant the lookup rests on: a resource type read off an ARN names the one
    // catalog type it is, however either spelling separates and cases the words. Driven
    // over every resource type this service shares (`RESOURCE_TYPES`, what
    // `ListResourceTypes` reports), not only the types the recording touched.
    for entry in RESOURCE_TYPES {
        let case = entry.resource_type;
        assert!(
            resource_type_matches(case, &case.to_ascii_lowercase()),
            "case {case:?}: a resource type matches its own lowercased spelling"
        );
        assert!(
            resource_type_matches(case, &case.replace('-', "")),
            "case {case:?}: a resource type matches its own unseparated spelling"
        );
        for other in RESOURCE_TYPES {
            if other.resource_type == case {
                continue;
            }
            assert!(
                !resource_type_matches(case, other.resource_type),
                "case {case:?}: must not match the different resource type {:?}",
                other.resource_type
            );
        }
    }

    // And the default lookup answers for every type the catalog publishes a default for,
    // whichever of the two spellings it is asked with.
    for entry in AWS_MANAGED_PERMISSIONS {
        let case = entry.name;
        let found = default_aws_managed_permission(entry.resource_type)
            .map(|default| default.name)
            .unwrap_or("<none>");
        let expected = AWS_MANAGED_PERMISSIONS
            .iter()
            .find(|other| {
                other.resource_type == entry.resource_type && other.is_resource_type_default
            })
            .map(|default| default.name)
            .unwrap_or("<none>");
        assert_eq!(
            found, expected,
            "case {case:?}: the default of {:?} is {expected:?}, got {found:?}",
            entry.resource_type
        );
        let spelled = entry.resource_type.to_ascii_lowercase().replace('-', "");
        let found_either_way = default_aws_managed_permission(&spelled)
            .map(|default| default.name)
            .unwrap_or("<none>");
        assert_eq!(
            found_either_way, expected,
            "case {case:?}: the spelling {spelled:?} finds the same default, got \
             {found_either_way:?}"
        );
    }
}
