// SPDX-License-Identifier: CC0-1.0

//! `DescribeRegions` end-to-end: the Regions an account can see, and the selectors that
//! narrow them.
//!
//! Every call goes over the wire the way a client makes it — an
//! `application/x-www-form-urlencoded` ec2Query POST dispatched through the same
//! `MultiServiceRouter` the emulator serves with, carrying the Region in the SigV4
//! `Credential=` scope of its `Authorization` header, which is where the service reads
//! the Region (and so the partition) it is being asked about.
//!
//! The rules under test:
//!
//! 1. A request reports every Region of the PARTITION it was signed for, each with its
//!    Region name, its EC2 service endpoint (`ec2.<region>.<partition dns suffix>`) and
//!    its opt-in status; a Region the account must opt in to is reported only when the
//!    request asks for it.
//! 2. `AllRegions` adds "Regions that are disabled for your account".
//! 3. `RegionNames` reports the Regions it names, "whether they are enabled and disabled
//!    for your account"; a name that is no Region of the partition is refused with
//!    `InvalidParameterValue`.
//! 4. `Filters` are ANDed with one another and ORed over each filter's values, with EC2's
//!    `*` / `?` wildcards; a filter the operation does not document is refused, and the
//!    ceilings on one request's filtering work are enforced at the factor, not the product.
//! 5. `DryRun` performs nothing and answers 412 `DryRunOperation`.
//! 6. The describe is a read: repeating it answers the same Regions.
//!
//! Region ORDER is deliberately not asserted — "the order of the elements in the
//! response ... might vary" — so every comparison here is over a sorted set.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::state::{
    MAX_FILTERS, MAX_FILTER_VALUES, MAX_FILTER_VALUE_BYTES, MAX_RESOURCE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, filters, indexed, unescaped};

/// The Regions of the commercial partition every account can use WITHOUT opting in: the
/// "Regions that are enabled by default" table of the Amazon Web Services Account
/// Management Reference Guide, "Specify which Amazon Web Services Regions your account
/// can use" — the guide `DescribeRegions`'s own documentation links to. Written out here
/// so the handler's own list cannot drift from the guide's.
const ENABLED_BY_DEFAULT: [&str; 17] = [
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ca-central-1",
    "eu-central-1",
    "eu-north-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
];

/// Commercial Regions that are NOT in the guide's enabled-by-default table, so an account
/// has to opt in to each — a sample of the catalog this emulator serves
/// (`emulator_telemetry::policy::KNOWN_REGIONS`), none of them named by the issue this
/// suite was written for.
const OPT_IN_SAMPLE: [&str; 6] = [
    "af-south-1",
    "ap-east-1",
    "ca-west-1",
    "eu-south-2",
    "il-central-1",
    "mx-central-1",
];

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. This
/// operation reports facts of a partition and stores nothing, so the engine handle it
/// hands back is not needed here.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `DescribeRegions` call signed for `region`, with `params` as extra form fields.
/// Answers the HTTP status and the response body.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeRegions", params).await
}

/// One Region of a `DescribeRegions` response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RegionItem {
    name: String,
    endpoint: String,
    opt_in_status: String,
    /// Element names the response carried for this Region, for the absent-member checks.
    elements: Vec<String>,
}

/// Every Region in an ec2Query `DescribeRegions` body, sorted.
///
/// Each Region is one `<item>` of the `<regionInfo>` list, and a Region's own
/// `<geographySet>` holds `<item>` elements of its own, so the reading goes through
/// `roxmltree` — a Region is an element that has a `<regionName>` child — rather than
/// splitting the text on an opening tag.
fn regions(case: &str, body: &str) -> Vec<RegionItem> {
    let parsed = super::wire::document(case, body);
    let mut found: Vec<RegionItem> = parsed
        .descendants()
        .filter(|node| {
            node.is_element() && super::wire::child(*node, "regionName").is_some()
        })
        .map(|node| {
            let member = |name: &str| {
                super::wire::child_text(node, name)
                    .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in a Region"))
            };
            RegionItem {
                name: member("regionName"),
                // `Region$Endpoint` is on the wire as `regionEndpoint`.
                endpoint: member("regionEndpoint"),
                opt_in_status: member("optInStatus"),
                elements: node
                    .descendants()
                    .filter(|inner| inner.is_element() && *inner != node)
                    .map(|inner| inner.tag_name().name().to_string())
                    .collect(),
            }
        })
        .collect();
    found.sort();
    found
}

