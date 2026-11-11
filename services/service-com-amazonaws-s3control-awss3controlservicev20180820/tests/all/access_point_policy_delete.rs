// SPDX-License-Identifier: CC0-1.0

//! Removing the policy document from an S3 Control access point, driven end to
//! end over HTTP.
//!
//! The rules under test:
//!
//! * removing an access point's policy returns that access point to carrying no
//!   policy at all — the state it was created in — so the reads that report a
//!   policy answer the missing-policy refusal afterwards while the access point
//!   resource itself, and every other access point's policy, is left as it was;
//! * the removal is idempotent: an access point that already carries no policy
//!   is in the state the call asks for, so the call succeeds;
//! * a policy can only be removed from an access point the requesting account
//!   owns — any other name is the missing-access-point refusal, which stores
//!   nothing.
//!
//! Each row differs along something those rules generalise over: the bytes of
//! the document removed (its publicness, its whitespace, its nesting, its
//! alphabet, its size), the shape of the access point it sits on (VPC-restricted
//! or not, a cross-account bucket, the shortest name the model admits), how many
//! times the removal is called, and whether the account calling owns the access
//! point.
//!
//! The harness is the sibling suite's: the same router, request builder and XML
//! readers a client's request meets.

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, xml_escape, Emulator, Reply, ACCOUNT, NO_CONTENT,
    OTHER_ACCOUNT, XMLNS,
};

/// One access point this suite removes a policy from, and the document put on
/// it.
struct Case {
    case: &'static str,
    name: &'static str,
    bucket: &'static str,
    /// `Some` makes the access point VPC-restricted.
    vpc_id: Option<&'static str>,
    /// The account owning the bucket, for a cross-account access point.
    bucket_account_id: Option<&'static str>,
    /// The document `PutAccessPointPolicy` stores before the removal.
    policy: &'static str,
}

impl Case {
    const fn new(case: &'static str, name: &'static str, policy: &'static str) -> Self {
        Self {
            case,
            name,
            bucket: "reports",
            vpc_id: None,
            bucket_account_id: None,
            policy,
        }
    }
}

/// A document that opens the access point to everyone, so a row carrying it
/// pins that publicness is no obstacle to removing it.
const PUBLIC_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"s3:GetObject","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}]}"#;

/// The same grant narrowed to one account: not public.
const ACCOUNT_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"s3:*","Resource":"arn:aws:s3:us-west-2:123456789012:accesspoint/ap/object/*"}]}"#;

/// The rows. No row is a renaming of another: each varies the document, the
/// access point's shape, or both.
fn cases() -> Vec<Case> {
    vec![
        Case::new(
            "a document granting nothing to anyone",
            "empty-policy-ap",
            r#"{"Version":"2012-10-17","Statement":[]}"#,
        ),
        Case::new("a public document", "public-policy-ap", PUBLIC_POLICY),
        Case::new(
            "a document scoped to one account",
            "scoped-policy-ap",
            ACCOUNT_POLICY,
        ),
        Case::new(
            "a document laid out with newlines and tabs",
            "indented-policy-ap",
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Deny\"\n\t\t}\n\t]\n}",
        ),
        Case::new(
            "a document carrying XML metacharacters and non-ASCII text",
            "escaped-policy-ap",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"a & b <c> \"d\" 'e' — ünïcode","Effect":"Deny"}]}"#,
        ),
        Case::new(
            // Nested through the elements the policy language defines, because a
            // member it does not define is refused whatever it nests.
            "a deeply nested document",
            "nested-policy-ap",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Condition":{"StringLike":{"s3:prefix":["a","b"]},"Bool":{"aws:SecureTransport":[false]}}}]}"#,
        ),
        Case {
            vpc_id: Some("vpc-0a1b2c3d4e5f67890"),
            ..Case::new(
                "a VPC-restricted access point",
                "vpc-policy-ap",
                ACCOUNT_POLICY,
            )
        },
        Case {
            bucket: "partner",
            bucket_account_id: Some(OTHER_ACCOUNT),
            ..Case::new(
                "an access point fronting another account's bucket",
                "partner-policy-ap",
                ACCOUNT_POLICY,
            )
        },
        // The shortest name the model's `len 3..255` admits.
        Case::new(
            "the shortest access point name the model admits",
            "abc",
            PUBLIC_POLICY,
        ),
    ]
}

