// SPDX-License-Identifier: CC0-1.0

//! The cache settings a Lightsail distribution is configured with, driven over
//! the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rules under test, both stated on the members of the modelled
//! `CacheSettings` and neither expressible by the shapes they target:
//!
//! * `allowedHTTPMethods` and `cachedHTTPMethods` are free-form
//!   `NonEmptyString`s whose domain is a closed list of option strings the
//!   member documents — three method sets for the methods a distribution
//!   forwards to its origin, two for the method responses it caches — so a
//!   string outside its member's list configures a method set the service does
//!   not offer;
//! * `minimumTTL` has to be `0` when the settings forward all headers to the
//!   origin ("A value of `0` must be specified for `minimumTTL` if the
//!   distribution is configured to forward all headers to the origin").
//!
//! Both operations that take the object from a request answer to them — the
//! update replaces the whole object, so what it carries is what the
//! distribution would serve under — and a refused request changes nothing;
//! settings that satisfy both rules are stored and read back as sent.

use serde_json::{json, Value};

// The harness, the wire error code, the instance stand-up and the distribution
// reads this test target already exposes: a sibling suite drives the service
// through the same front door rather than standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{distribution, wait_for_distribution_status};
use crate::instance_lifecycle::{error_code, Emulator};

/// The one Region a distribution lives in and its API is served from, from the
/// modelled `LightsailDistribution$location` documentation ("all distributions
/// are located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The `status` a distribution settles into once a configuration has rolled out.
const DEPLOYED: &str = "Deployed";

/// The instance every distribution here pulls content from.
const ORIGIN_INSTANCE: &str = "cache-settings-origin";

/// The distribution the update cases apply their settings to.
const DISTRIBUTION: &str = "cache-settings-cdn";

/// The bundle every distribution here is created on, an id of the service's
/// published CDN plans (`GetDistributionBundles`).
const BUNDLE_ID: &str = "small_1_0";

/// The value Lightsail puts in a not-found body's `code` member.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// Every method set `allowedHTTPMethods` documents, and every one
/// `cachedHTTPMethods` documents — written out here independently of the
/// service's own tables, so these rows pin the documented domains rather than
/// restating whatever the code happens to hold.
///
/// SOURCE: the two members of `CacheSettings` —
/// `smithy_query(query="shape", shape="com.amazonaws.lightsail#CacheSettings")`:
/// the methods a distribution "processes and forwards to the distribution's
/// origin" and "the HTTP method responses that are cached by your
/// distribution".
const ALLOWED_METHOD_SETS: &[&str] = &[
    "GET,HEAD",
    "GET,HEAD,OPTIONS",
    "GET,HEAD,OPTIONS,PUT,PATCH,POST,DELETE",
];
/// See [`ALLOWED_METHOD_SETS`].
const CACHED_METHOD_SETS: &[&str] = &["GET,HEAD", "GET,HEAD,OPTIONS"];

/// Every member of the modelled `ForwardValues` enum
/// (`smithy_query(query="shape", shape="com.amazonaws.lightsail#ForwardValues")`),
/// which is the domain of `HeaderObject$option`: `all` forwards every header to
/// the origin, `none` only the default headers and `allow-list` only the ones
/// `headersAllowList` names.
const FORWARD_ALL: &str = "all";
/// See [`FORWARD_ALL`].
const FORWARD_NONE: &str = "none";
/// See [`FORWARD_ALL`].
const FORWARD_ALLOW_LIST: &str = "allow-list";

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// What a row's settings break, when they break one of the two rules.
#[derive(Debug, Clone, Copy)]
enum Offends {
    /// A method-set member — `allowedHTTPMethods` or `cachedHTTPMethods` —
    /// carrying a string outside that member's documented options.
    MethodSet {
        member: &'static str,
        value: &'static str,
    },
    /// A `minimumTTL` other than zero beside settings that forward all headers
    /// to the origin.
    MinimumTtl(i64),
}

