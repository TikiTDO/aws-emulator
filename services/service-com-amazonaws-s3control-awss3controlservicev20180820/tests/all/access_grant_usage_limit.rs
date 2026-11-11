// SPDX-License-Identifier: CC0-1.0

//! The usage limit an S3 Access Grants grant READ reports, driven end to end
//! over HTTP.
//!
//! The rule under test: a `GetAccessGrant` 200 reports the grant's own usage
//! limit beside the rest of the resource — for EVERY grant, whatever the
//! location it narrows, the permission it carries, the grantee it is for and the
//! Region its instance lives in — and no modelled member of `CreateAccessGrant`
//! asks for a limit
//! (`smithy_query(query="operation", operation="CreateAccessGrant")`), so every
//! grant this service records carries the limit of a grant created without one
//! and the read reports that. A client reading the member to decide whether a
//! grant is capped therefore always finds a number rather than an absent field.
//!
//! The rows differ along what the rule generalises over: the scope of the
//! registered location, how the grant narrows it, the permission granted, and
//! the Region the instance belongs to. Reading the same grant twice is also a
//! row, because a read must not change what a later reader sees.
//!
//! The harness, the request builder and the grant-chain helpers are the sibling
//! suites': every assertion goes through the front door a client uses.

use crate::access_grants_data_plane::{create_grant, create_instance, create_location, get_grant};
use crate::access_point_lifecycle::{Emulator, Reply, ACCOUNT, REGION};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_GRANT_UNCAPPED_USAGE_LIMIT, DEFAULT_LOCATION_SCOPE, PERMISSION_READ,
    PERMISSION_READWRITE, PERMISSION_WRITE,
};

/// A second Region, so the rows span the instances a grant can belong to.
const OTHER_REGION: &str = "eu-central-1";

/// The element the limit travels in.
const USAGE_LIMIT: &str = "UsageLimit";

/// One grant to read: the location it is created on, how it narrows it, the
/// permission it carries and the Region of the instance that holds it.
struct GrantCase {
    case: &'static str,
    region: &'static str,
    location_scope: &'static str,
    sub_prefix: Option<&'static str>,
    permission: &'static str,
}

/// The rows. No row is a renaming of another: each varies the location scope,
/// the narrowing, the permission or the Region.
fn grant_cases() -> Vec<GrantCase> {
    vec![
        GrantCase {
            case: "the default location narrowed to a bucket",
            region: REGION,
            location_scope: DEFAULT_LOCATION_SCOPE,
            sub_prefix: Some("reports/*"),
            permission: PERMISSION_READ,
        },
        GrantCase {
            case: "a bucket location narrowed to a prefix",
            region: REGION,
            location_scope: "s3://finance",
            sub_prefix: Some("quarterly/*"),
            permission: PERMISSION_WRITE,
        },
        GrantCase {
            case: "a bucket location the grant does not narrow",
            region: REGION,
            location_scope: "s3://archive/",
            sub_prefix: None,
            permission: PERMISSION_READWRITE,
        },
        GrantCase {
            case: "a location narrowed to a single object key",
            region: REGION,
            location_scope: "s3://ledger",
            sub_prefix: Some("2026/january.csv"),
            permission: PERMISSION_READ,
        },
        GrantCase {
            case: "an instance in another Region",
            region: OTHER_REGION,
            location_scope: "s3://reports",
            sub_prefix: Some("eu/*"),
            permission: PERMISSION_READ,
        },
    ]
}

/// The limit a reply reports, read off the one element it travels in.
fn reported_limit(reply: &Reply) -> Option<i32> {
    reply
        .text(USAGE_LIMIT)
        .map(|text| text.parse::<i32>().unwrap_or_else(|_| panic!("<{USAGE_LIMIT}> must be a number, got {text:?} in {}", reply.body)))
}

#[tokio::test]
async fn every_grant_read_reports_the_grants_usage_limit() {
    let emulator = Emulator::start();

    for case in grant_cases() {
        // The chain a grant hangs off: the account's instance in that Region,
        // then the registered location the grant narrows.
        let instance = create_instance(&emulator, ACCOUNT, case.region).await;
        assert!(
            instance.status == 200 || instance.status == 409,
            "case {:?}: the Access Grants instance must exist first, got {} with body {}",
            case.case,
            instance.status,
            instance.body
        );
        let location = create_location(&emulator, ACCOUNT, case.region, case.location_scope).await;
        assert_eq!(
            location.status, 200,
            "case {:?}: the location must be registered first, got {} with body {}",
            case.case, location.status, location.body
        );
        let location_id = location
            .text("AccessGrantsLocationId")
            .unwrap_or_else(|| panic!("case {:?}: an AccessGrantsLocationId", case.case));

        let created = create_grant(
            &emulator,
            ACCOUNT,
            case.region,
            &location_id,
            case.sub_prefix,
            case.permission,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the grant to be created, got {} with body {}",
            case.case, created.status, created.body
        );
        let grant_id = created
            .text("AccessGrantId")
            .unwrap_or_else(|| panic!("case {:?}: an AccessGrantId", case.case));

        let read = get_grant(&emulator, ACCOUNT, case.region, &grant_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected GetAccessGrant to answer 200, got {} with body {}",
            case.case, read.status, read.body
        );
        assert_eq!(
            reported_limit(&read),
            Some(ACCESS_GRANT_UNCAPPED_USAGE_LIMIT),
            "case {:?}: expected <{USAGE_LIMIT}> to report the limit the grant was created \
             with ({ACCESS_GRANT_UNCAPPED_USAGE_LIMIT}), got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.texts(USAGE_LIMIT).len(),
            1,
            "case {:?}: expected exactly one <{USAGE_LIMIT}> element, got body {}",
            case.case,
            read.body
        );
        // The rest of the resource still round-trips beside it: the limit is an
        // addition to the report, not a replacement for it.
        assert_eq!(
            (read.text("AccessGrantId"), read.text("Permission")),
            (
                Some(grant_id.clone()),
                Some(case.permission.to_string())
            ),
            "case {:?}: expected the read to report the grant it was asked about, got body {}",
            case.case,
            read.body
        );

        // A read changes nothing a later reader sees.
        let again = get_grant(&emulator, ACCOUNT, case.region, &grant_id).await;
        assert_eq!(
            (again.status, reported_limit(&again)),
            (200, reported_limit(&read)),
            "case {:?}: expected the limit to be stable across reads, got body {}",
            case.case,
            again.body
        );
    }
}
