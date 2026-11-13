// SPDX-License-Identifier: CC0-1.0

//! Every write of a trail's log-delivery configuration re-resolves the destination.
//!
//! A trail delivers its log files — its Insights events among them — into one Amazon S3
//! bucket, so CloudTrail resolves that bucket and evaluates its bucket policy before it
//! accepts a configuration naming it: `S3BucketDoesNotExistException` for a destination
//! no bucket backs and `InsufficientS3BucketPolicyException` (HTTP 403) for one whose
//! policy does not let the CloudTrail service principal write the trail's log files
//! (`smithy_query(query="operation", operation="UpdateTrail,PutInsightSelectors")` binds
//! both on both).
//!
//! The rule under test is the TIMING of that resolution: it is made on EVERY such call,
//! against what the account holds at that call's instant, and before anything is
//! persisted. A bucket that was writable when the trail was created can have been
//! deleted or had its policy revoked since, out of band, and then no write of the
//! trail's delivery holds — whichever members the request carries, and whether it turns
//! Insights on or off. A refused call changes nothing a later caller sees, and the
//! identical call succeeds once the destination permits the delivery again.
//!
//! The rows span the class: each write this rule reaches, crossed with each way a
//! destination stops permitting the delivery (no bucket at all, a bucket with no policy,
//! a policy granting another principal, a policy granting the write under another key
//! prefix) and the row where it permits it.
//!
//! `StartLogging` is the boundary: its model binds neither error, so it is not held to
//! this check — and that is asserted too, so the rule is not read wider than it is.

use serde_json::{json, Value};

use super::emulator::{Emulator, HOME_REGION};

/// The HTTP status CloudTrail answers a destination that does not exist with, where the
/// model's own annotation is 404 — the status `request_validation` pins for the same
/// refusal on `CreateTrail` and `UpdateTrail`.
const BUCKET_ABSENT_STATUS: u16 = 400;

/// This account's id, as the policies below name it in their resource ARNs.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// What the account holds for the trail's destination when the write under test is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// The bucket is gone — a `DeleteBucket` made after the trail was created.
    Deleted,
    /// The bucket is there and carries no policy at all, which is the state a
    /// `DeleteBucketPolicy` leaves it in.
    PolicyRevoked,
    /// The bucket's policy grants the delivery to another service principal.
    AnotherPrincipal,
    /// The bucket's policy grants CloudTrail the write under a key prefix this trail
    /// does not deliver under.
    AnotherKeyPrefix,
    /// The bucket's policy lets CloudTrail deliver this trail's log files.
    Writable,
}

impl Destination {
    /// The refusal this destination state is answered with, or `None` for the state that
    /// permits the delivery.
    fn refusal(self) -> Option<(u16, &'static str)> {
        match self {
            Self::Deleted => Some((BUCKET_ABSENT_STATUS, "S3BucketDoesNotExistException")),
            Self::PolicyRevoked | Self::AnotherPrincipal | Self::AnotherKeyPrefix => {
                Some((403, "InsufficientS3BucketPolicyException"))
            }
            Self::Writable => None,
        }
    }

    /// Put the account's object store into this state for `bucket`.
    fn apply(self, emulator: &Emulator, bucket: &str) {
        match self {
            Self::Deleted => assert!(
                emulator.delete_bucket(bucket),
                "the fixture bucket {bucket} must be there to delete"
            ),
            Self::PolicyRevoked => {
                emulator.provision_bucket_with_policy(bucket, HOME_REGION, None);
            }
            Self::AnotherPrincipal => emulator.provision_bucket_with_policy(
                bucket,
                HOME_REGION,
                Some(&delivery_policy(
                    json!({"Service": "config.amazonaws.com"}),
                    &format!("arn:aws:s3:::{bucket}/*"),
                )),
            ),
            Self::AnotherKeyPrefix => emulator.provision_bucket_with_policy(
                bucket,
                HOME_REGION,
                Some(&delivery_policy(
                    json!({"Service": "cloudtrail.amazonaws.com"}),
                    &format!("arn:aws:s3:::{bucket}/elsewhere/AWSLogs/{}/*", account()),
                )),
            ),
            Self::Writable => emulator.provision_bucket(bucket, HOME_REGION),
        }
    }
}