/// One `cacheBehaviorSettings` object, and whether the rules admit it.
///
/// The rows differ along everything the rules generalise over: which member
/// carries the method set, each documented option of each member, the ways a
/// string can be outside those options (a single method, a set the OTHER member
/// documents, a longer set, a shorter one, reordered methods, lower case,
/// spaces after the commas, an empty string), each member of the forwarding
/// option domain, whether a forwarding option is carried at all, and the size
/// of the minimum TTL beside the option that forwards every header (absent,
/// zero, the smallest non-zero value, a day).
struct Row {
    case: &'static str,
    /// The `cacheBehaviorSettings` the request carries.
    settings: fn() -> Value,
    /// The rule the settings break, or `None` when they break neither — so a
    /// refusal has to name exactly what is wrong.
    offends: Option<Offends>,
}

/// The table every case below walks.
const ROWS: &[Row] = &[
    // --- the method-set members, each documented option of each ---
    Row {
        case: "the smallest method set both members document",
        settings: || json!({"allowedHTTPMethods": "GET,HEAD", "cachedHTTPMethods": "GET,HEAD"}),
        offends: None,
    },
    Row {
        case: "the three-method set both members document",
        settings: || {
            json!({
                "allowedHTTPMethods": "GET,HEAD,OPTIONS",
                "cachedHTTPMethods": "GET,HEAD,OPTIONS",
            })
        },
        offends: None,
    },
    Row {
        case: "the whole forwarding set, with the cached set at its smallest",
        settings: || {
            json!({
                "allowedHTTPMethods": "GET,HEAD,OPTIONS,PUT,PATCH,POST,DELETE",
                "cachedHTTPMethods": "GET,HEAD",
            })
        },
        offends: None,
    },
    Row {
        case: "neither method-set member carried at all",
        settings: || json!({"defaultTTL": 86_400}),
        offends: None,
    },
    Row {
        case: "an empty settings object",
        settings: || json!({}),
        offends: None,
    },
    Row {
        case: "a single method the forwarding set does not document",
        settings: || json!({"allowedHTTPMethods": "DELETE"}),
        offends: Some(Offends::MethodSet {
            member: "allowedHTTPMethods",
            value: "DELETE",
        }),
    },
    Row {
        case: "a four-method forwarding set between the documented ones",
        settings: || json!({"allowedHTTPMethods": "GET,HEAD,OPTIONS,PUT"}),
        offends: Some(Offends::MethodSet {
            member: "allowedHTTPMethods",
            value: "GET,HEAD,OPTIONS,PUT",
        }),
    },
    Row {
        case: "the documented methods reordered",
        settings: || json!({"allowedHTTPMethods": "HEAD,GET"}),
        offends: Some(Offends::MethodSet {
            member: "allowedHTTPMethods",
            value: "HEAD,GET",
        }),
    },
    Row {
        case: "a documented set in lower case",
        settings: || json!({"allowedHTTPMethods": "get,head"}),
        offends: Some(Offends::MethodSet {
            member: "allowedHTTPMethods",
            value: "get,head",
        }),
    },
    Row {
        case: "a documented set with a space after the comma",
        settings: || json!({"allowedHTTPMethods": "GET, HEAD"}),
        offends: Some(Offends::MethodSet {
            member: "allowedHTTPMethods",
            value: "GET, HEAD",
        }),
    },
    Row {
        case: "the whole forwarding set on the member that caches responses",
        settings: || json!({"cachedHTTPMethods": "GET,HEAD,OPTIONS,PUT,PATCH,POST,DELETE"}),
        offends: Some(Offends::MethodSet {
            member: "cachedHTTPMethods",
            value: "GET,HEAD,OPTIONS,PUT,PATCH,POST,DELETE",
        }),
    },
    Row {
        case: "a single method the cached set does not document",
        settings: || json!({"cachedHTTPMethods": "POST"}),
        offends: Some(Offends::MethodSet {
            member: "cachedHTTPMethods",
            value: "POST",
        }),
    },
    Row {
        case: "a documented forwarding set with the cached member outside its own",
        settings: || {
            json!({
                "allowedHTTPMethods": "GET,HEAD,OPTIONS",
                "cachedHTTPMethods": "GET,HEAD,PUT",
            })
        },
        offends: Some(Offends::MethodSet {
            member: "cachedHTTPMethods",
            value: "GET,HEAD,PUT",
        }),
    },
    // --- the minimum TTL beside the headers the settings forward ---
    Row {
        case: "all headers forwarded with the minimum TTL of zero",
        settings: || json!({"minimumTTL": 0, "forwardedHeaders": {"option": FORWARD_ALL}}),
        offends: None,
    },
    Row {
        case: "all headers forwarded with no minimum TTL stated",
        settings: || {
            json!({
                "defaultTTL": 86_400,
                "maximumTTL": 31_536_000,
                "forwardedHeaders": {"option": FORWARD_ALL},
            })
        },
        offends: None,
    },
    Row {
        case: "only the default headers forwarded, with a day of minimum TTL",
        settings: || json!({"minimumTTL": 86_400, "forwardedHeaders": {"option": FORWARD_NONE}}),
        offends: None,
    },
    Row {
        case: "an allow list of headers forwarded, with a day of minimum TTL",
        settings: || {
            json!({
                "minimumTTL": 86_400,
                "forwardedHeaders": {
                    "option": FORWARD_ALLOW_LIST,
                    "headersAllowList": ["Accept", "Host"],
                },
            })
        },
        offends: None,
    },
    Row {
        case: "no forwarded-headers object at all, with a day of minimum TTL",
        settings: || json!({"minimumTTL": 86_400}),
        offends: None,
    },
    Row {
        case: "all cookies and query strings forwarded, with a day of minimum TTL",
        settings: || {
            json!({
                "minimumTTL": 86_400,
                "forwardedCookies": {"option": FORWARD_ALL},
                "forwardedQueryStrings": {"option": true},
            })
        },
        offends: None,
    },
    Row {
        case: "all headers forwarded beside a day of minimum TTL",
        settings: || json!({"minimumTTL": 86_400, "forwardedHeaders": {"option": FORWARD_ALL}}),
        offends: Some(Offends::MinimumTtl(86_400)),
    },
    Row {
        case: "all headers forwarded beside the smallest non-zero minimum TTL",
        settings: || json!({"minimumTTL": 1, "forwardedHeaders": {"option": FORWARD_ALL}}),
        offends: Some(Offends::MinimumTtl(1)),
    },
    Row {
        case: "all headers forwarded, an allow list beside it, and a non-zero minimum TTL",
        settings: || {
            json!({
                "defaultTTL": 86_400,
                "minimumTTL": 3_600,
                "maximumTTL": 31_536_000,
                "allowedHTTPMethods": "GET,HEAD",
                "cachedHTTPMethods": "GET,HEAD",
                "forwardedHeaders": {"option": FORWARD_ALL, "headersAllowList": ["Accept"]},
            })
        },
        offends: Some(Offends::MinimumTtl(3_600)),
    },
];

