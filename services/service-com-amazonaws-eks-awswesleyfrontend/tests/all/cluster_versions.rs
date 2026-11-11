// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeClusterVersions`, driven over the wire through the
//! same `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test is: the operation reports the Kubernetes version calendar
//! this service's state holds, narrowing it by the request's filters, and each
//! row's support status is DERIVED from the clock and that row's own
//! end-of-support dates — standard support before the end-of-standard-support
//! instant, extended support before the end-of-extended-support instant,
//! unsupported after it — in both modelled spellings.
//!
//! So every suite asserts over a TABLE whose rows differ along what the rule
//! generalises over (where the clock sits relative to a row's dates, which filter
//! is applied, which spelling of the status filter, page size), not over one
//! recorded request. The seeded calendar is exercised as it ships; the
//! clock-relative classes are exercised by replacing the calendar in state with a
//! table of rows placed at chosen offsets from now, which is the only way to put
//! the clock in every class deterministically.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    self, ClusterVersionRecord, CLUSTER_TYPE_EKS, DEFAULT_KUBERNETES_VERSION,
};
use service_com_amazonaws_eks_awswesleyfrontend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::wire::{call, error_code, Answer};

/// One day, the unit the version calendar is published in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The emulator under test, plus the shared engine, so a suite can seed this
/// service's version calendar the way the production store seeds it.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// `GET /cluster-versions`, with an optional query string.
async fn describe_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/cluster-versions".to_string()
    } else {
        format!("/cluster-versions?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

/// The `clusterVersions` array of a successful answer.
fn rows(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a clusterVersions list, got {:?}", answer.body))
        .clone()
}

/// The `clusterVersion` of every row of an answer, in the order reported.
fn reported_versions(answer: &Answer) -> Vec<String> {
    rows(answer)
        .iter()
        .map(|row| {
            row["clusterVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a clusterVersion string, got {row:?}"))
                .to_string()
        })
        .collect()
}

/// A modelled timestamp as epoch seconds, whichever wire form the protocol used
/// (restJson1 defaults to epoch seconds; a `@timestampFormat` would make it
/// RFC-3339).
fn epoch_secs(value: &serde_json::Value) -> f64 {
    match value {
        serde_json::Value::Number(number) => number
            .as_f64()
            .unwrap_or_else(|| panic!("expected a numeric timestamp, got {number}")),
        serde_json::Value::String(text) => ::aws_smithy_types::DateTime::from_str(
            text,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| panic!("{text} is not an RFC-3339 instant: {error}"))
        .as_secs_f64(),
        other => panic!("expected a timestamp, got {other:?}"),
    }
}

/// Seconds since the epoch, as the handler's own clock reads them.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// The support status the RULE gives for a row with these dates at `now`,
/// as the `(versionStatus, status)` pair the response reports.
///
/// Both reported members carry the CURRENT (`VersionStatus`) spelling: `status`
/// "is deprecated. Use `versionStatus` instead, as that field matches for input
/// and output of this action", so the deprecated member's output tokens are not
/// the `ClusterVersionStatus` tokens its own filter accepts.
fn expected_status(now: f64, end_of_standard: f64, end_of_extended: f64) -> (&'static str, &'static str) {
    if now < end_of_standard {
        ("STANDARD_SUPPORT", "STANDARD_SUPPORT")
    } else if now < end_of_extended {
        ("EXTENDED_SUPPORT", "EXTENDED_SUPPORT")
    } else {
        ("UNSUPPORTED", "UNSUPPORTED")
    }
}

/// A Kubernetes version `"1.31"` as the pair `(1, 31)`, so ordering is compared
/// numerically rather than lexically. `pub(crate)` so every suite that has to
/// order the published calendar's versions reads them the same way.
pub(crate) fn version_key(version: &str) -> (u32, u32) {
    let (major, minor) = version
        .split_once('.')
        .unwrap_or_else(|| panic!("expected a <major>.<minor> version, got {version:?}"));
    (
        major.parse().unwrap_or_else(|_| panic!("major of {version:?}")),
        minor.parse().unwrap_or_else(|_| panic!("minor of {version:?}")),
    )
}

/// The calendar a suite installs to put the clock in every support class at once:
/// one row per class, plus a row the clock has not reached the release date of.
/// Rows are NEWEST VERSION FIRST, which is the order the calendar publishes.
fn clock_relative_calendar(now: SystemTime) -> Vec<ClusterVersionRecord> {
    let row = |version: &str, default_version: bool, release: SystemTime, standard: SystemTime, extended: SystemTime| {
        ClusterVersionRecord {
            cluster_version: version.to_string(),
            cluster_type: CLUSTER_TYPE_EKS.to_string(),
            default_version,
            release_date: release,
            end_of_standard_support: standard,
            end_of_extended_support: extended,
        }
    };
    vec![
        // Not released yet, so not an available version.
        row("2.13", false, now + 30 * DAY, now + 500 * DAY, now + 900 * DAY),
        // Inside standard support, and the default version.
        row("2.12", true, now - 10 * DAY, now + 60 * DAY, now + 425 * DAY),
        // Standard support ended 30 days ago, extended support runs on.
        row("2.11", false, now - 500 * DAY, now - 30 * DAY, now + 300 * DAY),
        // Extended support ended 200 days ago.
        row("2.10", false, now - 1000 * DAY, now - 600 * DAY, now - 200 * DAY),
    ]
}

/// Install `calendar` as this service's published version calendar.
fn seed_calendar(engine: &Arc<engine::InMemoryEngine>, calendar: Vec<ClusterVersionRecord>) {
    let store = state::service_state(&**engine).expect("the eks store is reachable in-process");
    store.lock().cluster_versions = calendar;
}

/// The calendar as it ships is reported in full, newest version first, with both
/// status spellings agreeing with the dates each row carries.
#[tokio::test]
async fn the_published_calendar_is_reported_from_state() {
    let (router, _engine) = emulator();

    let answer = describe_versions(&router, "").await;
    assert_eq!(
        answer.status, 200,
        "case plain catalog query: expected HTTP 200, got {} ({:?})",
        answer.status, answer.body
    );
    let now = now_secs();
    let listed = rows(&answer);
    assert!(
        !listed.is_empty(),
        "case plain catalog query: the calendar must publish at least one version, got {:?}",
        answer.body
    );
    assert!(
        answer.body.get("nextToken").is_none_or(serde_json::Value::is_null),
        "case plain catalog query: the whole calendar fits one page, got nextToken {:?}",
        answer.body.get("nextToken")
    );

    let mut defaults: Vec<String> = Vec::new();
    for row in &listed {
        let case = row["clusterVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a clusterVersion string, got {row:?}"))
            .to_string();
        assert_eq!(
            row["clusterType"].as_str(),
            Some(CLUSTER_TYPE_EKS),
            "case {case}: expected clusterType {CLUSTER_TYPE_EKS:?}, got {:?}",
            row["clusterType"]
        );
        assert!(
            row["defaultPlatformVersion"]
                .as_str()
                .is_some_and(|platform| platform.starts_with("eks.")),
            "case {case}: expected an eks.<n> defaultPlatformVersion, got {:?}",
            row["defaultPlatformVersion"]
        );

        let release = epoch_secs(&row["releaseDate"]);
        let end_of_standard = epoch_secs(&row["endOfStandardSupportDate"]);
        let end_of_extended = epoch_secs(&row["endOfExtendedSupportDate"]);
        assert!(
            release < end_of_standard && end_of_standard < end_of_extended,
            "case {case}: expected release < end of standard < end of extended, got \
             {release} {end_of_standard} {end_of_extended}"
        );
        assert!(
            release <= now,
            "case {case}: an unreleased version is not an available one, got release {release} \
             at now {now}"
        );

        let (version_status, status) = expected_status(now, end_of_standard, end_of_extended);
        assert_eq!(
            row["versionStatus"].as_str(),
            Some(version_status),
            "case {case}: expected versionStatus {version_status:?} from its own dates, got {:?}",
            row["versionStatus"]
        );
        assert_eq!(
            row["status"].as_str(),
            Some(status),
            "case {case}: expected the deprecated status {status:?}, got {:?}",
            row["status"]
        );

        if row["defaultVersion"].as_bool() == Some(true) {
            defaults.push(case);
        }
    }

    assert_eq!(
        defaults,
        vec![DEFAULT_KUBERNETES_VERSION.to_string()],
        "case default version: expected exactly the version CreateCluster defaults to, got \
         {defaults:?}"
    );

    let reported = reported_versions(&answer);
    let keys: Vec<(u32, u32)> = reported.iter().map(|version| version_key(version)).collect();
    assert!(
        keys.windows(2).all(|pair| pair[0] > pair[1]),
        "case ordering: expected the newest Kubernetes version first, got {reported:?}"
    );

    // The calendar must still be publishing versions the clock has not run past:
    // a catalog whose every row is out of support is one a caller cannot create a
    // cluster from, which is what a table of fixed dates decays into.
    let in_standard_support: Vec<&String> = reported
        .iter()
        .zip(listed.iter())
        .filter(|(_, row)| row["versionStatus"].as_str() == Some("STANDARD_SUPPORT"))
        .map(|(version, _)| version)
        .collect();
    assert!(
        !in_standard_support.is_empty(),
        "case support window: the calendar must publish a version in standard support at now \
         {now}, got {reported:?}"
    );
}