/// A bucket policy granting `principal` the log-file write on `resource`, under the
/// canned ACL a CloudTrail delivery carries — the one shape the destination rows vary.
fn delivery_policy(principal: Value, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "Delivery",
            "Effect": "Allow",
            "Principal": principal,
            "Action": "s3:PutObject",
            "Resource": resource,
            "Condition": {"StringEquals": {"s3:x-amz-acl": "bucket-owner-full-control"}},
        }],
    })
    .to_string()
}

/// The write under test: a call that configures where or what the trail delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    /// `UpdateTrail` re-designating the same destination under a new key prefix.
    RedesignateDestination,
    /// `UpdateTrail` carrying no destination member at all, so the destination judged is
    /// the one the trail already delivers to.
    UpdateWithoutDestinationMembers,
    /// `PutInsightSelectors` turning Insights event logging ON.
    EnableInsights,
    /// `PutInsightSelectors` turning Insights event logging OFF again, which is what an
    /// empty statement does.
    DisableInsights,
}

impl Write {
    fn operation(self) -> &'static str {
        match self {
            Self::RedesignateDestination | Self::UpdateWithoutDestinationMembers => "UpdateTrail",
            Self::EnableInsights | Self::DisableInsights => "PutInsightSelectors",
        }
    }

    fn request(self, trail: &str, bucket: &str) -> Value {
        match self {
            Self::RedesignateDestination => {
                json!({"Name": trail, "S3BucketName": bucket, "S3KeyPrefix": "denied-update"})
            }
            Self::UpdateWithoutDestinationMembers => {
                json!({"Name": trail, "IncludeGlobalServiceEvents": false})
            }
            Self::EnableInsights => json!({
                "TrailName": trail,
                "InsightSelectors": [{"InsightType": "ApiCallRateInsight"}],
            }),
            Self::DisableInsights => json!({"TrailName": trail, "InsightSelectors": []}),
        }
    }

    /// Whether Insights have to be ON before the write is made, so that the write really
    /// has something to change: turning them off changes nothing on a trail that never
    /// had them.
    fn needs_insights_enabled(self) -> bool {
        self == Self::DisableInsights
    }
}

/// Everything a later caller can see of a trail's configuration: the trail itself and
/// its Insights selectors, read through the implemented readers.
///
/// Both are reads, so taking this snapshot cannot be what changes the next one.
async fn configuration(emulator: &Emulator, trail: &str) -> Value {
    let read = emulator.call("GetTrail", &json!({"Name": trail})).await;
    let insights = emulator
        .call("GetInsightSelectors", &json!({"TrailName": trail}))
        .await;
    json!({
        "Trail": read.expect_ok(&format!("GetTrail {trail}")).clone(),
        "InsightsStatus": insights.status,
        "Insights": insights.body,
    })
}

/// A trail whose destination is writable, and the bucket it delivers to.
async fn trail_on_a_writable_destination(emulator: &Emulator, name: &str) -> String {
    let bucket = format!("{name}-destination");
    emulator.provision_bucket(&bucket, HOME_REGION);
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": name, "S3BucketName": bucket, "IncludeGlobalServiceEvents": true}),
        )
        .await
        .expect_ok(&format!("CreateTrail {name} against {bucket}"));
    bucket
}

