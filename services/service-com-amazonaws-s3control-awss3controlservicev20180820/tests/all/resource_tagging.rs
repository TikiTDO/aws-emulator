// SPDX-License-Identifier: CC0-1.0

//! The generic resource tagging surface — `TagResource`, `ListTagsForResource` and
//! `UntagResource` — driven end to end over HTTP.
//!
//! The rule under test: each of the three addresses ONE S3 resource through its
//! `ResourceArn` and reads or writes the ONE tag set of the resource that ARN names,
//! whichever kind of S3 resource it is and whichever store holds it, so a tag written
//! through this surface is the tag every reader of that resource's tags reports. Tagging
//! MERGES ("Creates a new user-defined tag or updates an existing tag", `TagResource`),
//! untagging removes exactly the keys it names, and a read reports the set without
//! changing it. An ARN naming a resource no store holds is refused as that resource being
//! missing, and an ARN naming no S3 resource at all as a bad request — both in this
//! service's own S3-style XML `<Code>`/`<Message>` envelope, never in a JSON
//! `ValidationException`.
//!
//! The rows span what the rule generalises over: the KIND of resource addressed (an
//! access point, a Storage Lens group, an S3 Access Grants instance, a registered Access
//! Grants location — four ARN forms, each read back from the ARN that resource's own
//! create or read reports), how many pairs one call carries, a key written twice, keys
//! removed one at a time, a key that was never there, and the tag-count bound and one
//! past it. The ARN forms this surface refuses are a table of their own.
//!
//! A general purpose bucket's tag set is Amazon S3's own, so the row that proves ONE tag
//! store per bucket needs both services on one engine and lives in
//! `core/router/tests/all/s3control_tags_a_general_purpose_bucket.rs`; what this suite
//! pins about a bucket ARN is the refusal when the bucket store holds no such bucket.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it per the
//! Smithy model, runs the handler and serializes the restXml response.

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION, XMLNS,
};
use crate::storage_lens_configuration::{leaf, reported_tags, tags_xml};
use crate::storage_lens_group::{create_group, get_group, Group};

/// The largest tag set an S3 resource carries: "You can add up to 50 Amazon Web Services
/// resource tags for each S3 resource" (`TagResource`). Spelled out here rather than read
/// from the implementation's own constant, so a change to that constant fails this suite.
const MAX_RESOURCE_TAGS: usize = 50;

/// An IAM role ARN satisfying `IAMRoleArn`'s pattern (`^arn:[^:]+:iam::\d{12}:role/`),
/// for the registered location a row addresses.
const LOCATION_ROLE: &str = "arn:aws:iam::123456789012:role/AccessGrantsLocationRole";

/// The code and the message this surface answers for an ARN naming a resource it holds
/// nothing under, whatever kind of resource the ARN names. Spelled out here rather than
/// read from the implementation's own constants, so a change to them fails this suite.
const NO_SUCH_RESOURCE_CODE: &str = "NoSuchResource";
const NO_SUCH_RESOURCE_MESSAGE: &str = "The specified resource doesn't exist.";

/// Which S3 resource a row addresses. Each kind is a different `ResourceArn` form, and
/// the ARN itself is always the one that resource's own create or read reports.
#[derive(Debug, Clone, Copy)]
enum Kind {
    AccessPoint,
    StorageLensGroup,
    AccessGrantsInstance,
    AccessGrantsLocation,
}

/// Create the account's S3 Access Grants instance and answer the ARN it reports.
///
/// An account has at most one instance per Region, so a second call is answered with the
/// conflict rather than a second instance; the ARN is then read from the instance that is
/// already there, which is the same resource.
async fn create_instance(emulator: &Emulator) -> String {
    let created = emulator
        .call(
            "POST",
            "/v20180820/accessgrantsinstance",
            ACCOUNT,
            &format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"/>"),
        )
        .await;
    let reply = match created.status {
        200 => created,
        409 => emulator
            .call("GET", "/v20180820/accessgrantsinstance", ACCOUNT, "")
            .await,
        status => panic!(
            "setup: CreateAccessGrantsInstance must succeed or conflict, got {status} {}",
            created.body
        ),
    };
    reply
        .text("AccessGrantsInstanceArn")
        .unwrap_or_else(|| panic!("an AccessGrantsInstanceArn in {}", reply.body))
}