/// `value` with every null member dropped, so an object sent without an
/// optional member compares equal to the one a read reports.
fn without_nulls(value: &Value) -> Value {
    match value {
        Value::Array(entries) => Value::Array(entries.iter().map(without_nulls).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), without_nulls(member)))
                .collect(),
        ),
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// Stand up the origin instance and the distribution the update cases apply
/// their settings to, settled into [`DEPLOYED`] so each case starts from a
/// stable record.
async fn stand_up(emulator: &Emulator) {
    create_running_instance(emulator, ORIGIN_INSTANCE, &format!("{DISTRIBUTION_REGION}a")).await;
    emulator
        .ok("CreateDistribution", create_body(DISTRIBUTION, None))
        .await;
    wait_for_distribution_status(emulator, DISTRIBUTION, DEPLOYED).await;
}

/// The `CreateDistribution` body for `name`, carrying `settings` as its cache
/// behaviour settings when the case states any.
fn create_body(name: &str, settings: Option<Value>) -> Value {
    let mut body = json!({
        "distributionName": name,
        "bundleId": BUNDLE_ID,
        "origin": {"name": ORIGIN_INSTANCE, "regionName": DISTRIBUTION_REGION},
        "defaultCacheBehavior": {"behavior": "cache"},
    });
    if let Some(settings) = settings {
        body["cacheBehaviorSettings"] = settings;
    }
    body
}

/// Assert `body` is this rule's refusal: the modelled `InvalidInputException`,
/// whose message names the member that is wrong and the value the request
/// carried in it.
fn assert_refusal(case: &str, status: u16, body: &Value, offends: Offends) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "InvalidInputException",
        "case {case}: expected InvalidInputException, got {code} {body:?}"
    );
    let names = match offends {
        Offends::MethodSet { member, value } => {
            format!("cacheBehaviorSettings.{member} '{value}'")
        }
        Offends::MinimumTtl(minimum_ttl) => {
            format!("cacheBehaviorSettings.minimumTTL is {minimum_ttl}")
        }
    };
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&names),
        "case {case}: expected a message naming {names}, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The documented domains are the ones the table walks
