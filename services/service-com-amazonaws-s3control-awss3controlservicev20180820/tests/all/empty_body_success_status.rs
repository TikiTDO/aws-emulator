// SPDX-License-Identifier: CC0-1.0

//! What status an S3 Control success carries, driven end to end over HTTP.
//!
//! The rule under test: a successful call whose modelled output is
//! `smithy.api#Unit` — the model declares no response document for it, so the
//! answer has nothing to put in a body — is answered `204 No Content` with an
//! empty body, while a call that DOES report a document is answered 200 with
//! one. The status is a property of the class of answer, not of the verb or the
//! resource: a `PUT` that reports nothing back answers 204 just as a `DELETE`
//! does, and a `DELETE` that reports a document would answer 200.
//!
//! Each row below differs along something that rule generalises over: the
//! operation, the resource family it belongs to (access points, Access Grants,
//! the account-level public-access block, Storage Lens), the verb, the bytes
//! and size of the document the call carries in, whether the resource the call
//! is about was there to begin with (so a repeated call is a row of its own),
//! and the account and Region the request names. The recorded wire answer the
//! issue was filed from is one of these rows.
//!
//! The harness, the request builder and the XML readers are the sibling suites':
//! every call goes through the multi-service router, so a passing row is a claim
//! about the wire.

use crate::access_grants_data_plane::{create_grant, create_instance, create_location, get_grant};
use crate::access_grants_location_delete::{delete_grant, delete_location};
use crate::access_point_lifecycle::{
    call_in_region, xml_escape, Emulator, Reply, ACCOUNT, NO_CONTENT, REGION, XMLNS,
};
use crate::storage_lens_configuration::{documents, get_configuration, put_configuration};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::PERMISSION_READ;

/// A second signing Region, so a row can show the status is not a property of
/// the Region the request is signed for.
const OTHER_REGION: &str = "eu-central-1";

/// The status a success that reports a document carries — the Smithy default,
/// and what every reader of this service answers.
const WITH_DOCUMENT: u16 = 200;

// ---------------------------------------------------------------------------
// The calls this suite drives, written here because they are the operations
// under test rather than another suite's setup.
// ---------------------------------------------------------------------------

async fn create_access_point(emulator: &Emulator, account_id: &str, name: &str) -> Reply {
    // An access point is created only over a bucket that exists.
    emulator.provision_bucket(name, "reports");
    let body = format!(
        "<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>reports</Bucket>\
         </CreateAccessPointRequest>"
    );
    emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{name}"),
            account_id,
            &body,
        )
        .await
}

async fn put_access_point_policy(
    emulator: &Emulator,
    account_id: &str,
    name: &str,
    policy: &str,
) -> Reply {
    let body = format!(
        "<PutAccessPointPolicyRequest xmlns=\"{XMLNS}\"><Policy>{}</Policy>\
         </PutAccessPointPolicyRequest>",
        xml_escape(policy)
    );
    emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account_id,
            &body,
        )
        .await
}

async fn get_access_point_policy(emulator: &Emulator, account_id: &str, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account_id,
            "",
        )
        .await
}

async fn delete_access_point_policy(emulator: &Emulator, account_id: &str, name: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account_id,
            "",
        )
        .await
}

async fn delete_access_point(emulator: &Emulator, account_id: &str, name: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/accesspoint/{name}"),
            account_id,
            "",
        )
        .await
}

/// The URI the account-level public-access block is bound to.
const PUBLIC_ACCESS_BLOCK_URI: &str = "/v20180820/configuration/publicAccessBlock";

async fn put_public_access_block(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    members: &str,
) -> Reply {
    let body = format!(
        "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">{members}\
         </PublicAccessBlockConfiguration>"
    );
    call_in_region(
        &emulator.router,
        region,
        "PUT",
        PUBLIC_ACCESS_BLOCK_URI,
        account_id,
        &body,
    )
    .await
}

async fn get_public_access_block(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        PUBLIC_ACCESS_BLOCK_URI,
        account_id,
        "",
    )
    .await
}

async fn delete_public_access_block(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        PUBLIC_ACCESS_BLOCK_URI,
        account_id,
        "",
    )
    .await
}

/// The URI an Access Grants instance's resource policy is bound to.
const RESOURCE_POLICY_URI: &str = "/v20180820/accessgrantsinstance/resourcepolicy";

