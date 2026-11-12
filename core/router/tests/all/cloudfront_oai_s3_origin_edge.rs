// SPDX-License-Identifier: CC0-1.0

//! A CloudFront distribution serving a PRIVATE Amazon S3 origin through a LEGACY ORIGIN
//! ACCESS IDENTITY, end to end over HTTP across the two services on one engine.
//!
//! RULE: `S3OriginConfig$OriginAccessIdentity` names "the CloudFront origin access identity
//! to associate with the origin", in the documented form
//! `origin-access-identity/cloudfront/ID-of-origin-access-identity`, and such an identity
//! exists so that "viewers can only access objects in an Amazon S3 bucket through
//! CloudFront": the BUCKET's own policy grants the read to that identity — named by either
//! spelling AWS publishes for it, the IAM principal of CloudFront's own account or the
//! `S3CanonicalUserId` the identity was minted with — so a fetch that policy grants is made
//! for the distribution and the viewer is served the object's bytes, while a request a viewer
//! makes to the bucket DIRECTLY, presenting no credential, stays refused. A fetch the policy
//! does not grant is not made for the identity either, so the origin answers it as it answers
//! any caller presenting no credential, in its own vocabulary.
//!
//! Only a cross-service suite can pin this: the identity and the distribution are
//! CloudFront's records, the document that grants it and the object it guards are Amazon
//! S3's, and the rule is observable only where both are mounted on one engine. The
//! `cloudfront_oac_s3_origin_edge` suite beside it pins the same shape for the modern ORIGIN
//! ACCESS CONTROL; this one is the legacy mechanism, whose grant names an IDENTITY rather than
//! the service principal.
//!
//! The rows span what the rule generalises over: WHICH spelling the bucket policy names the
//! identity by (the IAM principal of CloudFront's account, the canonical user id), WHICH
//! identity it names (this distribution's, and another one the account also holds), whether
//! the bucket carries a policy at all, and the key form of the object (a bare name, a nested
//! prefix).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::PROPAGATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::cloudfront_oac_s3_origin_edge::{
    assert_deployed, call, create_bucket, emulator, origin_domain, put_bucket_policy, put_object,
    regional_endpoint, view, Answer, API_VERSION, CLOUDFRONT_HOST,
};

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: every
/// viewer request under it reaches the origin, so each row's answer is the origin's own.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// The code Amazon S3 refuses a read nothing granted with (Amazon S3 "Error responses").
const ACCESS_DENIED: &str = "AccessDenied";

/// Long enough past a propagation deadline that the clock owes the distribution its
/// deployment, whatever span the service chose.
fn past_propagation() -> Duration {
    PROPAGATION_DELAY + Duration::from_millis(250)
}

/// `cloudfront:CreateCloudFrontOriginAccessIdentity`, answering the identity's
/// `(Id, S3CanonicalUserId)`.
async fn create_origin_access_identity(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
) -> (String, String) {
    let body = format!(
        "<CloudFrontOriginAccessIdentityConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Comment>an identity a private bucket grants</Comment>\
         </CloudFrontOriginAccessIdentityConfig>"
    );
    let created = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/origin-access-identity/cloudfront"),
        &[("content-type", "application/xml")],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateCloudFrontOriginAccessIdentity must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let id = created.element("Id");
    let canonical = created.element("S3CanonicalUserId");
    assert!(
        !id.is_empty() && !canonical.is_empty(),
        "case {case:?}: the created identity reports no Id or S3CanonicalUserId: {:?}",
        created.text()
    );
    (id, canonical)
}

/// `cloudfront:CreateDistribution` over the S3 origin `bucket` fronted by the origin access
/// identity `identity`, answering the distribution's `(id, DomainName)`.
///
/// `S3OriginConfig$OriginAccessIdentity` carries the documented reference form, which is what
/// a client writes and what the bucket policy's grant is then written for.
async fn create_distribution(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    bucket: &str,
    identity: &str,
) -> (String, String) {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>origin-bucket</Id><DomainName>{}</DomainName>\
         <S3OriginConfig>\
         <OriginAccessIdentity>origin-access-identity/cloudfront/{identity}</OriginAccessIdentity>\
         </S3OriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>origin-bucket</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <Comment>a legacy origin access identity over a private s3 origin</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>",
        origin_domain(bucket)
    );
    let created = call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        &[("content-type", "application/xml")],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateDistribution must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let (id, domain) = (created.element("Id"), created.element("DomainName"));
    assert!(
        !id.is_empty() && !domain.is_empty(),
        "case {case:?}: the created distribution reports no Id or DomainName: {:?}",
        created.text()
    );
    (id, domain)
}