/// Create an access point with every public-access setting spelled OFF, so a
/// public document is stored rather than refused and this suite stays about the
/// removal.
async fn create(emulator: &Emulator, account: &str, case: &Case) -> Reply {
    // An access point is created only over a bucket that exists.
    emulator.provision_bucket(case.case, case.bucket);
    let mut xml = format!("<CreateAccessPointRequest xmlns=\"{XMLNS}\">");
    xml.push_str(&format!("<Bucket>{}</Bucket>", xml_escape(case.bucket)));
    if let Some(vpc_id) = case.vpc_id {
        xml.push_str(&format!(
            "<VpcConfiguration><VpcId>{}</VpcId></VpcConfiguration>",
            xml_escape(vpc_id)
        ));
    }
    xml.push_str(
        "<PublicAccessBlockConfiguration><BlockPublicAcls>false</BlockPublicAcls>\
         <IgnorePublicAcls>false</IgnorePublicAcls><BlockPublicPolicy>false</BlockPublicPolicy>\
         <RestrictPublicBuckets>false</RestrictPublicBuckets>\
         </PublicAccessBlockConfiguration>",
    );
    if let Some(bucket_account_id) = case.bucket_account_id {
        xml.push_str(&format!(
            "<BucketAccountId>{}</BucketAccountId>",
            xml_escape(bucket_account_id)
        ));
    }
    xml.push_str("</CreateAccessPointRequest>");
    emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{}", case.name),
            account,
            &xml,
        )
        .await
}

async fn put_policy(emulator: &Emulator, account: &str, name: &str, document: &str) -> Reply {
    let body = format!(
        "<PutAccessPointPolicyRequest xmlns=\"{XMLNS}\"><Policy>{}</Policy>\
         </PutAccessPointPolicyRequest>",
        xml_escape(document)
    );
    emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account,
            &body,
        )
        .await
}

async fn get_policy(emulator: &Emulator, account: &str, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account,
            "",
        )
        .await
}

async fn get_policy_status(emulator: &Emulator, account: &str, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/accesspoint/{name}/policyStatus"),
            account,
            "",
        )
        .await
}

async fn delete_policy(emulator: &Emulator, account: &str, name: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/accesspoint/{name}/policy"),
            account,
            "",
        )
        .await
}

async fn get_access_point(emulator: &Emulator, account: &str, name: &str) -> Reply {
    emulator
        .call("GET", &format!("/v20180820/accesspoint/{name}"), account, "")
        .await
}

/// What a read of a policy answers for an access point carrying none.
const NO_POLICY: (u16, &str) = (404, "NoSuchAccessPointPolicy");

