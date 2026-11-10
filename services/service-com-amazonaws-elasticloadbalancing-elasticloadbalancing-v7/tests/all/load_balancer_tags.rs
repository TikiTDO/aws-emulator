// SPDX-License-Identifier: CC0-1.0

//! A Classic load balancer's tag set: what `AddTags` writes and what `DescribeTags`
//! reports.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour rather than a
//! handler called directly.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded request:
//!
//! * `AddTags` upserts the request's tags onto the named load balancer's stored record —
//!   a key the record already carries takes the request's value, every other key is added,
//!   and the keys the request does not name are left alone — and the set it leaves behind
//!   is exactly the set `DescribeTags` reports, whichever operation wrote it.
//! * the quota bounds the set a load balancer ENDS UP with (10 tags per load balancer, from
//!   the "Elastic Load Balancing endpoints and quotas" page of the AWS General Reference),
//!   so re-stating a key the record holds is always accepted and only the keys a request
//!   ADDS are counted against it.
//! * a refusal is the modelled one — `LoadBalancerNotFound`, `DuplicateTagKeys` or
//!   `TooManyTags`, all HTTP 400 — and leaves the stored tag set exactly as it found it.
//! * a load balancer is a REGIONAL resource, so both operations see only the records of
//!   the region the request is signed for.

use super::wire::{call, emulator, error_message, kid_text, members, param, parse, Wire};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for unless the case says otherwise.
const REGION: &str = "us-west-2";

/// The published per-load-balancer tag quota ("Tags per load balancer: 10", from the
/// "Elastic Load Balancing endpoints and quotas" page of the AWS General Reference).
const TAG_QUOTA: usize = 10;

/// One tag as a request carries it: a key, and a value that may be absent — `Tag$Value` is
/// optional ("Each tag consists of a key and an optional value").
type TagSpec = (String, Option<String>);

/// One tag of a case, stated the way a client states it.
fn tag(key: &str, value: Option<&str>) -> TagSpec {
    (key.to_string(), value.map(str::to_string))
}

/// One `(key, value)` pair of an expected tag set.
fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// An expected tag set, sorted so a case states the SET rather than an order the wire
/// happens to report.
fn set(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut sorted = tags.to_vec();
    sorted.sort();
    sorted
}

/// The `Tags.member.N` form fields for `tags`. A tag whose value is `None` carries no
/// `Value` field at all, which is how a client states the optional member's absence.
fn tag_params(tags: &[TagSpec]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let slot = index + 1;
        params.push(param(&format!("Tags.member.{slot}.Key"), key));
        if let Some(value) = value {
            params.push(param(&format!("Tags.member.{slot}.Value"), value));
        }
    }
    params
}