/// A bucket policy granting `s3:GetObject` on `resource` to the principal the row names,
/// under the principal KIND it names it by: `AWS` for the IAM principal of CloudFront's own
/// account, `CanonicalUser` for the `S3CanonicalUserId` the identity was minted with. Both
/// are spellings AWS publishes for an origin access identity.
fn identity_policy(kind: &str, principal: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\
         \"Sid\":\"AllowTheOriginAccessIdentity\",\"Effect\":\"Allow\",\
         \"Principal\":{{\"{kind}\":\"{principal}\"}},\
         \"Action\":\"s3:GetObject\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// The IAM principal an origin access identity is named by in a bucket policy, as the Amazon
/// S3 User Guide writes it.
fn identity_principal(id: &str) -> String {
    format!("arn:aws:iam::cloudfront:user/CloudFront Origin Access Identity {id}")
}

/// A direct, credential-less request for the object at the BUCKET's own endpoint — the
/// request an origin access identity exists to keep refused.
async fn direct_read(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("/{bucket}/{key}"))
        .header("host", regional_endpoint())
        .body(Bytes::new())
        .expect("a well-formed direct request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Which principal spelling a row's bucket policy names the identity by, and whose identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// The `AWS` principal of this origin's own identity.
    IamPrincipal,
    /// The `CanonicalUser` of this origin's own identity.
    CanonicalUser,
    /// The `AWS` principal of ANOTHER identity the account holds.
    AnotherIdentity,
    /// No policy at all.
    NoPolicy,
}

/// One row: a private bucket holding one object, a distribution over it fronted by an origin
/// access identity, the document the bucket carries — and what the viewer is then served.
struct Row {
    case: &'static str,
    /// The bucket, this row's own so one row's grant cannot admit another's read.
    bucket: &'static str,
    key: &'static str,
    body: &'static str,
    grant: Grant,
    /// Whether the viewer is served the object, or the origin's own refusal.
    served: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a grant to the identity's IAM principal",
        bucket: "oai-edge-granted-arn",
        key: "plain.txt",
        body: "the object behind the identity",
        grant: Grant::IamPrincipal,
        served: true,
    },
    Row {
        case: "a grant to the identity's canonical user id",
        bucket: "oai-edge-granted-canonical",
        key: "assets/site/index.html",
        body: "<p>the nested object</p>",
        grant: Grant::CanonicalUser,
        served: true,
    },
    Row {
        case: "a grant to another identity of the same account",
        bucket: "oai-edge-another-identity",
        key: "plain.txt",
        body: "the object another identity was granted",
        grant: Grant::AnotherIdentity,
        served: false,
    },
    Row {
        case: "a bucket carrying no policy at all",
        bucket: "oai-edge-no-policy",
        key: "plain.txt",
        body: "the object nothing was granted",
        grant: Grant::NoPolicy,
        served: false,
    },
];

#[tokio::test]
async fn a_legacy_origin_access_identity_serves_the_object_the_bucket_granted_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    // The setup plane: authentication off, so each call carries a credential scope without a
    // computed signature. The VIEWER plane is the authenticating one, which is what makes the
    // credential the fetch carries the thing S3 serves it for.
    let setup = emulator(&engine, false);
    let viewers = emulator(&engine, true);

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        create_bucket(&setup, case, row.bucket).await;
        put_object(&setup, case, row.bucket, row.key, row.body, &[]).await;
        let (identity, canonical) = create_origin_access_identity(
            &setup,
            case,
            &format!("oai-edge-identity-{index}"),
        )
        .await;
        // A SECOND identity the account also holds, which is what the "another identity" row
        // grants: an identity that exists and is not this origin's.
        let (other_identity, _other_canonical) = create_origin_access_identity(
            &setup,
            case,
            &format!("oai-edge-other-identity-{index}"),
        )
        .await;
        let (id, domain) = create_distribution(
            &setup,
            case,
            &format!("oai-edge-reference-{index}"),
            row.bucket,
            &identity,
        )
        .await;

        let resource = format!("arn:aws:s3:::{}/*", row.bucket);
        let document = match row.grant {
            Grant::IamPrincipal => Some(identity_policy(
                "AWS",
                &identity_principal(&identity),
                &resource,
            )),
            Grant::CanonicalUser => {
                Some(identity_policy("CanonicalUser", &canonical, &resource))
            }
            Grant::AnotherIdentity => Some(identity_policy(
                "AWS",
                &identity_principal(&other_identity),
                &resource,
            )),
            Grant::NoPolicy => None,
        };
        if let Some(document) = &document {
            put_bucket_policy(&setup, case, row.bucket, document).await;
        }

        ::tokio::time::sleep(past_propagation()).await;
        assert_deployed(&setup, case, &id).await;

        let path = format!("/{}", row.key);
        let served = view(&viewers, &domain, &path).await;
        if row.served {
            assert_eq!(
                (served.status, served.text().as_str()),
                (200, row.body),
                "case {case:?}: expected the object's own bytes through the edge, got {} with \
                 {:?}",
                served.status,
                served.text()
            );
        } else {
            assert_ne!(
                served.status, 200,
                "case {case:?}: a fetch the bucket granted this identity nothing for must not \
                 be served, got {:?}",
                served.text()
            );
            assert!(
                !served.text().contains(row.body),
                "case {case:?}: a refused fetch must carry none of the object, got {:?}",
                served.text()
            );
            assert!(
                served.text().contains(ACCESS_DENIED),
                "case {case:?}: expected the origin's own refusal, got {} with {:?}",
                served.status,
                served.text()
            );
        }

        // The point of the identity, in every row: the bucket itself serves no viewer. A
        // direct, credential-less read of the same key stays refused whatever the
        // distribution was served.
        let direct = direct_read(&viewers, row.bucket, row.key).await;
        assert_eq!(
            direct.status, 403,
            "case {case:?}: a direct request to the bucket must stay refused, got {} with {:?}",
            direct.status,
            direct.text()
        );
        assert!(
            !direct.text().contains(row.body),
            "case {case:?}: a direct request must carry none of the object, got {:?}",
            direct.text()
        );
    }
}
