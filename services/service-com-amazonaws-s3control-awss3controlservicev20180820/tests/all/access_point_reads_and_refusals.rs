// SPDX-License-Identifier: CC0-1.0

//! What an access point READ reports, and how an access point request this
//! service will not serve is refused, driven end to end over HTTP.
//!
//! The rules under test:
//!
//! * every read of an access point reports the whole resource — the account
//!   that owns the bucket it fronts (the one the create named, or the account
//!   that owns the access point when the create named none), what the access
//!   point fronts, and the Regional hosts its data plane is reached at, derived
//!   from the Region the access point lives in — whether the read is
//!   `GetAccessPoint` or an entry of `ListAccessPoints`, and however many
//!   entries that listing holds;
//! * a refusal states the CLASS of failure in the fixed words the service uses
//!   for it, with no request value interpolated into the message: what the
//!   request named travels in the members beside the message, and the
//!   correlation pair names the response it answers;
//! * an access point name is one segment of the request path, so a name longer
//!   than the wire can address makes the path unparseable — refused before
//!   anything is read, created or deleted;
//! * a request body this service cannot READ is refused as a request it will
//!   not carry out, whatever the member and whatever the value.
//!
//! Every assertion goes through the front door a client uses, and every rule is
//! driven over a TABLE whose rows differ along what the rule generalises over:
//! the Region the request is signed for, whether the create named the bucket's
//! account, how many access points the listing holds, which operation names the
//! access point, how long the name is, and which member of the body carries an
//! unreadable value.
//!
//! The harness is the sibling suite's: the same router, request builder and XML
//! readers a client's request meets.

use std::collections::{BTreeMap, BTreeSet};

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION,
    XMLNS,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::ACCESS_POINT_NAME_LEN;

/// The keys `GetAccessPoint$Endpoints` reports, and the host each one names in
/// Region `region`: the Regional S3 access point endpoints, over plain IPv4 and
/// over the FIPS and dual-stack variants of the same service.
///
/// Written out here, rather than read from the emulator, so the test states the
/// hosts a client resolves independently of the code that builds them.
fn expected_endpoints(region: &str) -> BTreeMap<String, String> {
    [
        ("ipv4", format!("s3-accesspoint.{region}.amazonaws.com")),
        ("fips", format!("s3-accesspoint-fips.{region}.amazonaws.com")),
        (
            "fips_dualstack",
            format!("s3-accesspoint-fips.dualstack.{region}.amazonaws.com"),
        ),
        (
            "dualstack",
            format!("s3-accesspoint.dualstack.{region}.amazonaws.com"),
        ),
    ]
    .into_iter()
    .map(|(key, host)| (key.to_string(), host))
    .collect()
}

/// The type an access point over a general-purpose bucket reports, which is the
/// one kind `CreateAccessPoint` makes: `Scope` (directory buckets) is refused
/// and no data source is ever attached.
const REGULAR: &str = "Regular";

/// The path segment every route of this service is bound under, and the one part
/// of a request path a refusal's `URI` member does NOT carry: that member names
/// the resource the request addressed, relative to the service root.
const API_VERSION_PREFIX: &str = "/v20180820/";

/// A name at the longest length the wire can ADDRESS an access point by — the
/// maximum of `ACCESS_POINT_NAME_LEN` (50), which
/// `state::addressable_access_point_name` holds a path segment to — so the
/// not-found rows span the name lengths a refusal can be asked about.
const LONGEST_ADDRESSABLE_NAME: &str = "ap-name-padded-out-to-the-addressable-maximum-0050";

/// The `<Endpoints>` map of a reply, read as key/value pairs: the restXml
/// wrapped-map layout is `<Endpoints><entry><key>..</key><value>..</value>
/// </entry>…</Endpoints>`, so the keys and values pair up by position.
fn endpoints_of(reply: &Reply) -> BTreeMap<String, String> {
    let keys = reply.texts("key");
    let values = reply.texts("value");
    keys.into_iter().zip(values).collect()
}