async fn put_instance_resource_policy(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    policy: &str,
) -> Reply {
    let body = format!(
        "<PutAccessGrantsInstanceResourcePolicyRequest xmlns=\"{XMLNS}\"><Policy>{}</Policy>\
         </PutAccessGrantsInstanceResourcePolicyRequest>",
        xml_escape(policy)
    );
    call_in_region(
        &emulator.router,
        region,
        "PUT",
        RESOURCE_POLICY_URI,
        account_id,
        &body,
    )
    .await
}

async fn delete_instance_resource_policy(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        RESOURCE_POLICY_URI,
        account_id,
        "",
    )
    .await
}

async fn delete_instance(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        "/v20180820/accessgrantsinstance",
        account_id,
        "",
    )
    .await
}

async fn delete_storage_lens_configuration(
    emulator: &Emulator,
    account_id: &str,
    config_id: &str,
) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/storagelens/{config_id}"),
            account_id,
            "",
        )
        .await
}

// ---------------------------------------------------------------------------
// The assertions
// ---------------------------------------------------------------------------

/// One answer and the case it came from.
struct Row {
    case: String,
    reply: Reply,
}

fn row(case: impl Into<String>, reply: Reply) -> Row {
    Row {
        case: case.into(),
        reply,
    }
}

/// Every row reports no document, so every row must be `204` with an empty
/// body.
fn assert_no_content(rows: Vec<Row>) {
    assert!(
        !rows.is_empty(),
        "the table must drive at least one call, got none"
    );
    for Row { case, reply } in rows {
        assert_eq!(
            reply.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT}, got {} with body {:?}",
            reply.status, reply.body
        );
        assert!(
            reply.body.trim().is_empty(),
            "case {case:?}: expected no body at all with {NO_CONTENT}, got {:?}",
            reply.body
        );
    }
}

/// Policy documents an access point takes, differing in their bytes, their
/// whitespace, their alphabet and their size rather than in the resource they
/// are put on.
fn policies() -> Vec<(&'static str, String)> {
    vec![
        (
            "the shortest document this surface takes",
            r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
        ),
        (
            "indented, with newlines and tabs",
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": []\n}".to_string(),
        ),
        (
            "XML-significant characters in a value",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Sid":"a<b&c>d'e\"f"}]}"#
                .to_string(),
        ),
        (
            "non-ASCII characters in a value",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"rapport-été-☃","Effect":"Allow"}]}"#
                .to_string(),
        ),
        (
            "a document of several kilobytes",
            format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"{}","Effect":"Allow"}}]}}"#,
                "s".repeat(4096)
            ),
        ),
    ]
}

/// Access point names spanning the lengths this service addresses an access
/// point by — SOURCE: the crate's `ACCESS_POINT_NAME_LEN`, the 3..=50 the
/// service's own name rule admits — and the characters such a name can carry.
fn access_point_names() -> Vec<String> {
    let longest =
        *service_com_amazonaws_s3control_awss3controlservicev20180820::state::ACCESS_POINT_NAME_LEN
            .end();
    vec![
        "abc".to_string(),
        "reports-ap".to_string(),
        "ap-0123456789".to_string(),
        format!("long-{}", "a".repeat(longest - "long-".len())),
    ]
}