/// Create the resource `kind` names under `id` and answer the ARN it reports for itself.
async fn provision(emulator: &Emulator, kind: Kind, id: &str) -> String {
    match kind {
        Kind::AccessPoint => {
            // An access point is created only over a bucket that exists.
            emulator.provision_bucket(id, &format!("{id}-bucket"));
            let created = emulator
                .call(
                    "PUT",
                    &format!("/v20180820/accesspoint/{id}"),
                    ACCOUNT,
                    &format!(
                        "<CreateAccessPointRequest xmlns=\"{XMLNS}\">\
                         <Bucket>{id}-bucket</Bucket></CreateAccessPointRequest>"
                    ),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "setup: CreateAccessPoint({id:?}) must succeed, got {} {}",
                created.status, created.body
            );
            created
                .text("AccessPointArn")
                .unwrap_or_else(|| panic!("an AccessPointArn in {}", created.body))
        }
        Kind::StorageLensGroup => {
            let group = Group {
                case: "a prefix filter",
                leaves: vec![leaf("Filter/MatchAnyPrefix/Prefix", "logs/")],
            };
            let created = create_group(emulator, ACCOUNT, id, &group, None).await;
            assert_eq!(
                created.status, 204,
                "setup: CreateStorageLensGroup({id:?}) must succeed, got {} {}",
                created.status, created.body
            );
            let read = get_group(emulator, ACCOUNT, id).await;
            read.text("StorageLensGroupArn")
                .unwrap_or_else(|| panic!("a StorageLensGroupArn in {}", read.body))
        }
        Kind::AccessGrantsInstance => create_instance(emulator).await,
        Kind::AccessGrantsLocation => {
            // A location lives in an instance, so the instance comes first.
            let _ = create_instance(emulator).await;
            let created = emulator
                .call(
                    "POST",
                    "/v20180820/accessgrantsinstance/location",
                    ACCOUNT,
                    &format!(
                        "<CreateAccessGrantsLocationRequest xmlns=\"{XMLNS}\">\
                         <LocationScope>s3://{id}/</LocationScope>\
                         <IAMRoleArn>{LOCATION_ROLE}</IAMRoleArn>\
                         </CreateAccessGrantsLocationRequest>"
                    ),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "setup: CreateAccessGrantsLocation({id:?}) must succeed, got {} {}",
                created.status, created.body
            );
            created
                .text("AccessGrantsLocationArn")
                .unwrap_or_else(|| panic!("an AccessGrantsLocationArn in {}", created.body))
        }
    }
}

/// The resource kinds every assertion below is run over, with the id each is created
/// under.
fn kinds() -> Vec<(&'static str, Kind, &'static str)> {
    vec![
        ("an access point", Kind::AccessPoint, "tagged-access-point"),
        (
            "a Storage Lens group",
            Kind::StorageLensGroup,
            "tagged-group",
        ),
        (
            "an S3 Access Grants instance",
            Kind::AccessGrantsInstance,
            "instance",
        ),
        (
            "a registered Access Grants location",
            Kind::AccessGrantsLocation,
            "tagged-location",
        ),
    ]
}

/// `POST /v20180820/tags/<arn>` carrying `pairs`.
async fn tag_resource(
    emulator: &Emulator,
    account_id: &str,
    resource_arn: &str,
    pairs: &[(String, String)],
) -> Reply {
    let body = format!(
        "<TagResourceRequest xmlns=\"{XMLNS}\">{}</TagResourceRequest>",
        tags_xml(Some(pairs))
    );
    emulator
        .call(
            "POST",
            &format!("/v20180820/tags/{resource_arn}"),
            account_id,
            &body,
        )
        .await
}

/// `GET /v20180820/tags/<arn>`.
async fn list_tags(emulator: &Emulator, account_id: &str, resource_arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/tags/{resource_arn}"),
            account_id,
            "",
        )
        .await
}

/// `DELETE /v20180820/tags/<arn>?tagKeys=…`, one query parameter per key — how a list
/// bound to `@httpQuery` travels.
async fn untag_resource(
    emulator: &Emulator,
    account_id: &str,
    resource_arn: &str,
    keys: &[&str],
) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .call(
            "DELETE",
            &format!("/v20180820/tags/{resource_arn}?{query}"),
            account_id,
            "",
        )
        .await
}