/// Create `name` in `region` with one HTTP 80 listener in one Availability Zone, carrying
/// `tags`.
async fn create(router: &MultiServiceRouter, region: &str, name: &str, tags: &[TagSpec]) {
    let mut params = vec![
        param("LoadBalancerName", name),
        param("AvailabilityZones.member.1", &format!("{region}a")),
        param("Listeners.member.1.Protocol", "HTTP"),
        param("Listeners.member.1.LoadBalancerPort", "80"),
        param("Listeners.member.1.InstancePort", "80"),
    ];
    params.extend(tag_params(tags));
    let answer = call(router, region, "CreateLoadBalancer", &params).await;
    assert_eq!(
        answer.status, 200,
        "creating {name:?} must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `AddTags` naming one load balancer and carrying `tags`.
async fn add_tags(router: &MultiServiceRouter, region: &str, name: &str, tags: &[TagSpec]) -> Wire {
    let mut params = vec![param("LoadBalancerNames.member.1", name)];
    params.extend(tag_params(tags));
    call(router, region, "AddTags", &params).await
}

/// `DescribeTags` naming `names`, whatever it answers.
async fn describe_tags(router: &MultiServiceRouter, region: &str, names: &[&str]) -> Wire {
    let params: Vec<(String, String)> = names
        .iter()
        .enumerate()
        .map(|(index, name)| param(&format!("LoadBalancerNames.member.{}", index + 1), name))
        .collect();
    call(router, region, "DescribeTags", &params).await
}

/// The `(LoadBalancerName, tags)` descriptions `DescribeTags` reports for `names`, in the
/// order the answer reports them.
async fn described_tags(
    router: &MultiServiceRouter,
    region: &str,
    names: &[&str],
) -> Vec<(String, Vec<(String, String)>)> {
    let answer = describe_tags(router, region, names).await;
    assert_eq!(
        answer.status, 200,
        "DescribeTags for {names:?} must succeed, got {} with {}",
        answer.status, answer.body
    );
    let doc = parse(&answer.body);
    let result = answer.result(&doc);
    members(result, "TagDescriptions")
        .into_iter()
        .map(|description| {
            let tags = members(description, "Tags")
                .into_iter()
                .map(|entry| {
                    (
                        kid_text(entry, "Key").unwrap_or_default(),
                        kid_text(entry, "Value").unwrap_or_default(),
                    )
                })
                .collect();
            (
                kid_text(description, "LoadBalancerName").unwrap_or_default(),
                tags,
            )
        })
        .collect()
}

/// The tag set one load balancer carries, sorted.
async fn tags_of(router: &MultiServiceRouter, region: &str, name: &str) -> Vec<(String, String)> {
    let described = described_tags(router, region, &[name]).await;
    assert_eq!(
        described.len(),
        1,
        "case {name:?}: DescribeTags of one load balancer reports one description, got \
         {described:?}"
    );
    set(&described[0].1)
}

/// One row of the upsert table: the case, the tags the create carried, the tags one
/// `AddTags` call adds, and the set the load balancer must end up with.
struct UpsertCase {
    case: &'static str,
    created_with: Vec<TagSpec>,
    added: Vec<TagSpec>,
    expected: Vec<(String, String)>,
}

/// What a request's tags do to the stored set, and what a later read reports.
///
/// The rows differ along what the rule generalises over: how many tags one call carries,
/// whether the value is given, empty or at its modelled maximum, whether the key is new or
/// one the record already carries, and whether the record was tagged at create time. The
/// recorded case — one `sim2real:managed` tag on a freshly created load balancer — is the
/// first row.
#[tokio::test]
async fn add_tags_upserts_the_set_describe_tags_reports() {
    let router = emulator();
    // `Tag$Key` is `@length(min: 1, max: 128)` and `Tag$Value` `@length(min: 0, max: 256)`,
    // so these are the longest key and value the model accepts.
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let cases = [
        UpsertCase {
            case: "one tag onto an untagged load balancer",
            created_with: Vec::new(),
            added: vec![tag("sim2real:managed", Some("true"))],
            expected: vec![pair("sim2real:managed", "true")],
        },
        UpsertCase {
            case: "three tags in one call",
            created_with: Vec::new(),
            added: vec![
                tag("env", Some("prod")),
                tag("team", Some("payments")),
                tag("cost-centre", Some("4711")),
            ],
            expected: vec![
                pair("cost-centre", "4711"),
                pair("env", "prod"),
                pair("team", "payments"),
            ],
        },
        UpsertCase {
            case: "an explicitly empty value",
            created_with: Vec::new(),
            added: vec![tag("owner", Some(""))],
            expected: vec![pair("owner", "")],
        },
        UpsertCase {
            case: "no Value member at all",
            created_with: Vec::new(),
            added: vec![tag("owner", None)],
            expected: vec![pair("owner", "")],
        },
        UpsertCase {
            case: "a key the create already set takes the request's value",
            created_with: vec![tag("env", Some("dev"))],
            added: vec![tag("env", Some("prod"))],
            expected: vec![pair("env", "prod")],
        },
        UpsertCase {
            case: "keys the request does not name are left alone",
            created_with: vec![tag("env", Some("dev")), tag("team", Some("payments"))],
            added: vec![tag("owner", Some("ops"))],
            expected: vec![
                pair("env", "dev"),
                pair("owner", "ops"),
                pair("team", "payments"),
            ],
        },
        UpsertCase {
            case: "the whole character set the key and value patterns allow",
            created_with: Vec::new(),
            added: vec![tag("a_b.c:d/e=f+g-h@i j", Some("A_b.c:d/e=f+g-h@i j"))],
            expected: vec![pair("a_b.c:d/e=f+g-h@i j", "A_b.c:d/e=f+g-h@i j")],
        },
        UpsertCase {
            case: "a key and a value at their modelled maximum length",
            created_with: Vec::new(),
            added: vec![tag(&longest_key, Some(&longest_value))],
            expected: vec![pair(&longest_key, &longest_value)],
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = format!("tag-upsert-{index}");
        create(&router, REGION, &name, &case.created_with).await;
        let answer = add_tags(&router, REGION, &name, &case.added).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: AddTags must answer 200, got {} with {}",
            case.case, answer.status, answer.body
        );
        // The modelled output carries no member, so the success an awsQuery client reads is
        // the empty `AddTagsResult`.
        let doc = parse(&answer.body);
        assert_eq!(
            answer.result(&doc).name,
            "AddTagsResult",
            "case {:?}: expected an AddTagsResult envelope, got {}",
            case.case,
            answer.body
        );

        let stored = tags_of(&router, REGION, &name).await;
        assert_eq!(
            stored,
            set(&case.expected),
            "case {:?}: expected the upserted set, got {stored:?}",
            case.case
        );
    }
}

/// One step of the accumulation walk: the tags one call carries, and the set the load
/// balancer must carry after it.
type AccumulationStep = (Vec<TagSpec>, Vec<(String, String)>);

/// Tagging the same load balancer again and again is the same rule applied repeatedly: the
/// set after each call is the previous set with that call's keys upserted into it.
#[tokio::test]
async fn successive_calls_accumulate_into_one_set() {
    let router = emulator();
    create(&router, REGION, "tag-accumulate-elb", &[]).await;

    // (the tags the call carries, the set the load balancer must then carry)
    let steps: Vec<AccumulationStep> = vec![
        (vec![tag("a", Some("1"))], vec![pair("a", "1")]),
        (
            vec![tag("b", Some("2"))],
            vec![pair("a", "1"), pair("b", "2")],
        ),
        (
            vec![tag("a", Some("9")), tag("c", None)],
            vec![pair("a", "9"), pair("b", "2"), pair("c", "")],
        ),
        (
            vec![tag("b", Some("2"))],
            vec![pair("a", "9"), pair("b", "2"), pair("c", "")],
        ),
    ];

    for (step, (tags, expected)) in steps.iter().enumerate() {
        let answer = add_tags(&router, REGION, "tag-accumulate-elb", tags).await;
        assert_eq!(
            answer.status, 200,
            "step {step}: AddTags must answer 200, got {} with {}",
            answer.status, answer.body
        );
        let stored = tags_of(&router, REGION, "tag-accumulate-elb").await;
        assert_eq!(
            stored,
            set(expected),
            "step {step}: expected the accumulated set, got {stored:?}"
        );
    }
}

/// `count` tags, `key-1 = value-1` .. `key-N = value-N`.
fn numbered(count: usize) -> Vec<TagSpec> {
    (1..=count)
        .map(|slot| tag(&format!("key-{slot}"), Some(&format!("value-{slot}"))))
        .collect()
}

/// The set `numbered` tags, or a restatement of them, leaves behind.
fn numbered_set(count: usize, value: Option<&str>) -> Vec<(String, String)> {
    set(&(1..=count)
        .map(|slot| {
            pair(
                &format!("key-{slot}"),
                value.unwrap_or(&format!("value-{slot}")),
            )
        })
        .collect::<Vec<_>>())
}

/// The quota bounds the set the load balancer ends up with, so a request is refused only
/// when the keys it ADDS would push the set past it — and a refusal stores nothing.
///
/// The rows walk the published bound from below it to past it: a set one short of the quota
/// and one exactly at it are accepted, one past it is refused, re-stating every key of a
/// full set is accepted (it adds none), and one new key beyond a full set is refused.
#[tokio::test]
async fn the_tag_quota_bounds_the_set_a_load_balancer_ends_up_with() {
    let router = emulator();

    for (case, count, accepted) in [
        ("one below the quota", TAG_QUOTA - 1, true),
        ("exactly the quota", TAG_QUOTA, true),
        ("one past the quota", TAG_QUOTA + 1, false),
        ("far past the quota", TAG_QUOTA * 10, false),
    ] {
        let name = format!("tag-quota-{count}");
        create(&router, REGION, &name, &[]).await;
        let answer = add_tags(&router, REGION, &name, &numbered(count)).await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: {count} tags are within the quota, got {} with {}",
                answer.status, answer.body
            );
            let stored = tags_of(&router, REGION, &name).await;
            assert_eq!(
                stored,
                numbered_set(count, None),
                "case {case:?}: expected every requested tag stored, got {stored:?}"
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, "TooManyTags".to_string()),
                "case {case:?}: {count} tags are past the quota, got {} with {}",
                answer.status,
                answer.body
            );
            assert_eq!(
                error_message(&answer),
                format!("Quota of {TAG_QUOTA} tags per load balancer reached"),
                "case {case:?}: the refusal must state the quota, got {}",
                answer.body
            );
            let stored = tags_of(&router, REGION, &name).await;
            assert!(
                stored.is_empty(),
                "case {case:?}: a refused request stores nothing, got {stored:?}"
            );
        }
    }

    // A load balancer already carrying a full set: re-stating its keys adds none, so it is
    // accepted and updates the values, while one further key is refused and changes nothing.
    create(&router, REGION, "tag-quota-full", &[]).await;
    let filled = add_tags(&router, REGION, "tag-quota-full", &numbered(TAG_QUOTA)).await;
    assert_eq!(
        filled.status, 200,
        "a set exactly at the quota must be accepted, got {} with {}",
        filled.status, filled.body
    );

    let restated: Vec<TagSpec> = numbered(TAG_QUOTA)
        .into_iter()
        .map(|(key, _)| tag(&key, Some("restated")))
        .collect();
    let answer = add_tags(&router, REGION, "tag-quota-full", &restated).await;
    assert_eq!(
        answer.status, 200,
        "case \"re-stating every key of a full set\": it adds no key, got {} with {}",
        answer.status, answer.body
    );
    let expected = numbered_set(TAG_QUOTA, Some("restated"));
    let stored = tags_of(&router, REGION, "tag-quota-full").await;
    assert_eq!(
        stored, expected,
        "case \"re-stating every key of a full set\": expected the updated values, got \
         {stored:?}"
    );

    let answer = add_tags(
        &router,
        REGION,
        "tag-quota-full",
        &[tag("one-more", Some("x"))],
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "TooManyTags".to_string()),
        "case \"one key beyond a full set\": expected the modelled refusal, got {} with {}",
        answer.status,
        answer.body
    );
    let after = tags_of(&router, REGION, "tag-quota-full").await;
    assert_eq!(
        after, expected,
        "case \"one key beyond a full set\": a refused request changes nothing, got {after:?}"
    );
}