/// The Region names in a response, sorted.
fn region_names(case: &str, body: &str) -> Vec<String> {
    regions(case, body)
        .into_iter()
        .map(|region| region.name)
        .collect()
}

/// `vec!["a".to_string(), …]` for a slice of string literals, sorted.
fn sorted(values: &[&str]) -> Vec<String> {
    let mut owned: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    owned.sort();
    owned
}

/// One row of a selector table: the case name, the `RegionNames` the request carries, and
/// the Region names expected back.
type SelectorCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

/// One row of a filter table: the case name, the extra form fields, the `Filters` the
/// request carries (each a name and its values), and the Region names expected back.
type FilterCase<'a> = (
    &'a str,
    &'a [(&'a str, &'a str)],
    &'a [(&'a str, &'a [&'a str])],
    &'a [&'a str],
);

// ---------------------------------------------------------------------------
// Rule 1: a request reports the Regions of its own partition
// ---------------------------------------------------------------------------

/// A commercial-partition call reports exactly the Regions that are enabled by default,
/// each `opt-in-not-required`, with the commercial endpoint and no invented geography.
#[tokio::test]
async fn a_commercial_call_reports_the_regions_enabled_by_default() {
    let router = emulator();
    // The rows differ in which enabled-by-default Region the call is signed for, which is
    // what the opt-in status of the rest is read against.
    for signed in ["us-east-1", "eu-west-3", "ap-northeast-3"] {
        let (status, body) = describe(&router, signed, &[]).await;
        assert_eq!(
            status, 200,
            "case {signed:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            region_names(signed, &body),
            sorted(&ENABLED_BY_DEFAULT),
            "case {signed:?}: expected the Regions enabled by default, got {body}"
        );
        for region in regions(signed, &body) {
            assert_eq!(
                region.endpoint,
                format!("ec2.{}.amazonaws.com", region.name),
                "case {signed:?}: expected the Region's own EC2 endpoint, got {region:?}"
            );
            assert_eq!(
                region.opt_in_status, "opt-in-not-required",
                "case {signed:?}: a Region enabled by default needs no opt-in, got {region:?}"
            );
            // A Region reports its status, its geography, its name and its endpoint and
            // nothing else — asserted over every element name the item carries, so the
            // check cannot pass just because a list serialises under a name this test did
            // not guess. The geography's own `<item>`/`<name>` elements are dropped: the
            // geography's VALUE is asserted in `read_path_modeled_members.rs`, over every Region.
            let members: Vec<String> = region
                .elements
                .iter()
                .filter(|tag| *tag != "item" && *tag != "name")
                .cloned()
                .collect();
            assert_eq!(
                members,
                vec![
                    "optInStatus".to_string(),
                    "geographySet".to_string(),
                    "regionName".to_string(),
                    "regionEndpoint".to_string(),
                ],
                "case {signed:?}: a Region reports only its status, geography, name and \
                 endpoint, got {region:?}"
            );
        }
    }
}