/// The key/value pairs a `ListTagsForResource` answer reports, in document order —
/// through the reader every tagging suite in this crate reads a tag set with.
fn reported(reply: &Reply) -> Vec<(String, String)> {
    reported_tags(&reply.body)
}

/// `pairs` as owned strings, sorted by key, for comparison against a report.
fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut owned: Vec<(String, String)> = entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    owned.sort();
    owned
}

/// The reported pairs, sorted by key: a tag set is a set, so no member order is part of
/// the contract.
fn sorted_report(reply: &Reply) -> Vec<(String, String)> {
    let mut found = reported(reply);
    found.sort();
    found
}

#[tokio::test]
async fn one_tag_set_per_resource_is_merged_read_and_removed_through_the_three_operations() {
    for (case, kind, id) in kinds() {
        // A fresh emulator per row: a row's resource is the only one in the store, so a
        // reported tag can only have come from the resource the ARN names.
        let emulator = Emulator::start();
        let arn = provision(&emulator, kind, id).await;

        // A resource nobody has tagged carries an empty tag set — reported as an empty
        // list, not as a refusal.
        let empty = list_tags(&emulator, ACCOUNT, &arn).await;
        assert_eq!(
            (empty.status, reported(&empty)),
            (200, Vec::new()),
            "case {case:?}: an untagged resource must report no tags, got {} {}",
            empty.status,
            empty.body
        );

        // Two pairs in one call.
        let tagged = tag_resource(
            &emulator,
            ACCOUNT,
            &arn,
            &pairs(&[("Team", "platform"), ("CostCenter", "engineering")]),
        )
        .await;
        assert_eq!(
            tagged.status, 204,
            "case {case:?}: expected the tagging call to be accepted with 204, got {} {}",
            tagged.status, tagged.body
        );
        let read = list_tags(&emulator, ACCOUNT, &arn).await;
        assert_eq!(
            sorted_report(&read),
            pairs(&[("CostCenter", "engineering"), ("Team", "platform")]),
            "case {case:?}: expected exactly the two pairs that were written, got {}",
            read.body
        );

        // A read reports; it must not change what the next caller sees.
        let again = list_tags(&emulator, ACCOUNT, &arn).await;
        assert_eq!(
            sorted_report(&again),
            sorted_report(&read),
            "case {case:?}: a second read must report the same tag set, got {}",
            again.body
        );

        // A second call MERGES: the repeated key is re-valued, the new one added, and
        // the untouched one kept.
        let merged = tag_resource(
            &emulator,
            ACCOUNT,
            &arn,
            &pairs(&[("Team", "storage"), ("Tier", "gold")]),
        )
        .await;
        assert_eq!(
            merged.status, 204,
            "case {case:?}: expected the merging call to be accepted with 204, got {} {}",
            merged.status, merged.body
        );
        let read = list_tags(&emulator, ACCOUNT, &arn).await;
        assert_eq!(
            sorted_report(&read),
            pairs(&[
                ("CostCenter", "engineering"),
                ("Team", "storage"),
                ("Tier", "gold"),
            ]),
            "case {case:?}: expected the re-valued, kept and added pairs, got {}",
            read.body
        );

        // Untagging removes exactly the keys it names — one present, one that was never
        // there, which is not an error.
        let untagged = untag_resource(&emulator, ACCOUNT, &arn, &["Tier", "Absent"]).await;
        assert_eq!(
            untagged.status, 204,
            "case {case:?}: expected the untagging call to be accepted with 204, got {} {}",
            untagged.status, untagged.body
        );
        let read = list_tags(&emulator, ACCOUNT, &arn).await;
        assert_eq!(
            sorted_report(&read),
            pairs(&[("CostCenter", "engineering"), ("Team", "storage")]),
            "case {case:?}: expected only the named key gone, got {}",
            read.body
        );

        // Repeating that call removes nothing more: what it states is the absence of
        // those keys afterwards.
        let repeated = untag_resource(&emulator, ACCOUNT, &arn, &["Tier", "Absent"]).await;
        assert_eq!(
            repeated.status, 204,
            "case {case:?}: expected the repeated untagging call to be accepted, got {} {}",
            repeated.status, repeated.body
        );
        let read = list_tags(&emulator, ACCOUNT, &arn).await;
        assert_eq!(
            sorted_report(&read),
            pairs(&[("CostCenter", "engineering"), ("Team", "storage")]),
            "case {case:?}: a repeated untagging call changed the tag set, got {}",
            read.body
        );

        // Removing the rest leaves the resource with no tags at all.
        let cleared = untag_resource(&emulator, ACCOUNT, &arn, &["Team", "CostCenter"]).await;
        assert_eq!(
            cleared.status, 204,
            "case {case:?}: expected the clearing call to be accepted with 204, got {} {}",
            cleared.status, cleared.body
        );
        let read = list_tags(&emulator, ACCOUNT, &arn).await;
        assert_eq!(
            (read.status, reported(&read)),
            (200, Vec::new()),
            "case {case:?}: expected no tags left, got {} {}",
            read.status,
            read.body
        );
    }
}