/// A cluster created without a `version` runs the version this catalog reports as
/// the default one — read back through `DescribeCluster`, so the catalog and the
/// cluster slice are proven to agree rather than assumed to.
#[tokio::test]
async fn the_default_version_is_what_create_cluster_uses() {
    let (router, _engine) = emulator();

    let answer = describe_versions(&router, "defaultOnly=true").await;
    assert_eq!(
        answer.status, 200,
        "case defaultOnly: expected HTTP 200, got {} ({:?})",
        answer.status, answer.body
    );
    let defaults = reported_versions(&answer);
    assert_eq!(
        defaults.len(),
        1,
        "case defaultOnly: expected exactly one default version, got {defaults:?}"
    );
    let default_version = defaults[0].clone();

    let body = serde_json::json!({
        "name": "versions-default",
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    let created = call(&router, REGION, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        created.status, 200,
        "case create without version: expected HTTP 200, got {} ({:?})",
        created.status, created.body
    );
    let described = call(
        &router,
        REGION,
        "GET",
        "/clusters/versions-default",
        "",
    )
    .await;
    assert_eq!(
        described.status, 200,
        "case read back: expected HTTP 200, got {} ({:?})",
        described.status, described.body
    );
    assert_eq!(
        described.body["cluster"]["version"].as_str(),
        Some(default_version.as_str()),
        "case read back: expected the catalog's default version {default_version:?}, got {:?}",
        described.body["cluster"]["version"]
    );
}

/// Where the clock sits relative to a row's own dates decides the row's status,
/// and a row whose release date has not arrived is not reported at all.
#[tokio::test]
async fn support_status_is_derived_from_the_clock() {
    let (router, engine) = emulator();
    let now = SystemTime::now();
    seed_calendar(&engine, clock_relative_calendar(now));

    // (row, expected versionStatus, expected deprecated status, reported at all)
    let cases = [
        ("2.10", "UNSUPPORTED", "UNSUPPORTED", true),
        ("2.11", "EXTENDED_SUPPORT", "EXTENDED_SUPPORT", true),
        ("2.12", "STANDARD_SUPPORT", "STANDARD_SUPPORT", true),
        ("2.13", "STANDARD_SUPPORT", "STANDARD_SUPPORT", false),
    ];

    // Every row the calendar holds is read with `includeAll`, because an unfiltered
    // listing publishes only the versions Amazon EKS still offers (the suite below
    // pins that); what this case is about is the status each row reports.
    let answer = describe_versions(&router, "includeAll=true").await;
    assert_eq!(
        answer.status, 200,
        "case clock-relative calendar: expected HTTP 200, got {} ({:?})",
        answer.status, answer.body
    );
    let listed = rows(&answer);

    for (version, version_status, status, reported) in cases {
        let found = listed
            .iter()
            .find(|row| row["clusterVersion"].as_str() == Some(version));
        match (reported, found) {
            (false, None) => {}
            (false, Some(row)) => panic!(
                "case {version}: a version released in the future must not be reported, got {row:?}"
            ),
            (true, None) => panic!(
                "case {version}: expected the version to be reported, got {:?}",
                reported_versions(&answer)
            ),
            (true, Some(row)) => {
                assert_eq!(
                    row["versionStatus"].as_str(),
                    Some(version_status),
                    "case {version}: expected versionStatus {version_status:?}, got {:?}",
                    row["versionStatus"]
                );
                assert_eq!(
                    row["status"].as_str(),
                    Some(status),
                    "case {version}: expected status {status:?}, got {:?}",
                    row["status"]
                );
            }
        }
    }
}

/// Each filter narrows the listing to the rows that satisfy it, in either
/// spelling of the status filter, and filters combine.
#[tokio::test]
async fn filters_narrow_the_listing() {
    let (router, engine) = emulator();
    let now = SystemTime::now();
    seed_calendar(&engine, clock_relative_calendar(now));

    // (case, query, expected versions in order)
    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        ("no filter", String::new(), vec!["2.12", "2.11"]),
        (
            "one named version",
            "clusterVersions=2.11".to_string(),
            vec!["2.11"],
        ),
        (
            // `clusterVersions` NARROWS the listing; it does not widen it to a
            // version the service no longer offers, which only `includeAll` or a
            // status filter naming the unsupported state does.
            "two named versions, one of them past extended support",
            "clusterVersions=2.10&clusterVersions=2.12".to_string(),
            vec!["2.12"],
        ),
        (
            "a version the calendar does not publish",
            "clusterVersions=0.1".to_string(),
            vec![],
        ),
        (
            "a named version that is not released yet",
            "clusterVersions=2.13".to_string(),
            vec![],
        ),
        (
            "the published cluster type",
            format!("clusterType={CLUSTER_TYPE_EKS}"),
            vec!["2.12", "2.11"],
        ),
        (
            "another cluster type",
            "clusterType=eks-anywhere".to_string(),
            vec![],
        ),
        ("default only", "defaultOnly=true".to_string(), vec!["2.12"]),
        (
            "default only, off",
            "defaultOnly=false".to_string(),
            vec!["2.12", "2.11"],
        ),
        (
            "versionStatus STANDARD_SUPPORT",
            "versionStatus=STANDARD_SUPPORT".to_string(),
            vec!["2.12"],
        ),
        (
            "versionStatus EXTENDED_SUPPORT",
            "versionStatus=EXTENDED_SUPPORT".to_string(),
            vec!["2.11"],
        ),
        (
            "versionStatus UNSUPPORTED",
            "versionStatus=UNSUPPORTED".to_string(),
            vec!["2.10"],
        ),
        (
            "the deprecated status spelling",
            "status=extended-support".to_string(),
            vec!["2.11"],
        ),
        (
            "both spellings, agreeing",
            "status=unsupported&versionStatus=UNSUPPORTED".to_string(),
            vec!["2.10"],
        ),
        (
            "both spellings, disagreeing",
            "status=unsupported&versionStatus=STANDARD_SUPPORT".to_string(),
            vec![],
        ),
        (
            "includeAll widens the listing to the versions EKS no longer offers",
            "includeAll=true".to_string(),
            vec!["2.12", "2.11", "2.10"],
        ),
        (
            "includeAll=false is the unfiltered listing",
            "includeAll=false".to_string(),
            vec!["2.12", "2.11"],
        ),
        (
            "includeAll with a status filter still narrows to that status",
            "includeAll=true&versionStatus=EXTENDED_SUPPORT".to_string(),
            vec!["2.11"],
        ),
        (
            "a status filter and a named version that satisfies it",
            "clusterVersions=2.12&versionStatus=STANDARD_SUPPORT".to_string(),
            vec!["2.12"],
        ),
        (
            "a status filter and a named version that does not",
            "clusterVersions=2.12&versionStatus=UNSUPPORTED".to_string(),
            vec![],
        ),
    ];

    for (case, query, expected) in cases {
        let answer = describe_versions(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} ({:?})",
            answer.status, answer.body
        );
        let actual = reported_versions(&answer);
        assert_eq!(
            actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }

    // A status filter whose token neither modelled enum declares is REFUSED (400),
    // in either spelling, rather than silently ignored or answered with an empty
    // page. Whether the refusal comes from schema validation or from the handler,
    // it is a client error that names the member.
    let refused = [
        ("versionStatus in the deprecated spelling", "versionStatus=extended-support"),
        ("status in the current spelling", "status=EXTENDED_SUPPORT"),
        ("a status token no enum declares", "versionStatus=NOT_A_STATUS"),
    ];
    for (case, query) in refused {
        let answer = describe_versions(&router, query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} ({:?})",
            answer.status, answer.body
        );
        let kind = answer.error_type.clone().unwrap_or_default();
        assert!(
            kind.contains("InvalidParameterException") || kind.contains("ValidationException"),
            "case {case}: expected a client error naming the bad parameter, got {kind:?} \
             ({:?})",
            answer.body
        );
    }
}