/// A call signed for a Region of another partition reports THAT partition's Regions, with
/// that partition's endpoint DNS suffix.
///
/// The rows are the non-commercial partitions this emulator's Region catalog
/// (`emulator_telemetry::policy::KNOWN_REGIONS`) holds Regions of, each with the
/// `dnsSuffix` the AWS SDK partition metadata publishes for it. A partition's own Regions
/// need no opt-in: an account of that partition reaches them directly.
#[tokio::test]
async fn a_call_reports_the_regions_of_its_own_partition() {
    let router = emulator();
    let cases: [(&str, &str, &[&str]); 6] = [
        (
            "cn-north-1",
            "amazonaws.com.cn",
            &["cn-north-1", "cn-northwest-1"],
        ),
        (
            "us-gov-west-1",
            "amazonaws.com",
            &["us-gov-east-1", "us-gov-west-1"],
        ),
        (
            "us-iso-east-1",
            "c2s.ic.gov",
            &["us-iso-east-1", "us-iso-west-1"],
        ),
        ("us-isob-east-1", "sc2s.sgov.gov", &["us-isob-east-1"]),
        (
            "us-isof-south-1",
            "csp.hci.ic.gov",
            &["us-isof-east-1", "us-isof-south-1"],
        ),
        ("eu-isoe-west-1", "cloud.adc-e.uk", &["eu-isoe-west-1"]),
    ];

    for (signed, suffix, expected) in cases {
        let (status, body) = describe(&router, signed, &[]).await;
        assert_eq!(
            status, 200,
            "case {signed:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            region_names(signed, &body),
            sorted(expected),
            "case {signed:?}: expected its own partition's Regions, got {body}"
        );
        for region in regions(signed, &body) {
            assert_eq!(
                region.endpoint,
                format!("ec2.{}.{suffix}", region.name),
                "case {signed:?}: expected the partition's {suffix} endpoint, got {region:?}"
            );
            assert_eq!(
                region.opt_in_status, "opt-in-not-required",
                "case {signed:?}: a Region of the caller's own partition needs no opt-in, got {region:?}"
            );
        }
    }
}

/// A Region that requires an opt-in this account has not made serves it NOTHING: a request
/// signed for one is refused `AuthFailure` rather than answered with an empty result, so a
/// client can tell "this Region is not available to me" from "this Region is empty".
///
/// The rows are opt-in Regions of different geographies, none of them the Region any other
/// case here signs for, and each is probed through two different operations so the refusal
/// belongs to the endpoint rather than to one handler.
#[tokio::test]
async fn a_region_the_account_has_not_opted_in_to_refuses_every_request() {
    let router = emulator();
    for signed in OPT_IN_SAMPLE {
        for action in ["DescribeRegions", "DescribeInstances"] {
            let (status, body) =
                super::ec2_query::call(&router, signed, action, &[]).await;
            assert_eq!(
                status, 400,
                "case {signed:?}/{action}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "AuthFailure",
                "case {signed:?}/{action}: expected AuthFailure, got {body}"
            );
            assert!(
                unescaped(&body).contains(ec2::state::REGION_NOT_ENABLED_MESSAGE),
                "case {signed:?}/{action}: expected the credentials sentence, got {body}"
            );
        }
    }
}