/// Every write of a trail's delivery is judged against the destination the account holds
/// NOW: it is refused while the destination does not permit the delivery, it persists
/// nothing when refused, and the identical call succeeds once it does again.
#[tokio::test]
async fn a_write_of_a_trails_delivery_is_judged_against_the_destination_held_now() {
    let writes = [
        Write::RedesignateDestination,
        Write::UpdateWithoutDestinationMembers,
        Write::EnableInsights,
        Write::DisableInsights,
    ];
    let destinations = [
        Destination::Deleted,
        Destination::PolicyRevoked,
        Destination::AnotherPrincipal,
        Destination::AnotherKeyPrefix,
        Destination::Writable,
    ];

    for write in writes {
        for destination in destinations {
            let case = format!("{write:?} against {destination:?}");
            // A fresh emulator per row: each row states for itself what the account
            // holds, and the harness provisions no destination of its own.
            let emulator = Emulator::without_provisioned_destinations();
            let trail = "revalidated-trail";
            let bucket = trail_on_a_writable_destination(&emulator, trail).await;
            if write.needs_insights_enabled() {
                emulator
                    .call(
                        "PutInsightSelectors",
                        &Write::EnableInsights.request(trail, &bucket),
                    )
                    .await
                    .expect_ok(&format!("{case}: enable Insights while the destination holds"));
            }

            destination.apply(&emulator, &bucket);
            let before = configuration(&emulator, trail).await;
            let request = write.request(trail, &bucket);
            let reply = emulator.call(write.operation(), &request).await;

            match destination.refusal() {
                Some((status, code)) => {
                    reply.expect_error(&case, status, code);
                    let after = configuration(&emulator, trail).await;
                    assert_eq!(
                        after, before,
                        "case {case}: a refused write must leave the trail exactly as it \
                         was, got {after:?}"
                    );
                }
                None => {
                    reply.expect_ok(&case);
                }
            }

            // The remedy: once the destination permits the delivery again, the IDENTICAL
            // call is accepted — so the refusal was about the destination and nothing
            // else about the request.
            Destination::Writable.apply(&emulator, &bucket);
            let retried = emulator.call(write.operation(), &request).await;
            retried.expect_ok(&format!("{case}: retried once the destination permits it"));
        }
    }
}

/// The refusal names the bucket the caller has to fix, in CloudTrail's own sentence for
/// an insufficient bucket policy — the same sentence whichever write was refused.
#[tokio::test]
async fn an_insufficient_destination_policy_refusal_names_the_bucket() {
    use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::referenced_resources;

    for write in [
        Write::RedesignateDestination,
        Write::UpdateWithoutDestinationMembers,
        Write::EnableInsights,
    ] {
        let case = format!("{write:?} on a bucket whose policy was revoked");
        let emulator = Emulator::without_provisioned_destinations();
        let trail = "wording-trail";
        let bucket = trail_on_a_writable_destination(&emulator, trail).await;
        Destination::PolicyRevoked.apply(&emulator, &bucket);

        let reply = emulator
            .call(write.operation(), &write.request(trail, &bucket))
            .await;
        reply.expect_error(&case, 403, "InsufficientS3BucketPolicyException");
        assert_eq!(
            reply.message(),
            referenced_resources::insufficient_s3_bucket_policy_message(&bucket),
            "case {case}: expected CloudTrail's own sentence for an insufficient bucket \
             policy, got {:?}",
            reply.body
        );
    }
}