/// One `CreateAccessPoint` request this suite sends: the members that decide
/// what a later read must report.
struct Created {
    case: &'static str,
    name: &'static str,
    bucket: &'static str,
    /// The account the create names as the bucket's owner, or `None` for a
    /// same-account access point, which the model says may leave it out.
    bucket_account_id: Option<&'static str>,
}

impl Created {
    const fn new(case: &'static str, name: &'static str, bucket: &'static str) -> Self {
        Self {
            case,
            name,
            bucket,
            bucket_account_id: None,
        }
    }

    fn body(&self) -> String {
        let mut xml = format!("<CreateAccessPointRequest xmlns=\"{XMLNS}\">");
        xml.push_str(&format!("<Bucket>{}</Bucket>", self.bucket));
        if let Some(account) = self.bucket_account_id {
            xml.push_str(&format!("<BucketAccountId>{account}</BucketAccountId>"));
        }
        xml.push_str("</CreateAccessPointRequest>");
        xml
    }

    /// The account a read must report in `BucketAccountId`.
    fn expected_bucket_account(&self) -> &'static str {
        self.bucket_account_id.unwrap_or(ACCOUNT)
    }
}

/// The access points every projection assertion is driven over. The rows differ
/// in whether the create named the bucket's owning account and in the bucket
/// they front, not merely in their names.
fn created() -> Vec<Created> {
    vec![
        Created::new("same-account bucket, nothing optional", "reports-ap", "reports"),
        Created {
            bucket_account_id: Some(ACCOUNT),
            ..Created::new("the owning account named explicitly", "explicit-ap", "reports")
        },
        Created {
            bucket_account_id: Some(OTHER_ACCOUNT),
            ..Created::new("a cross-account bucket", "partner-ap", "partner")
        },
        Created::new("another bucket again", "finance-ap", "finance"),
    ]
}

async fn create(emulator: &Emulator, account: &str, case: &Created) -> Reply {
    // An access point is created only over a bucket that exists.
    emulator.provision_bucket(case.case, case.bucket);
    emulator
        .call(
            "PUT",
            &format!("/v20180820/accesspoint/{}", case.name),
            account,
            &case.body(),
        )
        .await
}

async fn get(emulator: &Emulator, account: &str, name: &str) -> Reply {
    emulator
        .call("GET", &format!("/v20180820/accesspoint/{name}"), account, "")
        .await
}

/// Whether a reply names the request it answers: the correlation pair beside
/// `<Error>`, mirrored in the two response headers a client that does not parse
/// the body reads it from.
fn names_its_request(reply: &Reply) -> bool {
    let request_id = reply.text("RequestId");
    let host_id = reply.text("HostId");
    request_id.as_deref().is_some_and(|id| !id.is_empty())
        && host_id.as_deref().is_some_and(|id| !id.is_empty())
        && reply.header("x-amz-request-id") == request_id
        && reply.header("x-amz-id-2") == host_id
}

