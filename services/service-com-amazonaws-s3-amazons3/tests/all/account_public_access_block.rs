// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-level Block Public Access configuration, as every guardrail of this service
//! has to read it.
//!
//! The rule under test: a Block Public Access setting is in force on a bucket when it is on
//! in the BUCKET's own configuration or in the one the owning ACCOUNT carries — "when
//! Amazon S3 receives a request to access a bucket or an object, it determines whether the
//! account or the bucket has a block public access setting applied … if there is an existing
//! block public access setting that prohibits the requested access, Amazon S3 rejects the
//! request" (Amazon S3 User Guide, *Blocking public access to your Amazon S3 storage*). The
//! most restrictive of the configurations that apply governs, so a bucket carrying NO
//! configuration of its own is still governed by its account's, and an account-level setting
//! cannot be turned off at the bucket level.
//!
//! The table below spans the class in the dimension the rule generalises over: WHICH of the
//! four settings is on at the account level, and therefore which operation the guardrail
//! answers — `BlockPublicPolicy` a public bucket policy, `BlockPublicAcls` a public ACL
//! (written on the bucket, on an object, and with the object), `IgnorePublicAcls` which
//! grants an ACL read reports, and `RestrictPublicBuckets` an unsigned read a public policy
//! would otherwise serve. Each row is run twice: with ONLY that setting on at the account
//! level, where the guardrail must answer, and with every OTHER setting on, where it must
//! not — so a row cannot pass by treating "an account configuration exists" as the block.
//!
//! The account-level configuration is written by S3 Control's own `PutPublicAccessBlock`,
//! which this crate's tests cannot call; it is seeded here in the store this service reads
//! it from, which is the store that service writes it in. The suite that drives the whole
//! path over the wire lives beside that service
//! (`account_public_access_block_bucket_policy`).

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::state::{
    account_public_access_block::MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS, service_state,
    BucketPublicAccessBlock,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, put_bucket_policy, router_with_engine, send, Answer};
use super::bucket_ownership_controls::put_ownership_controls;
use super::bucket_public_access_block::allow_public_access;
use super::object_acl::{get_object_acl, put_object_acl};
use super::object_and_tagging_roundtrip::put_object;

/// The bytes every object in this suite holds: a guardrail is about who may reach an object,
/// not about what is in it.
const BODY: &[u8] = b"account-block-subject";

/// The key every row writes.
const KEY: &str = "report.csv";

/// The predefined group URI that names everyone, so a grant to it is a PUBLIC grant (Amazon
/// S3 User Guide, *The Meaning of "Public"*).
const ALL_USERS: &str = "http://acs.amazonaws.com/groups/global/AllUsers";

/// The canned ACL that grants `READ` to [`ALL_USERS`].
const PUBLIC_READ: &str = "public-read";

/// The code S3 answers a call its Block Public Access configuration denies with, and its
/// status.
const BLOCKED_CODE: &str = "AccessDenied";
/// See [`BLOCKED_CODE`].
const BLOCKED_STATUS: u16 = 403;

/// The four settings, in the order `PublicAccessBlockConfiguration` declares its members.
/// SOURCE: `smithy_query(query="shape",shape="PublicAccessBlockConfiguration")` — each
/// member targets `Setting` (boolean) with `@default: false`.
const SETTINGS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// The four settings as the configuration this service stores, in [`SETTINGS`] order.
fn configuration(flags: [bool; 4]) -> BucketPublicAccessBlock {
    BucketPublicAccessBlock {
        block_public_acls: flags[0],
        ignore_public_acls: flags[1],
        block_public_policy: flags[2],
        restrict_public_buckets: flags[3],
    }
}

/// Every setting but the one at `index`.
fn every_other_setting(index: usize) -> [bool; 4] {
    let mut flags = [true; 4];
    flags[index] = false;
    flags
}

/// Only the setting at `index`.
fn only_setting(index: usize) -> [bool; 4] {
    let mut flags = [false; 4];
    flags[index] = true;
    flags
}