#[tokio::test]
async fn a_tag_set_is_bounded_at_the_published_maximum_and_a_refused_call_writes_nothing() {
    let emulator = Emulator::start();
    let arn = provision(&emulator, Kind::StorageLensGroup, "bounded-group").await;

    // Exactly the maximum fits. The keys are distinct, so the set really holds 50.
    let full: Vec<(String, String)> = (0..MAX_RESOURCE_TAGS)
        .map(|index| (format!("key-{index:02}"), "v".to_string()))
        .collect();
    let filled = tag_resource(&emulator, ACCOUNT, &arn, &full).await;
    assert_eq!(
        filled.status, 204,
        "case exactly {MAX_RESOURCE_TAGS} tags: expected them stored, got {} {}",
        filled.status, filled.body
    );
    let read = list_tags(&emulator, ACCOUNT, &arn).await;
    assert_eq!(
        reported(&read).len(),
        MAX_RESOURCE_TAGS,
        "case exactly {MAX_RESOURCE_TAGS} tags: expected all of them reported, got {}",
        read.body
    );

    // One key past it is refused, and the refusal leaves the set exactly as it was.
    let refused = tag_resource(&emulator, ACCOUNT, &arn, &pairs(&[("one-too-many", "v")])).await;
    assert_eq!(
        (refused.status, refused.code().as_str()),
        (400, "TooManyTagsException"),
        "case one tag past the maximum: expected the refusal, got {} {}",
        refused.status,
        refused.body
    );
    let read = list_tags(&emulator, ACCOUNT, &arn).await;
    assert_eq!(
        reported(&read).len(),
        MAX_RESOURCE_TAGS,
        "case one tag past the maximum: the refused call wrote tags, got {}",
        read.body
    );

    // Re-valuing a key AT the maximum adds nothing, so it fits.
    let revalued = tag_resource(&emulator, ACCOUNT, &arn, &pairs(&[("key-00", "re-valued")])).await;
    assert_eq!(
        revalued.status, 204,
        "case re-valuing a key at the maximum: expected it stored, got {} {}",
        revalued.status, revalued.body
    );
    let read = list_tags(&emulator, ACCOUNT, &arn).await;
    assert!(
        reported(&read).contains(&("key-00".to_string(), "re-valued".to_string()))
            && reported(&read).len() == MAX_RESOURCE_TAGS,
        "case re-valuing a key at the maximum: expected the new value and no growth, got {}",
        read.body
    );
}