// ---------------------------------------------------------------------------

/// RULE: every method set the two members document is one a distribution can be
/// configured with, so a create carrying any of them is served and reports it
/// back — the published domains themselves, not only the rows above.
#[tokio::test]
async fn every_documented_http_method_set_configures_a_distribution() {
    let emulator = Emulator::new();
    stand_up(&emulator).await;

    for (index, allowed) in ALLOWED_METHOD_SETS.iter().enumerate() {
        for (cached_index, cached) in CACHED_METHOD_SETS.iter().enumerate() {
            let case = format!("allowed {allowed} with cached {cached}");
            let name = format!("method-set-cdn-{index}-{cached_index}");
            let settings = json!({"allowedHTTPMethods": allowed, "cachedHTTPMethods": cached});
            let (status, body) = emulator
                .call(
                    "CreateDistribution",
                    create_body(&name, Some(settings.clone())),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {body:?}"
            );
            let reported = distribution(&emulator, &name).await;
            let got = without_nulls(&reported["cacheBehaviorSettings"]);
            assert_eq!(
                got,
                without_nulls(&settings),
                "case {case}: expected {settings:?} read back, got {got:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CreateDistribution
// ---------------------------------------------------------------------------

/// RULE: `CreateDistribution` creates a distribution with the cache settings it
/// was given when both documented rules hold, and a request whose settings
/// break either creates no distribution at all.
#[tokio::test]
async fn create_distribution_takes_only_documented_cache_behavior_settings() {
    let emulator = Emulator::new();
    stand_up(&emulator).await;

    for (row_index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("created-cache-cdn-{row_index}");
        let settings = (row.settings)();
        let (status, body) = emulator
            .call(
                "CreateDistribution",
                create_body(&name, Some(settings.clone())),
            )
            .await;

        if let Some(offends) = row.offends {
            assert_refusal(case, status, &body, offends);
            // A refused create stores nothing, so the name addresses no
            // distribution at all.
            let (read_status, read_body) = emulator
                .call("GetDistributions", json!({"distributionName": name}))
                .await;
            assert_eq!(
                read_status, 400,
                "case {case}: expected the refused create to store nothing, got {read_status} \
                 {read_body:?}"
            );
            assert_eq!(
                read_body["code"].as_str(),
                Some(DOES_NOT_EXIST),
                "case {case}: expected the {DOES_NOT_EXIST} code, got {read_body:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        // The round trip: what the create stored is what the next read reports.
        let reported = distribution(&emulator, &name).await;
        let got = without_nulls(&reported["cacheBehaviorSettings"]);
        assert_eq!(
            got,
            without_nulls(&settings),
            "case {case}: expected {settings:?} read back, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateDistribution
// ---------------------------------------------------------------------------

/// RULE: the same two rules hold on the update, which replaces the whole object
/// — settings that satisfy them are stored and reported back unchanged, and
/// settings that break either are refused and change nothing.
#[tokio::test]
async fn update_distribution_takes_only_documented_cache_behavior_settings() {
    let emulator = Emulator::new();
    stand_up(&emulator).await;

    for row in ROWS {
        let case = row.case;
        let before = distribution(&emulator, DISTRIBUTION).await;
        let settings = (row.settings)();
        let (status, body) = emulator
            .call(
                "UpdateDistribution",
                json!({
                    "distributionName": DISTRIBUTION,
                    "cacheBehaviorSettings": settings.clone(),
                }),
            )
            .await;

        if let Some(offends) = row.offends {
            assert_refusal(case, status, &body, offends);
            let after = distribution(&emulator, DISTRIBUTION).await;
            assert_eq!(
                after, before,
                "case {case}: expected the refused update to change nothing, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        // The round trip: the object the update stored replaces the one before
        // it, and is what the next read reports.
        let reported = wait_for_distribution_status(&emulator, DISTRIBUTION, DEPLOYED).await;
        let got = without_nulls(&reported["cacheBehaviorSettings"]);
        assert_eq!(
            got,
            without_nulls(&settings),
            "case {case}: expected {settings:?} read back, got {got:?}"
        );
    }
}