#[tokio::test]
async fn a_read_reports_the_bucket_owner_the_type_and_the_regional_endpoints() {
    // One row per Region, because every endpoint host and the ARN are derived
    // from the Region the access point lives in.
    for region in [REGION, "us-east-1", "eu-central-1", "ap-southeast-2"] {
        let emulator = Emulator::start();
        let cases = created();
        for case in &cases {
            emulator.provision_bucket(case.case, case.bucket);
            let created = call_in_region(
                &emulator.router,
                region,
                "PUT",
                &format!("/v20180820/accesspoint/{}", case.name),
                ACCOUNT,
                &case.body(),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {:?} in {region}: expected the create to succeed, got {} with body {}",
                case.case, created.status, created.body
            );
        }

        for case in &cases {
            let read = call_in_region(
                &emulator.router,
                region,
                "GET",
                &format!("/v20180820/accesspoint/{}", case.name),
                ACCOUNT,
                "",
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {:?} in {region}: expected GetAccessPoint to answer 200, got {} with \
                 body {}",
                case.case, read.status, read.body
            );
            assert_eq!(
                read.text("BucketAccountId").as_deref(),
                Some(case.expected_bucket_account()),
                "case {:?} in {region}: expected the account that owns the bucket, got body {}",
                case.case,
                read.body
            );
            assert_eq!(
                read.text("AccessPointType").as_deref(),
                Some(REGULAR),
                "case {:?} in {region}: expected the access point's type, got body {}",
                case.case,
                read.body
            );
            assert_eq!(
                endpoints_of(&read),
                expected_endpoints(region),
                "case {:?} in {region}: expected the Regional access point endpoints, got \
                 body {}",
                case.case,
                read.body
            );

            // A read may not change what a later reader sees.
            let again = call_in_region(
                &emulator.router,
                region,
                "GET",
                &format!("/v20180820/accesspoint/{}", case.name),
                ACCOUNT,
                "",
            )
            .await;
            // Compared member by member rather than byte for byte: the
            // `Endpoints` map's ENTRY ORDER is not part of the contract, and
            // the codec writes the entries in the order the map holds them.
            for member in [
                "Name",
                "Bucket",
                "NetworkOrigin",
                "CreationDate",
                "Alias",
                "AccessPointArn",
                "BucketAccountId",
                "AccessPointType",
            ] {
                assert_eq!(
                    again.text(member),
                    read.text(member),
                    "case {:?} in {region}: expected a repeated read to report the same \
                     {member}, got body {}",
                    case.case,
                    again.body
                );
            }
            assert_eq!(
                endpoints_of(&again),
                endpoints_of(&read),
                "case {:?} in {region}: expected a repeated read to report the same endpoints, \
                 got body {}",
                case.case,
                again.body
            );
        }
    }
}

#[tokio::test]
async fn every_entry_of_a_listing_carries_what_a_read_of_it_carries() {
    let cases = created();

    // One row per listing size: a listing of one entry, and of each size up to
    // the whole set — the projection is per entry, so it must hold however many
    // entries a page holds. Each size gets its own emulator, so the listing
    // holds exactly that many access points.
    for held in 1..=cases.len() {
        let emulator = Emulator::start();
        for case in cases.iter().take(held) {
            assert_eq!(
                create(&emulator, ACCOUNT, case).await.status,
                200,
                "case {:?}: expected the create to succeed",
                case.case
            );
        }

        let listed = emulator
            .call("GET", "/v20180820/accesspoint", ACCOUNT, "")
            .await;
        assert_eq!(
            listed.status, 200,
            "case {held} entries: expected ListAccessPoints to answer 200, got {} with body {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.texts("Name").len(),
            held,
            "case {held} entries: expected that many entries, got body {}",
            listed.body
        );
        // Every entry carries the type, and the bucket owners of the listing are
        // exactly the ones the creates left on those access points.
        assert_eq!(
            listed.texts("AccessPointType"),
            vec![REGULAR.to_string(); held],
            "case {held} entries: expected every entry to carry the access point type, got \
             body {}",
            listed.body
        );
        let expected_owners: BTreeSet<String> = cases
            .iter()
            .take(held)
            .map(|case| case.expected_bucket_account().to_string())
            .collect();
        assert_eq!(
            listed
                .texts("BucketAccountId")
                .into_iter()
                .collect::<BTreeSet<_>>(),
            expected_owners,
            "case {held} entries: expected each entry to name its bucket's owning account, \
             got body {}",
            listed.body
        );

        // The entry says what a read of the same access point says.
        for case in cases.iter().take(held) {
            let read = get(&emulator, ACCOUNT, case.name).await;
            assert_eq!(
                read.text("BucketAccountId"),
                Some(case.expected_bucket_account().to_string()),
                "case {:?} in a listing of {held}: expected the listing and the read to agree",
                case.case
            );
        }
    }
}

/// What a name outside the rules is answered with: the error code a client
/// branches on and the message beside it. `None` means the name is served.
type NameRefusal = Option<(&'static str, &'static str)>;