/// A Region of the enabled-by-default table, and one of another partition, are served — the
/// refusal above is about the opt-in, not about every Region that is not `us-east-1`.
#[tokio::test]
async fn an_enabled_region_is_served() {
    let router = emulator();
    for signed in ["us-east-1", "ap-northeast-3", "us-gov-west-1", "cn-north-1"] {
        let (status, body) = describe(&router, signed, &[]).await;
        assert_eq!(
            status, 200,
            "case {signed:?}: expected 200, got {status} {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: AllRegions
// ---------------------------------------------------------------------------

/// `AllRegions` adds the Regions that are disabled for the account, each reported
/// `not-opted-in`; without it they are left out, and `AllRegions=false` is the same as
/// leaving it out.
#[tokio::test]
async fn all_regions_adds_the_disabled_regions() {
    let router = emulator();
    let cases: [(&str, &[(&str, &str)]); 3] = [
        ("absent", &[]),
        ("AllRegions=false", &[("AllRegions", "false")]),
        ("AllRegions=true", &[("AllRegions", "true")]),
    ];

    let mut by_case: Vec<(&str, Vec<RegionItem>)> = Vec::new();
    for (case, params) in cases {
        let params: Vec<(String, String)> = params
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        by_case.push((case, regions(case, &body)));
    }

    let enabled = sorted(&ENABLED_BY_DEFAULT);
    for (case, found) in &by_case[..2] {
        let names: Vec<String> = found.iter().map(|region| region.name.clone()).collect();
        assert_eq!(
            names, enabled,
            "case {case:?}: a listing that does not ask for all Regions reports only the enabled ones, got {found:?}"
        );
    }

    let (case, all) = &by_case[2];
    let names: Vec<String> = all.iter().map(|region| region.name.clone()).collect();
    assert!(
        names.len() > enabled.len(),
        "case {case:?}: AllRegions must add the disabled Regions, got {names:?}"
    );
    for region in &enabled {
        assert!(
            names.contains(region),
            "case {case:?}: AllRegions still reports {region}, got {names:?}"
        );
    }
    for disabled in OPT_IN_SAMPLE {
        let found = all
            .iter()
            .find(|region| region.name == disabled)
            .unwrap_or_else(|| panic!("case {case:?}: expected {disabled} among {names:?}"));
        assert_eq!(
            found.opt_in_status, "not-opted-in",
            "case {case:?}: a Region the account has not opted in to is not-opted-in, got {found:?}"
        );
        assert_eq!(
            found.endpoint,
            format!("ec2.{disabled}.amazonaws.com"),
            "case {case:?}: a disabled Region still reports its endpoint, got {found:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: RegionNames
// ---------------------------------------------------------------------------

/// A Region named by `RegionNames` is reported, whatever its opt-in status; naming none
/// reports the enabled ones.
///
/// The rows differ in how many Regions are named, whether they are enabled or disabled
/// for the account, whether a name is repeated, and whether `AllRegions` is set alongside.
#[tokio::test]
async fn region_names_select_the_regions_they_name() {
    let router = emulator();
    let cases: [SelectorCase; 6] = [
        ("one enabled Region", &["eu-west-1"], &["eu-west-1"]),
        (
            "two enabled Regions",
            &["us-west-2", "ap-south-1"],
            &["ap-south-1", "us-west-2"],
        ),
        // "You can specify any Regions, whether they are enabled and disabled for your
        // account" — so a disabled Region is reported without `AllRegions`.
        ("one disabled Region", &["eu-south-1"], &["eu-south-1"]),
        (
            "an enabled and a disabled Region",
            &["me-south-1", "eu-central-1"],
            &["eu-central-1", "me-south-1"],
        ),
        (
            "a repeated name is one Region",
            &["sa-east-1", "sa-east-1"],
            &["sa-east-1"],
        ),
        (
            "every opt-in Region of the sample",
            &OPT_IN_SAMPLE,
            &OPT_IN_SAMPLE,
        ),
    ];

    for (case, named, expected) in cases {
        let (status, body) = describe(&router, "us-east-1", &indexed("RegionName", named)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            region_names(case, &body),
            sorted(expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// A `RegionNames` entry that is no Region of the caller's partition is refused with
/// `InvalidParameterValue` naming the value at fault — not answered with an empty list.
///
/// The rows differ in how the name is wrong: a Region code no Region carries, a Region of
/// ANOTHER partition, a wrongly-cased name, a Region name with a stray suffix, a known
/// Region beside an unknown one, and an empty name.
#[tokio::test]
async fn an_unknown_region_name_is_refused() {
    let router = emulator();
    let cases: [(&str, &[&str], &str); 6] = [
        ("no such Region", &["us-east-9"], "us-east-9"),
        ("another partition's Region", &["cn-north-1"], "cn-north-1"),
        ("wrong case", &["US-EAST-1"], "US-EAST-1"),
        ("stray suffix", &["us-east-1a"], "us-east-1a"),
        (
            "a known Region beside an unknown one",
            &["us-east-1", "eu-west-9"],
            "eu-west-9",
        ),
        ("empty name", &[""], ""),
    ];

    for (case, named, offending) in cases {
        let (status, body) = describe(&router, "us-east-1", &indexed("RegionName", named)).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(offending),
            "case {case:?}: the message must name {offending:?}, got {message:?}"
        );
        assert!(
            element(&body, "regionInfo").is_none(),
            "case {case:?}: a refused request reports no Regions, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: Filters
// ---------------------------------------------------------------------------

/// Filters are ANDed with one another, each filter's values are ORed, and a value may
/// carry EC2's `*` / `?` wildcards or escape them with a backslash.
///
/// The rows walk every filter the operation documents, each filter's value forms (exact,
/// multi-value, wildcarded, escaped), the interaction with `AllRegions`, and two filters
/// at once — matching and contradictory.
#[tokio::test]
async fn filters_are_anded_and_their_values_ored() {
    let router = emulator();
    let all_regions: &[(&str, &str)] = &[("AllRegions", "true")];
    let cases: [FilterCase; 15] = [
        (
            "region-name exact",
            &[],
            &[("region-name", &["eu-west-2"])],
            &["eu-west-2"],
        ),
        (
            "region-name two values are ORed",
            &[],
            &[("region-name", &["eu-west-2", "us-west-1"])],
            &["eu-west-2", "us-west-1"],
        ),
        (
            "region-name wildcard",
            &[],
            &[("region-name", &["eu-west-*"])],
            &["eu-west-1", "eu-west-2", "eu-west-3"],
        ),
        (
            "region-name ? is one character",
            &[],
            &[("region-name", &["ap-northeast-?"])],
            &["ap-northeast-1", "ap-northeast-2", "ap-northeast-3"],
        ),
        (
            "an escaped * is a literal *",
            &[],
            &[("region-name", &["eu-west-\\*"])],
            &[],
        ),
        (
            "region-name no match",
            &[],
            &[("region-name", &["eu-west-9"])],
            &[],
        ),
        (
            "endpoint exact",
            &[],
            &[("endpoint", &["ec2.ca-central-1.amazonaws.com"])],
            &["ca-central-1"],
        ),
        (
            "endpoint wildcard",
            &[],
            &[("endpoint", &["ec2.sa-*.amazonaws.com"])],
            &["sa-east-1"],
        ),
        (
            "endpoint of another partition",
            &[],
            &[("endpoint", &["ec2.cn-north-1.amazonaws.com.cn"])],
            &[],
        ),
        (
            "opt-in-status of the enabled Regions",
            &[],
            &[("opt-in-status", &["opt-in-not-required"])],
            &ENABLED_BY_DEFAULT,
        ),
        // A disabled Region is not in a listing that does not ask for all Regions, so
        // filtering for its status matches nothing until `AllRegions` puts it there.
        (
            "opt-in-status not-opted-in without AllRegions",
            &[],
            &[("opt-in-status", &["not-opted-in"])],
            &[],
        ),
        (
            "opt-in-status not-opted-in with AllRegions",
            all_regions,
            &[
                ("opt-in-status", &["not-opted-in"]),
                ("region-name", &OPT_IN_SAMPLE),
            ],
            &OPT_IN_SAMPLE,
        ),
        (
            "two filters are ANDed",
            &[],
            &[
                ("opt-in-status", &["opt-in-not-required"]),
                ("region-name", &["us-east-2"]),
            ],
            &["us-east-2"],
        ),
        (
            "contradictory filters match nothing",
            &[],
            &[
                ("region-name", &["us-east-1"]),
                ("endpoint", &["ec2.us-east-2.amazonaws.com"]),
            ],
            &[],
        ),
        // A documented filter carrying no value at all is still a valid filter, so the
        // request is answered — and it matches nothing, there being no value for the
        // attribute to match.
        (
            "a filter with no values matches nothing",
            &[],
            &[("region-name", &[])],
            &[],
        ),
    ];

    for (case, extra, entries, expected) in cases {
        let mut params: Vec<(String, String)> = extra
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        params.extend(filters(entries));
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            region_names(case, &body),
            sorted(expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// Every filter the operation's `Filters` doc lists is accepted; the refusal below is only
/// for a name outside that list.
///
/// The list is the one in
/// `smithy_query(query="doc",shape="DescribeRegionsRequest",member="Filters")`, written
/// out here so the handler's own list cannot drift from the model's doc.
#[tokio::test]
async fn every_documented_filter_name_is_accepted() {
    let router = emulator();
    let documented = ["endpoint", "opt-in-status", "region-name"];

    for name in documented {
        let (status, body) = describe(
            &router,
            "us-east-1",
            &filters(&[(
                name,
                &[
                    "us-east-1",
                    "opt-in-not-required",
                    "ec2.us-east-1.amazonaws.com",
                ],
            )]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a documented filter must be accepted, got {status} {body}"
        );
        assert!(
            region_names(name, &body).contains(&"us-east-1".to_string()),
            "case {name:?}: one of the values names us-east-1, so it must be reported, got {body}"
        );
    }
}

/// A filter name this operation does not document is refused with `InvalidParameterValue`
/// naming it, rather than silently ignored.
///
/// The rows differ in how the name is wrong: a plural near-miss, a member-cased name, a
/// filter of another describe, a zone filter, and no name at all.
#[tokio::test]
async fn an_unknown_filter_name_is_refused() {
    let router = emulator();
    let cases: [(&str, &str); 5] = [
        ("plural near-miss", "region-names"),
        ("member casing", "RegionName"),
        ("another describe's filter", "instance-type"),
        ("a zone filter", "zone-name"),
        ("empty name", ""),
    ];

    for (case, name) in cases {
        let (status, body) = describe(&router, "us-east-1", &filters(&[(name, &["us-east-1"])])).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(&format!("'{name}'")),
            "case {case:?}: the message must name the filter {name:?}, got {message:?}"
        );
        assert!(
            element(&body, "regionInfo").is_none(),
            "case {case:?}: a refused request reports no Regions, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4b: the ceilings on one request's filtering work
// ---------------------------------------------------------------------------

/// Matching cost is the PRODUCT of how many filters a request carries, how many values
/// each carries, how long a value is, how many Regions it names and how many Regions the
/// catalog holds, and the model bounds none of them (`FilterList`, `Filter$Values` and
/// `RegionNameStringList` carry no `@length`). So the handler applies this crate's own
/// ceilings, the same ones the other filtered describes enforce: a request AT each ceiling
/// is answered.
///
/// Each row is a distinct factor of that product, named by the constant it is derived from
/// rather than by a literal.
#[tokio::test]
async fn a_request_at_each_filtering_ceiling_is_answered() {
    let router = emulator();

    // `MAX_FILTERS` filters, each selecting everything, so the AND of them all is still
    // every enabled Region: the ceiling itself is accepted and nothing is lost.
    let at_filter_ceiling: Vec<(&str, &[&str])> =
        (0..MAX_FILTERS).map(|_| ("region-name", &["*"][..])).collect();
    let (status, body) = describe(&router, "us-east-1", &filters(&at_filter_ceiling)).await;
    assert_eq!(
        status, 200,
        "case \"MAX_FILTERS filters\": expected 200, got {status} {body}"
    );
    assert_eq!(
        region_names("MAX_FILTERS filters", &body),
        sorted(&ENABLED_BY_DEFAULT),
        "case \"MAX_FILTERS filters\": expected every enabled Region, got {body}"
    );

    let many_values: Vec<String> = (0..MAX_FILTER_VALUES)
        .map(|index| format!("us-east-{index}"))
        .collect();
    let many_values: Vec<&str> = many_values.iter().map(String::as_str).collect();
    let long_value = "a".repeat(MAX_FILTER_VALUE_BYTES);
    let many_names = vec!["us-east-1"; MAX_RESOURCE_IDS];
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "MAX_FILTER_VALUES values in one filter",
            filters(&[("region-name", &many_values)]),
        ),
        (
            "a value of MAX_FILTER_VALUE_BYTES bytes",
            filters(&[("region-name", &[long_value.as_str()])]),
        ),
        (
            "MAX_RESOURCE_IDS Region names",
            indexed("RegionName", &many_names),
        ),
    ];
    for (case, params) in cases {
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 at the ceiling, got {status} {body}"
        );
    }
}

/// One step past any ceiling is refused with `InvalidParameterValue` and reports no
/// Region: the request asked for more work than one call will do, and is told so rather
/// than served a partial answer.
#[tokio::test]
async fn a_request_one_step_past_a_filtering_ceiling_is_refused() {
    let router = emulator();

    let past_filter_ceiling: Vec<(&str, &[&str])> = (0..=MAX_FILTERS)
        .map(|_| ("region-name", &["*"][..]))
        .collect();
    let past_value_ceiling: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|index| format!("us-east-{index}"))
        .collect();
    let past_value_ceiling: Vec<&str> = past_value_ceiling.iter().map(String::as_str).collect();
    let past_byte_ceiling = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);
    let past_name_ceiling = vec!["us-east-1"; MAX_RESOURCE_IDS + 1];

    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("one filter past MAX_FILTERS", filters(&past_filter_ceiling)),
        (
            "one value past MAX_FILTER_VALUES",
            filters(&[("region-name", &past_value_ceiling)]),
        ),
        (
            "one byte past MAX_FILTER_VALUE_BYTES",
            filters(&[("region-name", &[past_byte_ceiling.as_str()])]),
        ),
        (
            "one name past MAX_RESOURCE_IDS",
            indexed("RegionName", &past_name_ceiling),
        ),
    ];
    for (case, params) in cases {
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert!(
            element(&body, "regionInfo").is_none(),
            "case {case:?}: a refused request reports no Regions, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: DryRun
// ---------------------------------------------------------------------------

/// A `DryRun` request performs nothing and is answered 412 `DryRunOperation`, and the same
/// call without it still answers the account's Regions.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation() {
    let router = emulator();
    for case in ["DryRun alone", "DryRun beside a filter and AllRegions"] {
        let mut params = vec![("DryRun".to_string(), "true".to_string())];
        if case.contains("filter") {
            params.push(("AllRegions".to_string(), "true".to_string()));
            params.extend(filters(&[("region-name", &["us-east-1"])]));
        }
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(status, 412, "case {case:?}: expected 412, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            element(&body, "regionInfo").is_none(),
            "case {case:?}: a dry run reports no Regions, got {body}"
        );
    }

    // `DryRun=false` is not a dry run: the request is performed.
    let (status, body) = describe(
        &router,
        "us-east-1",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"DryRun=false\": expected 200, got {status} {body}"
    );
    assert_eq!(
        region_names("DryRun=false", &body),
        sorted(&ENABLED_BY_DEFAULT),
        "case \"DryRun=false\": expected the enabled Regions, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the describe is a read
// ---------------------------------------------------------------------------

/// Describing Regions changes nothing a later caller sees: repeated calls — including ones
/// that name, filter, refuse or dry-run in between — answer the same Regions.
#[tokio::test]
async fn describing_regions_does_not_change_what_a_later_caller_sees() {
    let router = emulator();

    let (_, first) = describe(&router, "us-east-1", &[]).await;
    assert_eq!(
        region_names("first", &first),
        sorted(&ENABLED_BY_DEFAULT),
        "case \"first\": expected the enabled Regions, got {first}"
    );

    // A named read, a filtered read, an all-Regions read, a refused read and a dry run,
    // all between the two full reads.
    let _ = describe(&router, "us-east-1", &indexed("RegionName", &["eu-south-1"])).await;
    let _ = describe(
        &router,
        "us-east-1",
        &filters(&[("region-name", &["us-west-*"])]),
    )
    .await;
    let _ = describe(
        &router,
        "us-east-1",
        &[("AllRegions".to_string(), "true".to_string())],
    )
    .await;
    let _ = describe(&router, "us-east-1", &indexed("RegionName", &["us-east-9"])).await;
    let _ = describe(
        &router,
        "us-east-1",
        &[("DryRun".to_string(), "true".to_string())],
    )
    .await;
    // …and a read signed for another Region, which must not leak into this one.
    let _ = describe(&router, "af-south-1", &[]).await;

    let (status, second) = describe(&router, "us-east-1", &[]).await;
    assert_eq!(
        status, 200,
        "case \"second\": expected 200, got {status} {second}"
    );
    assert_eq!(
        regions("second", &second),
        regions("first", &first),
        "case \"second\": a read must not change what a later caller sees"
    );
}