/// One row of the refusal table: the case, the names the request carries, the tags it
/// carries, the error code it must be refused with and the message that refusal states.
struct RefusalCase {
    case: &'static str,
    names: Vec<&'static str>,
    tags: Vec<TagSpec>,
    code: &'static str,
    message: &'static str,
}

/// `AddTags` refuses with the modelled error for the rule the request broke, and leaves the
/// stored set exactly as it found it.
///
/// The rows span the refusals the operation models for the request itself — an unknown load
/// balancer, including one only ANOTHER region holds, and a key stated twice — plus the
/// documented "you can specify one load balancer only" rule, which the model states in the
/// member's documentation and declares no error for, over a request naming none, two and
/// three.
#[tokio::test]
async fn add_tags_refuses_with_the_modelled_error_and_stores_nothing() {
    let router = emulator();
    create(
        &router,
        REGION,
        "tag-refusal-elb",
        &[tag("env", Some("dev"))],
    )
    .await;
    // The same account in another region: a load balancer is a regional resource, so this
    // one is invisible to a request signed for `REGION`.
    create(&router, "eu-west-1", "tag-elsewhere-elb", &[]).await;

    let cases = vec![
        RefusalCase {
            case: "a load balancer no region holds",
            names: vec!["tag-absent-elb"],
            tags: vec![tag("env", Some("prod"))],
            code: "LoadBalancerNotFound",
            message: "There is no ACTIVE Load Balancer named 'tag-absent-elb'",
        },
        RefusalCase {
            case: "a load balancer only another region holds",
            names: vec!["tag-elsewhere-elb"],
            tags: vec![tag("env", Some("prod"))],
            code: "LoadBalancerNotFound",
            message: "There is no ACTIVE Load Balancer named 'tag-elsewhere-elb'",
        },
        RefusalCase {
            case: "one key stated twice",
            names: vec!["tag-refusal-elb"],
            tags: vec![tag("env", Some("prod")), tag("env", Some("staging"))],
            code: "DuplicateTagKeys",
            // `DuplicateTagKeys` is FIXED text in full: the two rows state different keys
            // and earn the identical sentence, which names neither the key nor the load
            // balancer.
            message: "Duplicate Tag Keys found in the request. Tag keys should be unique.",
        },
        RefusalCase {
            case: "one key stated twice, once without a value",
            names: vec!["tag-refusal-elb"],
            tags: vec![tag("owner", None), tag("owner", Some("ops"))],
            code: "DuplicateTagKeys",
            message: "Duplicate Tag Keys found in the request. Tag keys should be unique.",
        },
        RefusalCase {
            // `LoadBalancerNames` is `@required`, so a request carrying no name at all is
            // refused by the boundary validator before the handler runs — which is the
            // same rule, stated one layer earlier.
            case: "no load balancer name at all",
            names: Vec::new(),
            tags: vec![tag("env", Some("prod"))],
            code: "MalformedInput",
            message: "1 validation error detected: Value null at 'loadBalancerNames' failed to \
                      satisfy constraint: Member must not be null",
        },
        RefusalCase {
            case: "two load balancer names",
            names: vec!["tag-refusal-elb", "tag-elsewhere-elb"],
            tags: vec![tag("env", Some("prod"))],
            code: "ValidationError",
            message: "LoadBalancerNames must name exactly one load balancer; got 2",
        },
        RefusalCase {
            case: "three load balancer names",
            names: vec!["tag-refusal-elb", "tag-elsewhere-elb", "tag-absent-elb"],
            tags: vec![tag("env", Some("prod"))],
            code: "ValidationError",
            message: "LoadBalancerNames must name exactly one load balancer; got 3",
        },
    ];

    for case in &cases {
        let mut params: Vec<(String, String)> = case
            .names
            .iter()
            .enumerate()
            .map(|(index, name)| param(&format!("LoadBalancerNames.member.{}", index + 1), name))
            .collect();
        params.extend(tag_params(&case.tags));
        let answer = call(&router, REGION, "AddTags", &params).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, case.code.to_string()),
            "case {:?}: expected 400 {}, got {} with {}",
            case.case,
            case.code,
            answer.status,
            answer.body
        );
        assert_eq!(
            error_message(&answer),
            case.message,
            "case {:?}: unexpected refusal message in {}",
            case.case,
            answer.body
        );

        // The load balancer still carries the set its create left on it.
        let stored = tags_of(&router, REGION, "tag-refusal-elb").await;
        assert_eq!(
            stored,
            vec![pair("env", "dev")],
            "case {:?}: a refused request leaves the stored set alone, got {stored:?}",
            case.case
        );
    }
}