#[tokio::test]
async fn the_two_rules_an_access_point_name_is_held_to_are_refused_apart() {
    let emulator = Emulator::start();
    let shortest = *ACCESS_POINT_NAME_LEN.start();
    let longest = *ACCESS_POINT_NAME_LEN.end();

    // The rules split across the stages that read the name: parsing the path
    // reads the name as part of the host it addresses, within the length a name
    // may have, and the parsed name is then held to the name's own rules.
    const UNPARSEABLE: NameRefusal = Some(("InvalidURI", "Couldn't parse the specified URI."));
    const INVALID_NAME: NameRefusal =
        Some(("InvalidRequest", "Your Amazon S3 AccessPoint name is invalid"));

    // Each published length bound, one past each of them, and one row per rule.
    let rows: Vec<(&str, String, NameRefusal)> = vec![
        ("at the shortest length", "a".repeat(shortest), None),
        ("one under the shortest length", "a".repeat(shortest - 1), INVALID_NAME),
        ("one inside the longest length", "b".repeat(longest - 1), None),
        ("at the longest length", "c".repeat(longest), None),
        ("one past the longest length", "d".repeat(longest + 1), UNPARSEABLE),
        ("far past the longest length", "e".repeat(longest * 3), UNPARSEABLE),
        ("digits and hyphens", "ap-7-9".to_string(), None),
        ("an upper-case letter", "Reports-ap".to_string(), UNPARSEABLE),
        ("an underscore", "reports_ap".to_string(), UNPARSEABLE),
        ("a dot", "reports.ap".to_string(), INVALID_NAME),
        ("a leading hyphen", "-reports-ap".to_string(), INVALID_NAME),
        ("a trailing hyphen", "reports-ap-".to_string(), INVALID_NAME),
        ("the alias suffix", "reports-s3alias".to_string(), INVALID_NAME),
        ("an escaped character", "reports%20ap".to_string(), UNPARSEABLE),
        ("a sub-delimiter", "reports+ap".to_string(), UNPARSEABLE),
    ];

    for (case, name, refusal) in rows {
        let body = format!(
            "<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>reports</Bucket>\
             </CreateAccessPointRequest>"
        );
        emulator.provision_bucket(case, "reports");
        let path = format!("/v20180820/accesspoint/{name}");
        let Some((code, message)) = refusal else {
            let created = emulator.call("PUT", &path, ACCOUNT, &body).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected a name the rules admit to be created, got {} with \
                 body {}",
                created.status, created.body
            );
            assert_eq!(
                get(&emulator, ACCOUNT, &name).await.status,
                200,
                "case {case:?}: expected the created access point to be readable"
            );
            continue;
        };

        // Every route that addresses an access point by name is refused the
        // same way, and each names the resource path the caller addressed.
        for (route, method, uri) in [
            ("CreateAccessPoint", "PUT", path.clone()),
            ("GetAccessPoint", "GET", path.clone()),
            ("DeleteAccessPoint", "DELETE", path.clone()),
            ("GetAccessPointPolicy", "GET", format!("{path}/policy")),
            ("GetAccessPointPolicyStatus", "GET", format!("{path}/policyStatus")),
        ] {
            let refused = emulator.call(method, &uri, ACCOUNT, &body).await;
            assert_eq!(
                (refused.status, refused.code().as_str()),
                (400, code),
                "case {case:?} on {route}: expected the name to be refused, got {} with body {}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.text("Message").as_deref(),
                Some(message),
                "case {case:?} on {route}: expected the fixed message, got body {}",
                refused.body
            );
            // What the `URI` member names is the stage's own: a segment longer
            // than the wire can carry as a name is not read as a resource name
            // at all, so the refusal echoes the request target; every other
            // refusal names the resource it did read, under the service root.
            let named = match name.chars().count() > longest {
                true => Some(uri.as_str()),
                false => uri.strip_prefix(API_VERSION_PREFIX),
            };
            assert_eq!(
                refused.text("URI").as_deref(),
                named,
                "case {case:?} on {route}: expected the refusal to name {named:?}, got body {}",
                refused.body
            );
            assert!(
                names_its_request(&refused),
                "case {case:?} on {route}: expected the refusal to name the request it answers, \
                 got body {}",
                refused.body
            );
        }

        // The refused create stored nothing: the listing is untouched.
        let listed = emulator
            .call("GET", "/v20180820/accesspoint", ACCOUNT, "")
            .await;
        assert!(
            !listed.texts("Name").iter().any(|listed| listed == &name),
            "case {case:?}: expected a refused name to create nothing, got body {}",
            listed.body
        );
    }
}