/// Record `flags` as the ACCOUNT's whole configuration, in the store this service judges a
/// request by — which is the store S3 Control's `PutPublicAccessBlock` writes it in.
fn hold_account_configuration(engine: &Arc<engine::InMemoryEngine>, flags: [bool; 4]) {
    let state = service_state(&**engine).expect("the engine must hold this service's store");
    assert!(
        state
            .lock()
            .record_account_public_access_block(ACCOUNT, configuration(flags)),
        "the account configuration {flags:?} must be recorded"
    );
}

/// Remove the ACCOUNT's configuration, leaving it with none — what S3 Control's
/// `DeletePublicAccessBlock` does.
fn release_account_configuration(engine: &Arc<engine::InMemoryEngine>) {
    let state = service_state(&**engine).expect("the engine must hold this service's store");
    state.lock().forget_account_public_access_block(ACCOUNT);
}

/// The one account this emulator serves, which owns every bucket of this store.
const ACCOUNT: &str = ::emulator_server::EMULATOR_ACCOUNT_ID;

/// A bucket whose ACLs affect permissions and whose OWN Block Public Access configuration is
/// lifted altogether, holding one object — so every refusal below can only come from the
/// account-level configuration.
async fn bucket_without_its_own_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
) {
    let created = create_bucket(router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {bucket:?} must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let enabled = put_ownership_controls(router, bucket, &["ObjectWriter"]).await;
    assert_eq!(
        enabled.status, 200,
        "the bucket {bucket:?} must take the ownership setting that enables ACLs, got {} with \
         body {:?}",
        enabled.status,
        String::from_utf8_lossy(&enabled.body)
    );
    let lifted = allow_public_access(router, bucket).await;
    assert_eq!(
        lifted.status, 200,
        "the bucket {bucket:?} must carry a configuration that blocks nothing, got {} with \
         body {:?}",
        lifted.status,
        String::from_utf8_lossy(&lifted.body)
    );
    let written = put_object(router, bucket, KEY, BODY, &[]).await;
    assert_eq!(
        written.status, 200,
        "the object {KEY:?} must be written into {bucket:?} first, got {} with body {:?}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
}

/// A bucket policy whose single statement allows `s3:GetObject` on the bucket's objects to
/// every principal — a PUBLIC document, which is what `BlockPublicPolicy` refuses a write of
/// and what `RestrictPublicBuckets` stops an unsigned caller from being served by.
fn public_policy(bucket: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":\"*\",\"Action\":\"s3:GetObject\",\
         \"Resource\":\"arn:aws:s3:::{bucket}/*\"}}]}}"
    )
}

/// `PUT /<bucket>?acl` — `PutBucketAcl` with the canned ACL `canned`.
async fn put_bucket_acl(router: &MultiServiceRouter, bucket: &str, canned: &str) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?acl"),
        &[("x-amz-acl", canned)],
        "",
    )
    .await
}

/// `GET /<bucket>?acl` — `GetBucketAcl`.
async fn get_bucket_acl(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?acl"), &[], "").await
}

/// Dispatch one request carrying NO credential: no `Authorization` header and no presigned
/// query parameter, which is what an unsigned client puts on the wire.
async fn send_anonymous(router: &MultiServiceRouter, method: &str, uri: &str) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://localhost{uri}"))
        .header("host", "localhost")
        .body(::bytes::Bytes::new())
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Whether `answer` reports a grant to [`ALL_USERS`] — the public grant `IgnorePublicAcls`
/// drops from an ACL report.
fn reports_a_public_grant(answer: &Answer) -> bool {
    String::from_utf8_lossy(&answer.body).contains(ALL_USERS)
}