/// One row of the read table: the case, the names one `DescribeTags` request carries, and
/// the `(LoadBalancerName, tags)` descriptions it must report, in order.
type DescribeCase = (
    &'static str,
    Vec<&'static str>,
    Vec<(&'static str, Vec<(String, String)>)>,
);

/// `DescribeTags` reports the stored set of every load balancer the request names, in the
/// order it names them, once per distinct name — and refuses a name the region does not
/// hold with the modelled error.
#[tokio::test]
async fn describe_tags_reports_the_stored_set_of_every_name_it_is_given() {
    let router = emulator();
    create(&router, REGION, "tag-read-a", &[tag("env", Some("prod"))]).await;
    create(&router, REGION, "tag-read-b", &[]).await;
    create(
        &router,
        REGION,
        "tag-read-c",
        &[tag("env", Some("dev")), tag("owner", Some(""))],
    )
    .await;

    // (case, the names the request carries, the descriptions it must report)
    let cases: Vec<DescribeCase> = vec![
        (
            "one tagged load balancer",
            vec!["tag-read-a"],
            vec![("tag-read-a", vec![pair("env", "prod")])],
        ),
        (
            "a load balancer nobody has tagged reports an empty set",
            vec!["tag-read-b"],
            vec![("tag-read-b", Vec::new())],
        ),
        (
            "three names are reported in the order the request names them",
            vec!["tag-read-c", "tag-read-a", "tag-read-b"],
            vec![
                ("tag-read-c", vec![pair("env", "dev"), pair("owner", "")]),
                ("tag-read-a", vec![pair("env", "prod")]),
                ("tag-read-b", Vec::new()),
            ],
        ),
        (
            "a repeated name names one load balancer, reported once",
            vec!["tag-read-a", "tag-read-a"],
            vec![("tag-read-a", vec![pair("env", "prod")])],
        ),
    ];

    for (case, names, expected) in cases {
        let expected: Vec<(String, Vec<(String, String)>)> = expected
            .into_iter()
            .map(|(name, tags)| (name.to_string(), set(&tags)))
            .collect();
        let actual: Vec<(String, Vec<(String, String)>)> = described_tags(&router, REGION, &names)
            .await
            .into_iter()
            .map(|(name, tags)| (name, set(&tags)))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored sets of the names given, got {actual:?}"
        );
    }

    // A name the region does not hold is the modelled refusal, whichever position it holds
    // in the list.
    for (case, names) in [
        ("only an absent name", vec!["tag-read-absent"]),
        (
            "an absent name beside present ones",
            vec!["tag-read-a", "tag-read-absent", "tag-read-b"],
        ),
    ] {
        let answer = describe_tags(&router, REGION, &names).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "LoadBalancerNotFound".to_string()),
            "case {case:?}: expected the modelled refusal, got {} with {}",
            answer.status,
            answer.body
        );
        assert_eq!(
            error_message(&answer),
            "There is no ACTIVE Load Balancer named 'tag-read-absent'",
            "case {case:?}: the refusal must name the load balancer, got {}",
            answer.body
        );
    }

    // The region scopes the read too: the names above belong to `REGION` alone.
    let answer = describe_tags(&router, "eu-west-1", &["tag-read-a"]).await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "LoadBalancerNotFound".to_string()),
        "case \"another region's read\": expected the modelled refusal, got {} with {}",
        answer.status,
        answer.body
    );
}