#[tokio::test]
async fn an_arn_naming_no_resource_this_store_holds_is_refused_in_this_services_envelope() {
    let emulator = Emulator::start();
    // One resource of each kind really is there, so a refusal below is about the ARN the
    // request names rather than about an empty store.
    let mut present: Vec<String> = Vec::new();
    for (_, kind, id) in kinds() {
        present.push(provision(&emulator, kind, id).await);
    }
    let account = ::emulator_server::emulator_account_id();

    // Each row is an ARN this surface will not serve, with the code and status it is
    // refused with: a resource of a form this service holds but no record of, a bucket
    // the one bucket store does not hold, and a value that names no S3 resource at all.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "an access point nobody created",
            format!("arn:aws:s3:{REGION}:{account}:accesspoint/absent-point"),
            404,
            NO_SUCH_RESOURCE_CODE,
        ),
        (
            "a Storage Lens group nobody created",
            format!("arn:aws:s3:{REGION}:{account}:storage-lens-group/absent-group"),
            404,
            NO_SUCH_RESOURCE_CODE,
        ),
        (
            "a registered location nobody registered",
            format!("arn:aws:s3:{REGION}:{account}:access-grants/default/location/absent"),
            404,
            NO_SUCH_RESOURCE_CODE,
        ),
        (
            "an access grant nobody created",
            format!("arn:aws:s3:{REGION}:{account}:access-grants/default/grant/absent"),
            404,
            NO_SUCH_RESOURCE_CODE,
        ),
        (
            "an Access Grants instance in another Region",
            format!("arn:aws:s3:eu-west-1:{account}:access-grants/default"),
            404,
            NO_SUCH_RESOURCE_CODE,
        ),
        (
            // A bucket's tag set lives in the one bucket store, which holds no bucket of
            // this name here.
            "a general purpose bucket the bucket store does not hold",
            "arn:aws:s3:::absent-bucket".to_string(),
            404,
            NO_SUCH_RESOURCE_CODE,
        ),
        (
            "a directory bucket the bucket store does not hold",
            format!("arn:aws:s3express:{REGION}:{account}:bucket/absent--usw2-az1--x-s3"),
            404,
            NO_SUCH_RESOURCE_CODE,
        ),
        (
            "another vendor's resource ARN",
            format!("arn:aws:sqs:{REGION}:{account}:queue"),
            400,
            "InvalidRequest",
        ),
        (
            "an S3 ARN of a resource type this surface does not tag",
            format!("arn:aws:s3:{REGION}:{account}:storage-lens/default-account-dashboard"),
            400,
            "InvalidRequest",
        ),
        (
            "a value that is no ARN at all",
            "tagged-group".to_string(),
            400,
            "InvalidRequest",
        ),
    ];

    for (case, arn, status, code) in &rows {
        for (operation, reply) in [
            (
                "TagResource",
                tag_resource(&emulator, ACCOUNT, arn, &pairs(&[("Team", "platform")])).await,
            ),
            (
                "ListTagsForResource",
                list_tags(&emulator, ACCOUNT, arn).await,
            ),
            (
                "UntagResource",
                untag_resource(&emulator, ACCOUNT, arn, &["Team"]).await,
            ),
        ] {
            assert_eq!(
                (reply.status, reply.code().as_str()),
                (*status, *code),
                "case {case:?} through {operation}: expected {status} {code}, got {} {}",
                reply.status,
                reply.body
            );
            if *code == NO_SUCH_RESOURCE_CODE {
                assert_eq!(
                    reply.text("Message").unwrap_or_default(),
                    NO_SUCH_RESOURCE_MESSAGE,
                    "case {case:?} through {operation}: expected the published \
                     {NO_SUCH_RESOURCE_CODE} message, got {}",
                    reply.body
                );
            }
        }
    }

    // None of those refusals touched the resources that are there.
    for arn in &present {
        let read = list_tags(&emulator, ACCOUNT, arn).await;
        assert_eq!(
            (read.status, reported(&read)),
            (200, Vec::new()),
            "the refusals wrote tags onto {arn}, got {} {}",
            read.status,
            read.body
        );
    }
}

#[tokio::test]
async fn a_resource_is_tagged_through_the_account_that_owns_it() {
    let emulator = Emulator::start();
    let arn = provision(&emulator, Kind::StorageLensGroup, "owned-group").await;

    // The same ARN, addressed in an account the caller holds no credentials
    // for: the request is refused before the resource is resolved.
    let refused = tag_resource(
        &emulator,
        OTHER_ACCOUNT,
        &arn,
        &pairs(&[("Team", "platform")]),
    )
    .await;
    assert_foreign_account_refused("TagResource for another account", OTHER_ACCOUNT, &refused);

    // The owner's call is accepted, and only the owner reads the tags back.
    let tagged = tag_resource(&emulator, ACCOUNT, &arn, &pairs(&[("Team", "platform")])).await;
    assert_eq!(
        tagged.status, 204,
        "the owner's tagging call must be accepted, got {} {}",
        tagged.status, tagged.body
    );
    let read = list_tags(&emulator, ACCOUNT, &arn).await;
    assert_eq!(
        sorted_report(&read),
        pairs(&[("Team", "platform")]),
        "the owner must read the tag back, got {}",
        read.body
    );
    let theirs = list_tags(&emulator, OTHER_ACCOUNT, &arn).await;
    assert_foreign_account_refused(
        "ListTagsForResource for another account",
        OTHER_ACCOUNT,
        &theirs,
    );
}