#[tokio::test]
async fn the_access_point_surface_answers_no_content_where_it_reports_no_document() {
    let emulator = Emulator::start();
    let mut rows = Vec::new();

    // PutAccessPointPolicy and DeleteAccessPointPolicy, once per (name, policy)
    // pair: the name's length and alphabet and the document's bytes are what
    // vary, and neither changes the status.
    for (index, name) in access_point_names().iter().enumerate() {
        let created = create_access_point(&emulator, ACCOUNT, name).await;
        assert_eq!(
            created.status, WITH_DOCUMENT,
            "setting up {name:?}: expected CreateAccessPoint to report a document with \
             {WITH_DOCUMENT}, got {} with body {}",
            created.status, created.body
        );

        for (case, policy) in policies() {
            rows.push(row(
                format!("PutAccessPointPolicy, {case}"),
                put_access_point_policy(&emulator, ACCOUNT, name, &policy).await,
            ));
            // The 204 is a real store, not a shallow success: the reader answers
            // the document back.
            let read = get_access_point_policy(&emulator, ACCOUNT, name).await;
            assert_eq!(
                read.text("Policy").as_deref(),
                Some(policy.as_str()),
                "case {case:?}: expected the document the 204 put stored to read back"
            );
        }

        rows.push(row(
            format!("DeleteAccessPointPolicy, a policy that is there ({name:?})"),
            delete_access_point_policy(&emulator, ACCOUNT, name).await,
        ));
        // Calling it again asks for a state the access point is already in, so
        // it succeeds the same way.
        rows.push(row(
            format!("DeleteAccessPointPolicy, no policy left ({name:?})"),
            delete_access_point_policy(&emulator, ACCOUNT, name).await,
        ));

        // Half the rows carry a policy again when the access point goes, so the
        // delete is driven both against an access point holding a policy and
        // against one holding none.
        if index % 2 == 0 {
            let restored = put_access_point_policy(
                &emulator,
                ACCOUNT,
                name,
                r#"{"Version":"2012-10-17","Statement":[]}"#,
            )
            .await;
            rows.push(row(
                format!("PutAccessPointPolicy, restoring a removed policy ({name:?})"),
                restored,
            ));
        }
        rows.push(row(
            format!("DeleteAccessPoint ({name:?})"),
            delete_access_point(&emulator, ACCOUNT, name).await,
        ));
    }

    assert_no_content(rows);

    // And the access point really is gone: the 204 reported a removal that
    // happened.
    for name in access_point_names() {
        let read = emulator
            .call(
                "GET",
                &format!("/v20180820/accesspoint/{name}"),
                ACCOUNT,
                "",
            )
            .await;
        assert_eq!(
            (read.status, read.code().as_str()),
            (404, "NoSuchAccessPoint"),
            "case {name:?}: expected the deleted access point to be gone, got {} with body {}",
            read.status,
            read.body
        );
    }
}

#[tokio::test]
async fn the_public_access_block_surface_answers_no_content_for_its_put_and_its_delete() {
    let emulator = Emulator::start();
    let mut rows = Vec::new();

    // The rows differ in which of the four settings the request carries at all,
    // in their values, and in the Region the request is signed for (the
    // configuration is account-level, so the signing Region is a dimension the
    // status must not depend on). The account is always the caller's own: a
    // request addressed in another account is refused before it is served.
    let bodies: Vec<(&str, String)> = vec![
        ("no member at all", String::new()),
        (
            "one member",
            "<BlockPublicAcls>true</BlockPublicAcls>".to_string(),
        ),
        (
            "every member, all false",
            "<BlockPublicAcls>false</BlockPublicAcls><IgnorePublicAcls>false</IgnorePublicAcls>\
             <BlockPublicPolicy>false</BlockPublicPolicy>\
             <RestrictPublicBuckets>false</RestrictPublicBuckets>"
                .to_string(),
        ),
        (
            "every member, all true",
            "<BlockPublicAcls>true</BlockPublicAcls><IgnorePublicAcls>true</IgnorePublicAcls>\
             <BlockPublicPolicy>true</BlockPublicPolicy>\
             <RestrictPublicBuckets>true</RestrictPublicBuckets>"
                .to_string(),
        ),
    ];

    for region in [REGION, OTHER_REGION] {
        for (case, members) in &bodies {
            rows.push(row(
                format!("PutPublicAccessBlock, {case}, signed for {region}"),
                put_public_access_block(&emulator, ACCOUNT, region, members).await,
            ));
            // The configuration the 204 stored is the one reported back.
            let read = get_public_access_block(&emulator, ACCOUNT, region).await;
            assert_eq!(
                read.status, WITH_DOCUMENT,
                "case {case:?}: expected the stored configuration to be reported with \
                 {WITH_DOCUMENT}, got {} with body {}",
                read.status, read.body
            );
        }

        rows.push(row(
            format!("DeletePublicAccessBlock, a configuration that is there ({region})"),
            delete_public_access_block(&emulator, ACCOUNT, region).await,
        ));
        // A repeat delete asks for the state the account is already in.
        rows.push(row(
            format!("DeletePublicAccessBlock, no configuration left ({region})"),
            delete_public_access_block(&emulator, ACCOUNT, region).await,
        ));

        let gone = get_public_access_block(&emulator, ACCOUNT, region).await;
        assert_eq!(
            (gone.status, gone.code().as_str()),
            (404, "NoSuchPublicAccessBlockConfiguration"),
            "region {region}: expected the 204 delete to have removed the configuration, \
             got {} with body {}",
            gone.status,
            gone.body
        );
    }

    assert_no_content(rows);
}