#[tokio::test]
async fn a_body_this_service_cannot_read_is_refused_as_an_invalid_request() {
    let emulator = Emulator::start();

    // One row per member, per unreadable value and per operation: each of the
    // four `PublicAccessBlockConfiguration` settings carrying something that is
    // not a boolean, the same on the operation whose whole BODY is that shape,
    // and an integer member carrying something that is not a number. None of
    // them is a value this service can read, so all of them are the one
    // refusal.
    let mut rows: Vec<(String, &str, &str, String)> = Vec::new();
    for setting in [
        "BlockPublicAcls",
        "IgnorePublicAcls",
        "BlockPublicPolicy",
        "RestrictPublicBuckets",
    ] {
        for value in ["maybe", "2", "TRUE-ish"] {
            rows.push((
                format!("CreateAccessPoint: {setting} carrying {value:?}"),
                "PUT",
                "/v20180820/accesspoint/unreadable-ap",
                format!(
                    "<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>reports</Bucket>\
                     <PublicAccessBlockConfiguration><{setting}>{value}</{setting}>\
                     </PublicAccessBlockConfiguration></CreateAccessPointRequest>"
                ),
            ));
            rows.push((
                format!("PutPublicAccessBlock: {setting} carrying {value:?}"),
                "PUT",
                "/v20180820/configuration/publicAccessBlock",
                format!(
                    "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">\
                     <{setting}>{value}</{setting}></PublicAccessBlockConfiguration>"
                ),
            ));
        }
    }
    for value in ["soon", "high", "1.5"] {
        rows.push((
            format!("CreateJob: an integer member carrying {value:?}"),
            "POST",
            "/v20180820/jobs",
            format!(
                "<CreateJobRequest xmlns=\"{XMLNS}\"><Priority>{value}</Priority>\
                 </CreateJobRequest>"
            ),
        ));
    }

    for (case, method, uri, body) in rows {
        let refused = emulator.call(method, uri, ACCOUNT, &body).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (400, "InvalidRequest"),
            "case {case:?}: expected the request to be refused as invalid, got {} with body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some("Request invalid"),
            "case {case:?}: expected the fixed message, with no request value in it, got body {}",
            refused.body
        );
        assert!(
            names_its_request(&refused),
            "case {case:?}: expected the refusal to name the request it answers, got body {}",
            refused.body
        );
    }

    // The refused creates stored nothing.
    assert_eq!(
        get(&emulator, ACCOUNT, "unreadable-ap").await.status,
        404,
        "expected a refused create to store nothing"
    );
}