#[tokio::test]
async fn an_account_level_block_public_policy_refuses_a_public_bucket_policy() {
    let index = 2;
    assert_eq!(
        SETTINGS[index], "BlockPublicPolicy",
        "the row must be about the setting it names"
    );
    // (case, the account configuration, whether the write is refused)
    let rows = [
        ("only BlockPublicPolicy on", only_setting(index), true),
        ("every setting but BlockPublicPolicy on", every_other_setting(index), false),
        ("all four settings on", [true; 4], true),
    ];
    for (case, flags, refused) in rows {
        let (router, engine) = router_with_engine();
        let bucket = "account-block-policy";
        bucket_without_its_own_configuration(&router, bucket).await;
        hold_account_configuration(&engine, flags);

        let answer = put_bucket_policy(&router, bucket, &public_policy(bucket)).await;
        if refused {
            assert_eq!(
                (answer.status, answer.error_code()),
                (BLOCKED_STATUS, BLOCKED_CODE.to_string()),
                "case {case:?}: expected the public document refused, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        } else {
            assert_eq!(
                answer.status,
                204,
                "case {case:?}: expected the public document accepted, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }

        // And the account's configuration is live: released, the write is accepted.
        release_account_configuration(&engine);
        let after = put_bucket_policy(&router, bucket, &public_policy(bucket)).await;
        assert_eq!(
            after.status, 204,
            "case {case:?}: expected the write accepted once the account configuration is \
             released, got {} with body {:?}",
            after.status,
            String::from_utf8_lossy(&after.body)
        );
    }
}

#[tokio::test]
async fn an_account_level_block_public_acls_refuses_every_public_acl_write() {
    let index = 0;
    assert_eq!(
        SETTINGS[index], "BlockPublicAcls",
        "the row must be about the setting it names"
    );
    for (case, flags, refused) in [
        ("only BlockPublicAcls on", only_setting(index), true),
        ("every setting but BlockPublicAcls on", every_other_setting(index), false),
        ("all four settings on", [true; 4], true),
    ] {
        let (router, engine) = router_with_engine();
        let bucket = "account-block-acls";
        bucket_without_its_own_configuration(&router, bucket).await;
        hold_account_configuration(&engine, flags);

        // One row per operation the setting covers: the bucket's ACL, an object's ACL, and
        // the ACL a write puts on the object it stores.
        let bucket_acl = put_bucket_acl(&router, bucket, PUBLIC_READ).await;
        let object_acl = put_object_acl(
            &router,
            bucket,
            KEY,
            "",
            &[("x-amz-acl", PUBLIC_READ)],
            "",
        )
        .await;
        let with_the_object = put_object(
            &router,
            bucket,
            "written-with-an-acl.csv",
            BODY,
            &[("x-amz-acl", PUBLIC_READ)],
        )
        .await;
        for (operation, answer) in [
            ("PutBucketAcl", bucket_acl),
            ("PutObjectAcl", object_acl),
            ("PutObject carrying the ACL", with_the_object),
        ] {
            match refused {
                true => assert_eq!(
                    (answer.status, answer.error_code()),
                    (BLOCKED_STATUS, BLOCKED_CODE.to_string()),
                    "case {case:?} on {operation}: expected the public ACL refused, got {} with \
                     body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                ),
                false => assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} on {operation}: expected the public ACL accepted, got {} with \
                     body {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                ),
            }
        }
    }
}

#[tokio::test]
async fn an_account_level_ignore_public_acls_drops_the_public_grant_from_every_acl_read() {
    let index = 1;
    assert_eq!(
        SETTINGS[index], "IgnorePublicAcls",
        "the row must be about the setting it names"
    );
    for (case, flags, ignored) in [
        ("only IgnorePublicAcls on", only_setting(index), true),
        ("every setting but IgnorePublicAcls on", every_other_setting(index), false),
        ("all four settings on", [true; 4], true),
    ] {
        let (router, engine) = router_with_engine();
        let bucket = "account-ignore-acls";
        bucket_without_its_own_configuration(&router, bucket).await;
        // The public grants are STORED while nothing blocks them, so what the reads below
        // report can only be the effect of the setting.
        assert_eq!(
            put_bucket_acl(&router, bucket, PUBLIC_READ).await.status,
            200,
            "case {case:?}: the public bucket ACL must be stored first"
        );
        assert_eq!(
            put_object_acl(&router, bucket, KEY, "", &[("x-amz-acl", PUBLIC_READ)], "")
                .await
                .status,
            200,
            "case {case:?}: the public object ACL must be stored first"
        );
        hold_account_configuration(&engine, flags);

        for (operation, answer) in [
            ("GetBucketAcl", get_bucket_acl(&router, bucket).await),
            (
                "GetObjectAcl",
                get_object_acl(&router, bucket, KEY, "", &[]).await,
            ),
        ] {
            assert_eq!(
                answer.status, 200,
                "case {case:?} on {operation}: the ACL must still be reported, got {} with body \
                 {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                reports_a_public_grant(&answer),
                !ignored,
                "case {case:?} on {operation}: expected the public grant {}, got body {:?}",
                match ignored {
                    true => "dropped from the report",
                    false => "reported",
                },
                String::from_utf8_lossy(&answer.body)
            );
        }

        // The grants are not taken away: released, the configuration reports them again.
        release_account_configuration(&engine);
        assert!(
            reports_a_public_grant(&get_bucket_acl(&router, bucket).await),
            "case {case:?}: expected the stored public grant reported again once the account \
             configuration is released"
        );
    }
}

#[tokio::test]
async fn an_account_level_restrict_public_buckets_refuses_the_unsigned_read_a_public_policy_allows()
{
    let index = 3;
    assert_eq!(
        SETTINGS[index], "RestrictPublicBuckets",
        "the row must be about the setting it names"
    );
    for (case, flags, restricted) in [
        ("only RestrictPublicBuckets on", only_setting(index), true),
        (
            "every setting but RestrictPublicBuckets on",
            every_other_setting(index),
            false,
        ),
        ("all four settings on", [true; 4], true),
    ] {
        let (router, engine) = router_with_engine();
        let bucket = "account-restrict-buckets";
        bucket_without_its_own_configuration(&router, bucket).await;
        // The public policy is stored while nothing blocks it, so what the read below
        // answers can only be the effect of the setting.
        assert_eq!(
            put_bucket_policy(&router, bucket, &public_policy(bucket))
                .await
                .status,
            204,
            "case {case:?}: the public policy must be stored first"
        );
        hold_account_configuration(&engine, flags);

        for (operation, answer) in [
            (
                "GetObject",
                send_anonymous(&router, "GET", &format!("/{bucket}/{KEY}")).await,
            ),
            (
                "HeadObject",
                send_anonymous(&router, "HEAD", &format!("/{bucket}/{KEY}")).await,
            ),
        ] {
            let expected = match restricted {
                true => BLOCKED_STATUS,
                false => 200,
            };
            assert_eq!(
                answer.status, expected,
                "case {case:?} on {operation}: expected {expected}, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }

        // Released, the public policy serves the unsigned read again.
        release_account_configuration(&engine);
        assert_eq!(
            send_anonymous(&router, "GET", &format!("/{bucket}/{KEY}"))
                .await
                .status,
            200,
            "case {case:?}: expected the unsigned read served again once the account \
             configuration is released"
        );
    }
}

#[tokio::test]
async fn the_store_holds_a_bounded_number_of_accounts_configurations() {
    let (_router, engine) = router_with_engine();
    let state = service_state(&*engine).expect("the engine must hold this service's store");
    let mut store = state.lock();
    // A 12-digit account id, as `AccountId`'s `^\d{12}$` requires, distinct per index.
    let account = |index: usize| format!("{:012}", 100_000_000_000u64 + index as u64);
    for index in 0..MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS {
        assert!(
            store.record_account_public_access_block(&account(index), configuration([true; 4])),
            "case account {index}: expected it recorded under the ceiling"
        );
    }
    assert!(
        !store.record_account_public_access_block(
            &account(MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS),
            configuration([true; 4])
        ),
        "case one past the ceiling: expected the record refused, got {} held",
        store.account_public_access_blocks()
    );
    assert_eq!(
        store.account_public_access_blocks(),
        MAX_ACCOUNT_PUBLIC_ACCESS_BLOCKS,
        "case one past the ceiling: expected the ceiling held, got {}",
        store.account_public_access_blocks()
    );
}