#[tokio::test]
async fn the_access_grants_surface_answers_no_content_where_it_reports_no_document() {
    let emulator = Emulator::start();
    let mut rows = Vec::new();

    // Two Regions of the caller's own account, so the status is shown not to
    // depend on which instance the request addresses. (Another account is not a
    // row: S3 Control refuses a request addressed in an account the caller holds
    // no credentials for before it reaches the resource.)
    for (account_id, region) in [(ACCOUNT, REGION), (ACCOUNT, OTHER_REGION)] {
        let instance = create_instance(&emulator, account_id, region).await;
        assert_eq!(
            instance.status, WITH_DOCUMENT,
            "setting up {account_id}/{region}: expected CreateAccessGrantsInstance to report a \
             document, got {} with body {}",
            instance.status, instance.body
        );

        // The instance resource policy: the put reports a document (200), the
        // delete reports none (204), and so does a delete of a policy that is
        // not there.
        let put = put_instance_resource_policy(
            &emulator,
            account_id,
            region,
            r#"{"Version":"2012-10-17","Statement":[]}"#,
        )
        .await;
        assert_eq!(
            put.status, WITH_DOCUMENT,
            "setting up {account_id}/{region}: expected PutAccessGrantsInstanceResourcePolicy to \
             report a document with {WITH_DOCUMENT}, got {} with body {}",
            put.status, put.body
        );
        rows.push(row(
            format!("DeleteAccessGrantsInstanceResourcePolicy, a policy that is there \
                     ({account_id}/{region})"),
            delete_instance_resource_policy(&emulator, account_id, region).await,
        ));
        rows.push(row(
            format!(
                "DeleteAccessGrantsInstanceResourcePolicy, no policy left ({account_id}/{region})"
            ),
            delete_instance_resource_policy(&emulator, account_id, region).await,
        ));

        // Locations and the grants on them: the rows differ in the scope
        // registered and in how many grants the location carries.
        let scopes = [
            ("a whole bucket", "s3://reports-bucket/", 1_usize),
            ("a prefix", "s3://finance-bucket/quarter/", 2),
        ];
        for (case, scope, grants) in scopes {
            let registered = create_location(&emulator, account_id, region, scope).await;
            assert_eq!(
                registered.status, WITH_DOCUMENT,
                "setting up {case:?}: expected CreateAccessGrantsLocation to report a document, \
                 got {} with body {}",
                registered.status, registered.body
            );
            let location_id = registered
                .text("AccessGrantsLocationId")
                .unwrap_or_default();

            let mut grant_ids = Vec::new();
            for grant in 0..grants {
                let created = create_grant(
                    &emulator,
                    account_id,
                    region,
                    &location_id,
                    Some(&format!("team-{grant}/*")),
                    PERMISSION_READ,
                )
                .await;
                assert_eq!(
                    created.status, WITH_DOCUMENT,
                    "setting up {case:?}: expected CreateAccessGrant to report a document, got \
                     {} with body {}",
                    created.status, created.body
                );
                grant_ids.push(created.text("AccessGrantId").unwrap_or_default());
            }

            for grant_id in &grant_ids {
                rows.push(row(
                    format!("DeleteAccessGrant, {case} ({account_id}/{region})"),
                    delete_grant(&emulator, account_id, region, grant_id).await,
                ));
                let gone = get_grant(&emulator, account_id, region, grant_id).await;
                assert_eq!(
                    gone.status, 404,
                    "case {case:?}: expected the 204 delete to have removed the grant, got {} \
                     with body {}",
                    gone.status, gone.body
                );
            }

            rows.push(row(
                format!("DeleteAccessGrantsLocation, {case} ({account_id}/{region})"),
                delete_location(&emulator, account_id, region, &location_id).await,
            ));
        }

        // With nothing left in it, the instance itself: another empty-body
        // success of the same family.
        rows.push(row(
            format!("DeleteAccessGrantsInstance ({account_id}/{region})"),
            delete_instance(&emulator, account_id, region).await,
        ));
    }

    assert_no_content(rows);
}