/// A `clusterVersions` entry that does not name Kubernetes versions is REFUSED, while
/// every entry that does is a filter — whether or not the calendar publishes it.
///
/// The rows differ along what the rule generalises over: how many dot-separated
/// components the value has, whether each is numeric, how many versions one entry
/// carries and how many entries the filter has, and whether a value that names none
/// sits beside one that does.
#[tokio::test]
async fn a_cluster_version_filter_must_be_a_cluster_version() {
    let (router, engine) = emulator();
    let now = SystemTime::now();
    seed_calendar(&engine, clock_relative_calendar(now));

    // The sentence this filter's own refusal states, written out rather than read from
    // the constant the handler renders it from, so this check fails if the wording
    // changes.
    let refusal =
        "Provided Cluster Versions are invalid, it must be comma separated versions ie 1.27,1.28";

    // (case, the query, whether the filter names Kubernetes versions). The form is
    // `<major>.<minor>`, which is the form every row of the calendar carries
    // (`ClusterVersionInformation$clusterVersion`), and one entry may carry several of
    // them comma-separated — the form the refusal above states.
    let rows: &[(&str, &str, bool)] = &[
        ("the default version of the seeded calendar", "clusterVersions=2.12", true),
        ("a version the calendar does not publish", "clusterVersions=0.1", true),
        ("a two-digit minor", "clusterVersions=1.31", true),
        ("a many-digit major and minor", "clusterVersions=123.456", true),
        ("two well-formed entries", "clusterVersions=2.11&clusterVersions=2.12", true),
        (
            "two versions comma-separated in one entry",
            "clusterVersions=2.11%2C2.12",
            true,
        ),
        ("a word", "clusterVersions=not-a-version", false),
        ("a major alone", "clusterVersions=1", false),
        ("a non-numeric minor", "clusterVersions=1.x", false),
        ("a patch component as well", "clusterVersions=1.31.2", false),
        ("an empty entry", "clusterVersions=", false),
        ("a trailing dot", "clusterVersions=1.", false),
        ("a leading dot", "clusterVersions=.31", false),
        ("whitespace around a version", "clusterVersions=%201.31", false),
        (
            "a word beside a version in one comma-separated entry",
            "clusterVersions=2.12%2Cnot-a-version",
            false,
        ),
        (
            "a malformed entry beside a well-formed one",
            "clusterVersions=2.12&clusterVersions=not-a-version",
            false,
        ),
    ];

    for (case, query, accepted) in rows {
        let answer = describe_versions(&router, query).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a cluster version is a filter, got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400 for a filter that is no cluster version, \
                 got {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                refusal,
                "case {case:?}: expected this filter's own refusal sentence, got {:?}",
                answer.message()
            );
        }
    }
}