/// Insights stay OFF after a refused enable, so the read answers
/// `InsightNotEnabledException` — and the enable that follows the remedy is what turns
/// them on.
#[tokio::test]
async fn insights_stay_off_after_an_enable_the_destination_refused() {
    let emulator = Emulator::without_provisioned_destinations();
    let trail = "insights-after-refusal";
    let bucket = trail_on_a_writable_destination(&emulator, trail).await;
    Destination::PolicyRevoked.apply(&emulator, &bucket);

    let request = Write::EnableInsights.request(trail, &bucket);
    emulator
        .call("PutInsightSelectors", &request)
        .await
        .expect_error(
            "enable Insights on a trail whose destination policy was revoked",
            403,
            "InsufficientS3BucketPolicyException",
        );

    let read = emulator
        .call("GetInsightSelectors", &json!({"TrailName": trail}))
        .await;
    read.expect_error(
        "GetInsightSelectors after a refused enable",
        400,
        "InsightNotEnabledException",
    );
    let read_trail = emulator.call("GetTrail", &json!({"Name": trail})).await;
    assert_eq!(
        read_trail.expect_ok("GetTrail after a refused enable")["Trail"]["HasInsightSelectors"],
        json!(false),
        "case a refused enable: expected HasInsightSelectors false, got {:?}",
        read_trail.body
    );

    // The remedy, and the round trip through the implemented reader.
    Destination::Writable.apply(&emulator, &bucket);
    emulator
        .call("PutInsightSelectors", &request)
        .await
        .expect_ok("enable Insights once the destination policy is back");
    let read = emulator
        .call("GetInsightSelectors", &json!({"TrailName": trail}))
        .await;
    let body = read.expect_ok("GetInsightSelectors once Insights are enabled");
    assert_eq!(
        body["InsightSelectors"],
        json!([{"InsightType": "ApiCallRateInsight", "EventCategories": ["Management"]}]),
        "case the remedied enable: expected the statement that was accepted, got {:?}",
        read.body
    );
}

/// The boundary of the rule: `StartLogging` models neither destination refusal, so it is
/// not held to the check — a trail whose destination no longer permits the delivery
/// still starts logging, and reports that it is logging.
#[tokio::test]
async fn starting_logging_is_not_held_to_the_destination_check() {
    for destination in [Destination::Deleted, Destination::PolicyRevoked] {
        let case = format!("StartLogging with the destination {destination:?}");
        let emulator = Emulator::without_provisioned_destinations();
        let trail = "logging-trail";
        let bucket = trail_on_a_writable_destination(&emulator, trail).await;
        destination.apply(&emulator, &bucket);

        emulator
            .call("StartLogging", &json!({"Name": trail}))
            .await
            .expect_ok(&case);
        let status = emulator
            .call("GetTrailStatus", &json!({"Name": trail}))
            .await;
        assert_eq!(
            status.expect_ok(&case)["IsLogging"],
            json!(true),
            "case {case}: expected IsLogging true, got {:?}",
            status.body
        );
    }
}

/// A trail the write would be refused for is also the trail a write of an UNRELATED
/// configuration is refused for: the destination is what the check is about, so the
/// refusal does not depend on the members the request carries.
///
/// The rows are the members `UpdateTrail` takes beside the destination, each on its own
/// trail, so none of them is what made the call hold or fail.
#[tokio::test]
async fn an_update_carrying_no_destination_member_is_refused_all_the_same() {
    let rows: Vec<(&str, Value)> = vec![
        ("the global-service-events flag", json!({"IncludeGlobalServiceEvents": true})),
        ("the multi-Region flag", json!({"IsMultiRegionTrail": true})),
        ("log file validation", json!({"EnableLogFileValidation": true})),
        ("recursive logging", json!({"RecursiveLogging": true})),
    ];

    for (case, members) in rows {
        let emulator = Emulator::without_provisioned_destinations();
        let trail = "unrelated-update";
        let bucket = trail_on_a_writable_destination(&emulator, trail).await;
        Destination::PolicyRevoked.apply(&emulator, &bucket);

        let mut request = json!({"Name": trail});
        for (member, value) in members.as_object().expect("the row is an object") {
            request[member] = value.clone();
        }
        let before = configuration(&emulator, trail).await;
        let reply = emulator.call("UpdateTrail", &request).await;
        reply.expect_error(case, 403, "InsufficientS3BucketPolicyException");
        let after = configuration(&emulator, trail).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the trail exactly as it was, got \
             {after:?}"
        );

        // Restoring the destination's policy is the whole remedy: the same update holds.
        Destination::Writable.apply(&emulator, &bucket);
        emulator
            .call("UpdateTrail", &request)
            .await
            .expect_ok(&format!("{case}: once the destination policy is back"));
    }
}