#[tokio::test]
async fn the_storage_lens_surface_answers_no_content_for_its_put_and_its_delete() {
    let emulator = Emulator::start();
    let mut rows = Vec::new();

    // The rows differ in the configuration document put — how deeply it nests
    // and which optional containers it carries — and in the id it is put under.
    for (index, document) in documents().iter().enumerate() {
        let config_id = format!("status-{index}");
        rows.push(row(
            format!("PutStorageLensConfiguration, document {index}"),
            put_configuration(&emulator, ACCOUNT, &config_id, document, None).await,
        ));
        let read = get_configuration(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            read.status, WITH_DOCUMENT,
            "document {index}: expected the configuration the 204 put stored to be reported with \
             {WITH_DOCUMENT}, got {} with body {}",
            read.status, read.body
        );

        rows.push(row(
            format!("DeleteStorageLensConfiguration, document {index}"),
            delete_storage_lens_configuration(&emulator, ACCOUNT, &config_id).await,
        ));
    }

    assert_no_content(rows);
}

#[tokio::test]
async fn a_success_that_reports_a_document_still_answers_200_with_one() {
    let emulator = Emulator::start();
    let name = "boundary-ap";

    // The contrast class: these calls DO report a document, so they keep the
    // modelled 200 and carry a body. The rows span both verbs the no-content
    // class uses (a PUT and a POST that report a document, and GETs), so the
    // status is shown to follow the answer rather than the method.
    let created = create_access_point(&emulator, ACCOUNT, name).await;
    let policy = put_access_point_policy(
        &emulator,
        ACCOUNT,
        name,
        r#"{"Version":"2012-10-17","Statement":[]}"#,
    )
    .await;
    assert_eq!(
        policy.status, NO_CONTENT,
        "setting up: expected the policy put to answer {NO_CONTENT}, got {} with body {}",
        policy.status, policy.body
    );

    let instance = create_instance(&emulator, ACCOUNT, REGION).await;
    let location = create_location(&emulator, ACCOUNT, REGION, "s3://reports-bucket/").await;
    let location_id = location
        .text("AccessGrantsLocationId")
        .unwrap_or_default();
    let grant = create_grant(
        &emulator,
        ACCOUNT,
        REGION,
        &location_id,
        Some("team-a/*"),
        PERMISSION_READ,
    )
    .await;
    let grant_id = grant.text("AccessGrantId").unwrap_or_default();
    let stored_policy = put_instance_resource_policy(
        &emulator,
        ACCOUNT,
        REGION,
        r#"{"Version":"2012-10-17","Statement":[]}"#,
    )
    .await;

    let rows = vec![
        row("CreateAccessPoint", created),
        row(
            "GetAccessPoint",
            emulator
                .call(
                    "GET",
                    &format!("/v20180820/accesspoint/{name}"),
                    ACCOUNT,
                    "",
                )
                .await,
        ),
        row(
            "GetAccessPointPolicy",
            get_access_point_policy(&emulator, ACCOUNT, name).await,
        ),
        row(
            "ListAccessPoints",
            emulator.call("GET", "/v20180820/accesspoint", ACCOUNT, "").await,
        ),
        row("CreateAccessGrantsInstance", instance),
        row("CreateAccessGrantsLocation", location),
        row("CreateAccessGrant", grant),
        row(
            "GetAccessGrant",
            get_grant(&emulator, ACCOUNT, REGION, &grant_id).await,
        ),
        row("PutAccessGrantsInstanceResourcePolicy", stored_policy),
        row(
            "GetAccessGrantsInstanceResourcePolicy",
            call_in_region(
                &emulator.router,
                REGION,
                "GET",
                RESOURCE_POLICY_URI,
                ACCOUNT,
                "",
            )
            .await,
        ),
    ];

    for Row { case, reply } in rows {
        assert_eq!(
            reply.status, WITH_DOCUMENT,
            "case {case:?}: expected {WITH_DOCUMENT}, got {} with body {:?}",
            reply.status, reply.body
        );
        assert!(
            !reply.body.trim().is_empty(),
            "case {case:?}: expected a document in the body with {WITH_DOCUMENT}, got {:?}",
            reply.body
        );
    }
}