#[tokio::test]
async fn a_refusal_states_its_class_in_fixed_words_and_names_what_was_asked_about() {
    let emulator = Emulator::start();
    let owned = Created::new("owned", "owned-ap", "reports");
    assert_eq!(create(&emulator, ACCOUNT, &owned).await.status, 200);

    // Every operation that names an access point, against a name the account
    // does not own — including a name another account DOES own, which is not
    // this account's access point.
    let absent = "absent-ap";
    let rows: Vec<(&str, &str, &str, String, &str)> = vec![
        ("GetAccessPoint", "GET", ACCOUNT, format!("/v20180820/accesspoint/{absent}"), absent),
        ("DeleteAccessPoint", "DELETE", ACCOUNT, format!("/v20180820/accesspoint/{absent}"), absent),
        (
            "GetAccessPointPolicy",
            "GET",
            ACCOUNT,
            format!("/v20180820/accesspoint/{absent}/policy"),
            absent,
        ),
        (
            "GetAccessPointPolicyStatus",
            "GET",
            ACCOUNT,
            format!("/v20180820/accesspoint/{absent}/policyStatus"),
            absent,
        ),
        (
            "GetAccessPoint, a name at the longest addressable length",
            "GET",
            ACCOUNT,
            format!("/v20180820/accesspoint/{LONGEST_ADDRESSABLE_NAME}"),
            LONGEST_ADDRESSABLE_NAME,
        ),
        // The shortest name the model admits, so the row set spans the name
        // lengths a not-found refusal can be asked about.
        ("GetAccessPoint, shortest name", "GET", ACCOUNT, "/v20180820/accesspoint/zzz".to_string(), "zzz"),
    ];
    for (case, method, account, uri, named) in rows {
        let refused = emulator.call(method, &uri, account, "").await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (404, "NoSuchAccessPoint"),
            "case {case:?}: expected the missing-access-point refusal, got {} with body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some("The specified accesspoint does not exist"),
            "case {case:?}: expected the fixed message, with no name in it, got body {}",
            refused.body
        );
        assert_eq!(
            refused.text("AccessPointName").as_deref(),
            Some(named),
            "case {case:?}: expected the refusal to name the access point asked about, got \
             body {}",
            refused.body
        );
        assert!(
            names_its_request(&refused),
            "case {case:?}: expected the refusal to name the request it answers, got body {}",
            refused.body
        );
    }

    // The same calls addressed in another account are refused ahead of the
    // lookup, so they name no access point at all: the refusal names the
    // ACCOUNT the request asked for instead, and says the same thing for a name
    // this account holds as for one nothing holds.
    for (case, method, uri) in [
        (
            "GetAccessPoint in another account, name present",
            "GET",
            format!("/v20180820/accesspoint/{}", owned.name),
        ),
        (
            "GetAccessPoint in another account, name absent",
            "GET",
            format!("/v20180820/accesspoint/{absent}"),
        ),
        (
            "DeleteAccessPoint in another account",
            "DELETE",
            format!("/v20180820/accesspoint/{}", owned.name),
        ),
        (
            "GetAccessPointPolicy in another account",
            "GET",
            format!("/v20180820/accesspoint/{}/policy", owned.name),
        ),
    ] {
        let refused = emulator.call(method, &uri, OTHER_ACCOUNT, "").await;
        assert_foreign_account_refused(case, OTHER_ACCOUNT, &refused);
        assert!(
            refused.text("AccessPointName").is_none(),
            "case {case:?}: expected the refusal to name no access point, got body {}",
            refused.body
        );
        assert!(
            names_its_request(&refused),
            "case {case:?}: expected the refusal to name the request it answers, got body {}",
            refused.body
        );
    }

    // Re-creating an access point the caller already owns: the same fixed-words
    // rule, over a row set that differs in the members the repeat carries.
    for (case, repeat) in [
        ("a byte-identical repeat", Created::new("repeat", owned.name, owned.bucket)),
        (
            "a repeat naming another bucket",
            Created::new("repeat", owned.name, "a-different-bucket"),
        ),
        (
            "a repeat naming another bucket owner",
            Created {
                bucket_account_id: Some(OTHER_ACCOUNT),
                ..Created::new("repeat", owned.name, owned.bucket)
            },
        ),
    ] {
        let conflict = create(&emulator, ACCOUNT, &repeat).await;
        assert_eq!(
            (conflict.status, conflict.code().as_str()),
            (409, "AccessPointAlreadyOwnedByYou"),
            "case {case:?}: expected the conflict refusal, got {} with body {}",
            conflict.status,
            conflict.body
        );
        assert_eq!(
            conflict.text("Message").as_deref(),
            Some("Your previous request to create the named accesspoint succeeded and you \
                  already own it."),
            "case {case:?}: expected the fixed message, with no name in it, got body {}",
            conflict.body
        );
        assert!(
            names_its_request(&conflict),
            "case {case:?}: expected the refusal to name the request it answers, got body {}",
            conflict.body
        );
    }

    // The refused repeats left the stored access point alone.
    let read = get(&emulator, ACCOUNT, owned.name).await;
    assert_eq!(
        read.text("Bucket").as_deref(),
        Some(owned.bucket),
        "expected the refused repeats to leave the stored bucket alone, got body {}",
        read.body
    );
    assert_eq!(
        read.text("BucketAccountId").as_deref(),
        Some(owned.expected_bucket_account()),
        "expected the refused repeats to leave the stored bucket owner alone, got body {}",
        read.body
    );
}