/// Paging walks the filtered listing exactly once, whatever the page size, and the
/// last page carries no token.
#[tokio::test]
async fn pagination_walks_every_row_once() {
    let (router, engine) = emulator();
    let now = SystemTime::now();
    seed_calendar(&engine, clock_relative_calendar(now));

    // Paged with `includeAll`, so the table has three rows to walk: what this suite
    // is about is the paging, and an unfiltered listing offers only the two rows
    // still inside support.
    let whole = reported_versions(&describe_versions(&router, "includeAll=true").await);
    assert_eq!(
        whole,
        vec!["2.12", "2.11", "2.10"],
        "case whole listing: expected the three released rows, newest first, got {whole:?}"
    );

    for page_size in [1usize, 2, 3, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut pages = 0usize;
        let mut token: Option<String> = None;
        loop {
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case page size {page_size}: paging must terminate, walked {walked:?}"
            );
            let query = match &token {
                None => format!("includeAll=true&maxResults={page_size}"),
                Some(next) => {
                    format!("includeAll=true&maxResults={page_size}&nextToken={next}")
                }
            };
            let answer = describe_versions(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: expected HTTP 200, got {} ({:?})",
                answer.status, answer.body
            );
            let page = reported_versions(&answer);
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must not exceed maxResults, got {page:?}"
            );
            walked.extend(page);
            token = answer
                .body
                .get("nextToken")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case page size {page_size}: expected the whole listing once and in order, got \
             {walked:?}"
        );
    }

    // A page token is honoured only with the filters it was minted under: the row
    // it names is not in a differently filtered listing.
    let first = describe_versions(&router, "includeAll=true&maxResults=1").await;
    let token = first
        .body
        .get("nextToken")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("expected a nextToken on a 1-row page, got {:?}", first.body))
        .to_string();
    // A token that names a row the differently filtered listing does NOT hold: the
    // second page's token names `2.11`, which the `defaultOnly` listing (just the
    // default version, `2.12`) has no position for.
    let second = describe_versions(&router, "includeAll=true&maxResults=2").await;
    let foreign_filter_token = second
        .body
        .get("nextToken")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("expected a nextToken on a 2-row page, got {:?}", second.body))
        .to_string();
    let refused = [
        ("a token this service never minted", "nextToken=not-a-token".to_string()),
        (
            "a hex string that is not a token",
            "nextToken=deadbeefdeadbeef".to_string(),
        ),
        (
            "a well-formed token minted by another listing",
            format!("nextToken={}", state::encode_page_token("2.12")),
        ),
        (
            "a token used with different filters",
            format!("nextToken={foreign_filter_token}&defaultOnly=true"),
        ),
    ];
    for (case, query) in refused {
        let answer = describe_versions(&router, &query).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} ({:?})",
            answer.status, answer.body
        );
        assert!(
            answer
                .error_type
                .as_deref()
                .is_some_and(|kind| kind.contains("InvalidParameterException")),
            "case {case}: expected InvalidParameterException, got {:?}",
            answer.error_type
        );
    }

    // And the same token WITH the same filters resumes where the first page ended.
    let resumed = describe_versions(
        &router,
        &format!("includeAll=true&maxResults=1&nextToken={token}"),
    )
    .await;
    assert_eq!(
        reported_versions(&resumed),
        vec![whole[1].clone()],
        "case resume: expected the row after the first, got {:?}",
        resumed.body
    );

    // `maxResults` is modelled 1..100 (`smithy_query query="operation"
    // operation="DescribeClusterVersions"`), so a value outside it is refused with
    // a client error rather than paging nothing or faulting.
    for (case, value) in [
        ("zero", "0"),
        ("above the modelled maximum", "101"),
        ("negative", "-1"),
    ] {
        let answer = describe_versions(&router, &format!("maxResults={value}")).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {case}: expected HTTP 400, got {} ({:?})",
            answer.status, answer.body
        );
    }
}