#[tokio::test]
async fn deleting_the_policy_leaves_the_access_point_carrying_none() {
    let emulator = Emulator::start();

    for case in cases() {
        let created = create(&emulator, ACCOUNT, &case).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateAccessPoint to answer 200, got {} with body {}",
            case.case, created.status, created.body
        );
        let arn = created.text("AccessPointArn");

        assert_eq!(
            put_policy(&emulator, ACCOUNT, case.name, case.policy)
                .await
                .status,
            NO_CONTENT,
            "case {:?}: expected the document to be stored before it is removed",
            case.case
        );
        let stored = get_policy(&emulator, ACCOUNT, case.name).await;
        assert_eq!(
            stored.text("Policy").as_deref(),
            Some(case.policy),
            "case {:?}: expected the document stored before the removal, got body {}",
            case.case,
            stored.body
        );

        let deleted = delete_policy(&emulator, ACCOUNT, case.name).await;
        assert_eq!(
            deleted.status, NO_CONTENT,
            "case {:?}: expected DeleteAccessPointPolicy to answer {NO_CONTENT}, got {} with \
             body {}",
            case.case, deleted.status, deleted.body
        );

        // The document is gone, so both reads ABOUT a policy answer the
        // missing-policy refusal rather than the access point's old document.
        let read = get_policy(&emulator, ACCOUNT, case.name).await;
        assert_eq!(
            (read.status, read.code().as_str()),
            NO_POLICY,
            "case {:?}: expected the missing-policy refusal after the removal, got {} with body {}",
            case.case,
            read.status,
            read.body
        );
        assert_eq!(
            read.text("Message").as_deref(),
            Some("The specified accesspoint policy does not exist"),
            "case {:?}: expected the service's missing-policy message, got body {}",
            case.case,
            read.body
        );
        let status = get_policy_status(&emulator, ACCOUNT, case.name).await;
        assert_eq!(
            (status.status, status.code().as_str()),
            NO_POLICY,
            "case {:?}: expected no policy status after the removal, got {} with body {}",
            case.case,
            status.status,
            status.body
        );

        // The access point resource itself is untouched: only its policy was
        // removed.
        let resource = get_access_point(&emulator, ACCOUNT, case.name).await;
        assert_eq!(
            resource.status, 200,
            "case {:?}: expected the access point to survive the removal, got {} with body {}",
            case.case, resource.status, resource.body
        );
        assert_eq!(
            resource.text("AccessPointArn"),
            arn,
            "case {:?}: expected the access point to report the same ARN, got body {}",
            case.case,
            resource.body
        );
        assert_eq!(
            resource.text("Name").as_deref(),
            Some(case.name),
            "case {:?}: expected the access point to report its name, got body {}",
            case.case,
            resource.body
        );

        // And it still takes a policy: the removal left the access point in the
        // state it was created in, not in a state that refuses a put.
        assert_eq!(
            put_policy(&emulator, ACCOUNT, case.name, ACCOUNT_POLICY)
                .await
                .status,
            NO_CONTENT,
            "case {:?}: expected a policy to be storable again after the removal",
            case.case
        );
        let again = get_policy(&emulator, ACCOUNT, case.name).await;
        assert_eq!(
            again.text("Policy").as_deref(),
            Some(ACCOUNT_POLICY),
            "case {:?}: expected the document put after the removal to be the stored one, \
             got body {}",
            case.case,
            again.body
        );
    }
}