/// A version whose extended support has ENDED is no longer one Amazon EKS offers:
/// an unfiltered listing reports only the versions still inside standard or
/// extended support, and every row it does report agrees with its own dates — so a
/// caller can hand any version it reads straight to `CreateCluster`. The row is
/// still PUBLISHED: a caller that asks for it, with `includeAll` or with a status
/// filter naming the unsupported state, gets it.
#[tokio::test]
async fn an_unfiltered_listing_offers_only_versions_still_in_support() {
    let (router, engine) = emulator();
    let now = SystemTime::now();

    // The rows differ along where the clock sits relative to a row's two
    // end-of-support dates, including BOTH published boundaries (a row whose
    // standard support ends exactly now, and one whose extended support does) and
    // one day either side of each: the rule is a comparison against those dates,
    // so the boundary is where it can be wrong.
    //
    // (version, end of standard support, end of extended support, offered
    //  unfiltered)
    let calendar: Vec<(&str, SystemTime, SystemTime, bool)> = vec![
        ("3.9", now - 400 * DAY, now - 300 * DAY, false),
        ("3.10", now - 400 * DAY, now - DAY, false),
        ("3.11", now - 400 * DAY, now, false),
        ("3.12", now - 400 * DAY, now + DAY, true),
        ("3.13", now - DAY, now + 300 * DAY, true),
        ("3.14", now, now + 300 * DAY, true),
        ("3.15", now + DAY, now + 300 * DAY, true),
        ("3.16", now + 100 * DAY, now + 500 * DAY, true),
    ];
    seed_calendar(
        &engine,
        calendar
            .iter()
            .map(|(version, standard, extended, _)| ClusterVersionRecord {
                cluster_version: (*version).to_string(),
                cluster_type: CLUSTER_TYPE_EKS.to_string(),
                default_version: false,
                release_date: now - 1000 * DAY,
                end_of_standard_support: *standard,
                end_of_extended_support: *extended,
            })
            .collect(),
    );

    let offered: Vec<String> = calendar
        .iter()
        .filter(|(_, _, _, offered)| *offered)
        .map(|(version, _, _, _)| (*version).to_string())
        .collect();

    // The listing holds whole and page by page: a filter that is applied per page
    // would otherwise let an out-of-support row through on a later page.
    for page_size in [None, Some(1), Some(2), Some(100)] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut parts: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                parts.push(format!("maxResults={size}"));
            }
            if let Some(next) = &token {
                parts.push(format!("nextToken={next}"));
            }
            let answer = describe_versions(&router, &parts.join("&")).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size:?}: expected HTTP 200, got {} ({:?})",
                answer.status, answer.body
            );
            // Every reported row agrees with its own dates, and none of them is
            // past its end of extended support.
            let clock = now_secs();
            for row in rows(&answer) {
                let case = row["clusterVersion"].as_str().unwrap_or_default().to_owned();
                let end_of_standard = epoch_secs(&row["endOfStandardSupportDate"]);
                let end_of_extended = epoch_secs(&row["endOfExtendedSupportDate"]);
                assert!(
                    clock < end_of_extended,
                    "case {case}: an unfiltered listing must not offer a version past extended \
                     support, got end of extended support {end_of_extended} at now {clock}"
                );
                let (version_status, status) =
                    expected_status(clock, end_of_standard, end_of_extended);
                assert_eq!(
                    row["versionStatus"].as_str(),
                    Some(version_status),
                    "case {case}: expected versionStatus {version_status:?} from its own dates, \
                     got {:?}",
                    row["versionStatus"]
                );
                assert_eq!(
                    row["status"].as_str(),
                    Some(status),
                    "case {case}: expected status {status:?} from its own dates, got {:?}",
                    row["status"]
                );
            }
            walked.extend(reported_versions(&answer));
            token = answer
                .body
                .get("nextToken")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, offered,
            "case page size {page_size:?}: expected only the versions still in support, got \
             {walked:?}"
        );
    }

    // Asked for, every row is reported — the out-of-support ones are hidden from
    // the default listing, not dropped from the calendar.
    let every_version: Vec<String> = calendar
        .iter()
        .map(|(version, _, _, _)| (*version).to_string())
        .collect();
    let out_of_support: Vec<String> = calendar
        .iter()
        .filter(|(_, _, _, offered)| !*offered)
        .map(|(version, _, _, _)| (*version).to_string())
        .collect();
    for (case, query, expected) in [
        ("includeAll", "includeAll=true", &every_version),
        (
            "the current status spelling naming the unsupported state",
            "versionStatus=UNSUPPORTED",
            &out_of_support,
        ),
        (
            "the deprecated status spelling naming it",
            "status=unsupported",
            &out_of_support,
        ),
    ] {
        let answer = describe_versions(&router, &format!("{query}&maxResults=100")).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} ({:?})",
            answer.status, answer.body
        );
        let actual = reported_versions(&answer);
        assert_eq!(
            &actual, expected,
            "case {case}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The same rule on the calendar AS SHIPPED, walked the way a client discovers
/// versions: every page of an unfiltered listing offers only versions still inside
/// standard or extended support, each row agrees with its own dates, and the rows
/// the support window hides are still reachable with `includeAll`.
///
/// The suites above install a calendar to put the clock in every support class at
/// once; this one leaves the shipped rows in place, so the table a caller actually
/// reads is held to the same rule and `includeAll` is shown to change the answer
/// against it rather than being inert.
#[tokio::test]
async fn the_shipped_calendar_hides_out_of_support_versions_until_asked() {
    let (router, _engine) = emulator();

    // The page size differs per case so the support window is proven to hold for
    // the listing as a WHOLE: a filter applied per page would let an out-of-support
    // row through on a later page.
    for page_size in [None, Some(1), Some(3), Some(100)] {
        let mut walked: Vec<String> = Vec::new();
        let mut defaults: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut parts: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                parts.push(format!("maxResults={size}"));
            }
            if let Some(next) = &token {
                parts.push(format!("nextToken={next}"));
            }
            let answer = describe_versions(&router, &parts.join("&")).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size:?}: expected HTTP 200, got {} ({:?})",
                answer.status, answer.body
            );
            let clock = now_secs();
            for row in rows(&answer) {
                let case = row["clusterVersion"]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected a clusterVersion string, got {row:?}"))
                    .to_string();
                let end_of_standard = epoch_secs(&row["endOfStandardSupportDate"]);
                let end_of_extended = epoch_secs(&row["endOfExtendedSupportDate"]);
                assert!(
                    clock < end_of_extended,
                    "case {case}: the shipped calendar must not offer a version past extended \
                     support, got end of extended support {end_of_extended} at now {clock}"
                );
                let (version_status, status) =
                    expected_status(clock, end_of_standard, end_of_extended);
                assert_eq!(
                    row["versionStatus"].as_str(),
                    Some(version_status),
                    "case {case}: expected versionStatus {version_status:?} from its own dates, \
                     got {:?}",
                    row["versionStatus"]
                );
                assert_eq!(
                    row["status"].as_str(),
                    Some(status),
                    "case {case}: expected status {status:?} from its own dates, got {:?}",
                    row["status"]
                );
                assert!(
                    row["kubernetesPatchVersion"]
                        .as_str()
                        .is_some_and(|patch| patch.starts_with(&format!("{case}."))),
                    "case {case}: expected a kubernetesPatchVersion of this minor, got {:?}",
                    row["kubernetesPatchVersion"]
                );
                if row["defaultVersion"].as_bool() == Some(true) {
                    defaults.push(case.clone());
                }
                walked.push(case);
            }
            token = answer
                .body
                .get("nextToken")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if token.is_none() {
                break;
            }
        }

        let mut distinct = walked.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            walked.len(),
            "case page size {page_size:?}: expected every version once, got {walked:?}"
        );
        // Version discovery is the first call of a provisioning flow, so the
        // default listing has to keep offering a choice of versions as the clock
        // advances, not decay into one row or none.
        assert!(
            walked.len() >= 2,
            "case page size {page_size:?}: expected at least two versions still in support, got \
             {walked:?}"
        );
        assert_eq!(
            defaults,
            vec![DEFAULT_KUBERNETES_VERSION.to_string()],
            "case page size {page_size:?}: expected exactly the version CreateCluster defaults \
             to, got {defaults:?}"
        );
        let keys: Vec<(u32, u32)> = walked.iter().map(|version| version_key(version)).collect();
        assert!(
            keys.windows(2).all(|pair| pair[0] > pair[1]),
            "case page size {page_size:?}: expected the newest Kubernetes version first, got \
             {walked:?}"
        );
    }

    // `includeAll` is what reaches the rows the support window hides: it is a
    // superset of the default listing, and every row it adds is one the clock has
    // carried past its own end of extended support.
    let unfiltered = reported_versions(&describe_versions(&router, "maxResults=100").await);
    let widened = describe_versions(&router, "includeAll=true&maxResults=100").await;
    let every_version = reported_versions(&widened);
    assert!(
        unfiltered
            .iter()
            .all(|version| every_version.contains(version)),
        "case includeAll: expected a superset of the default listing {unfiltered:?}, got \
         {every_version:?}"
    );
    let clock = now_secs();
    for row in rows(&widened) {
        let case = row["clusterVersion"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a clusterVersion string, got {row:?}"))
            .to_string();
        if unfiltered.contains(&case) {
            continue;
        }
        let end_of_extended = epoch_secs(&row["endOfExtendedSupportDate"]);
        assert!(
            clock >= end_of_extended,
            "case {case}: includeAll must add only versions past extended support, got end of \
             extended support {end_of_extended} at now {clock}"
        );
    }
}