#[tokio::test]
async fn deleting_a_policy_is_idempotent_however_many_times_it_is_called() {
    let emulator = Emulator::start();

    // How many removals in a row each row calls: none of them needs a policy to
    // be there, so every call answers the same way.
    for calls in [1_usize, 2, 3] {
        let case = Case::new(
            "repeated removal",
            match calls {
                1 => "idempotent-1-ap",
                2 => "idempotent-2-ap",
                _ => "idempotent-3-ap",
            },
            PUBLIC_POLICY,
        );
        assert_eq!(
            create(&emulator, ACCOUNT, &case).await.status,
            200,
            "case {calls} calls: expected CreateAccessPoint to answer 200"
        );

        // An access point that never carried a policy is already in the state
        // the removal asks for.
        let on_nothing = delete_policy(&emulator, ACCOUNT, case.name).await;
        assert_eq!(
            on_nothing.status, NO_CONTENT,
            "case {calls} calls: expected removing a policy that is not there to answer \
             {NO_CONTENT}, got {} with body {}",
            on_nothing.status, on_nothing.body
        );

        assert_eq!(
            put_policy(&emulator, ACCOUNT, case.name, case.policy)
                .await
                .status,
            NO_CONTENT,
            "case {calls} calls: expected the document to be stored"
        );
        for call in 1..=calls {
            let deleted = delete_policy(&emulator, ACCOUNT, case.name).await;
            assert_eq!(
                deleted.status, NO_CONTENT,
                "case {calls} calls: expected removal {call} to answer {NO_CONTENT}, got {} with \
                 body {}",
                deleted.status, deleted.body
            );
            let read = get_policy(&emulator, ACCOUNT, case.name).await;
            assert_eq!(
                (read.status, read.code().as_str()),
                NO_POLICY,
                "case {calls} calls: expected no policy after removal {call}, got {} with body {}",
                read.status,
                read.body
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_policy_refuses_an_access_point_the_account_does_not_own() {
    let emulator = Emulator::start();

    let owned = Case::new("owned by this account", "owned-ap", PUBLIC_POLICY);
    assert_eq!(create(&emulator, ACCOUNT, &owned).await.status, 200);
    assert_eq!(
        put_policy(&emulator, ACCOUNT, owned.name, owned.policy)
            .await
            .status,
        NO_CONTENT
    );

    for (label, account, name) in [
        ("a name no access point has", ACCOUNT, "no-such-ap"),
        (
            "a name no access point has, three characters long",
            ACCOUNT,
            "zzz",
        ),
    ] {
        let refused = delete_policy(&emulator, account, name).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (404, "NoSuchAccessPoint"),
            "case {label:?}: expected the missing-access-point refusal, got {} with body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some("The specified accesspoint does not exist"),
            "case {label:?}: expected the service's missing-access-point message, got body {}",
            refused.body
        );
        // The message is fixed text, so WHICH access point was asked about is
        // read out of the member beside it.
        assert_eq!(
            refused.text("AccessPointName").as_deref(),
            Some(name),
            "case {label:?}: expected the refusal to name the access point asked about, \
             got body {}",
            refused.body
        );
    }

    // A removal addressed in another account is refused ahead of the lookup, so
    // it names no access point: a name the caller's own account holds and one
    // nothing holds are the same answer there.
    for (label, account, name) in [
        ("another account, a name this account holds", OTHER_ACCOUNT, owned.name),
        ("another account, a name nothing holds", OTHER_ACCOUNT, "no-such-ap"),
        ("a third account", "210987654322", owned.name),
    ] {
        assert_foreign_account_refused(
            label,
            account,
            &delete_policy(&emulator, account, name).await,
        );
    }

    // A refused removal stored nothing: the owner's document is as it was.
    let read = get_policy(&emulator, ACCOUNT, owned.name).await;
    assert_eq!(
        read.text("Policy").as_deref(),
        Some(PUBLIC_POLICY),
        "expected the refused removals to leave the stored policy alone, got body {}",
        read.body
    );
}

#[tokio::test]
async fn deleting_a_policy_touches_only_the_named_access_point() {
    let emulator = Emulator::start();
    let rows = cases();

    for case in &rows {
        assert_eq!(
            create(&emulator, ACCOUNT, case).await.status,
            200,
            "case {:?}: expected CreateAccessPoint to answer 200",
            case.case
        );
        assert_eq!(
            put_policy(&emulator, ACCOUNT, case.name, case.policy)
                .await
                .status,
            NO_CONTENT,
            "case {:?}: expected the document to be stored",
            case.case
        );
    }

    // Remove the rows' policies one at a time: after each removal every row not
    // yet removed still reports its own document, byte for byte.
    for (removed, case) in rows.iter().enumerate() {
        assert_eq!(
            delete_policy(&emulator, ACCOUNT, case.name).await.status,
            NO_CONTENT,
            "case {:?}: expected DeleteAccessPointPolicy to answer {NO_CONTENT}",
            case.case
        );

        for (row, other) in rows.iter().enumerate() {
            let read = get_policy(&emulator, ACCOUNT, other.name).await;
            if row <= removed {
                assert_eq!(
                    (read.status, read.code().as_str()),
                    NO_POLICY,
                    "case {:?} after removing {:?}: expected no policy, got {} with body {}",
                    other.case,
                    case.case,
                    read.status,
                    read.body
                );
            } else {
                assert_eq!(
                    read.text("Policy").as_deref(),
                    Some(other.policy),
                    "case {:?} after removing {:?}: expected its own document untouched, \
                     got body {}",
                    other.case,
                    case.case,
                    read.body
                );
            }
        }
    }
}
