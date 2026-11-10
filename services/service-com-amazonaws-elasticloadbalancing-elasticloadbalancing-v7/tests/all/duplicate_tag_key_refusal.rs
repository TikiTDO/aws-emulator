// SPDX-License-Identifier: CC0-1.0

//! The refusal a request stating one tag key twice earns, over every operation that takes
//! a tag list.
//!
//! The rule under test: `DuplicateTagKeysException` — documented "A tag key was specified
//! more than once", awsQuery code `DuplicateTagKeys`, HTTP 400 — is worded with Elastic
//! Load Balancing's own FIXED sentence, which states the rule the caller has to satisfy
//! and names neither the duplicated key nor the load balancer. So every request that
//! breaks the rule earns the IDENTICAL message, whichever key it repeated, whatever value
//! it gave the key, wherever in the list the repeat is, and whichever operation carried the
//! list — and a client cannot read the offending key out of it, because the real service
//! puts none there.
//!
//! Asserted over a TABLE whose rows differ in the key, the values, the position of the
//! repeat, how many times the key is stated and how many other tags the list carries, and
//! over BOTH operations the model declares the error on (`AddTags` and
//! `CreateLoadBalancer`). The quota refusal of the same tag list is the neighbouring
//! `load_balancer_tags.rs` suite's.

use super::wire::{call, emulator, error_message, param, Wire};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The sentence Elastic Load Balancing refuses a duplicated tag key with, in full. Fixed
/// text: no part of it comes from the request.
const DUPLICATE_TAG_KEYS_MESSAGE: &str =
    "Duplicate Tag Keys found in the request. Tag keys should be unique.";

/// One tag as a client states it: a key, and a value that may be absent (`Tag$Value` is
/// optional — "Each tag consists of a key and an optional value").
type TagSpec = (&'static str, Option<&'static str>);

/// The `Tags.member.N` form fields for `tags`.
fn tag_params(tags: &[TagSpec]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        params.push(param(&format!("Tags.member.{}.Key", index + 1), key));
        if let Some(value) = value {
            params.push(param(&format!("Tags.member.{}.Value", index + 1), value));
        }
    }
    params
}

/// One row: a tag list that states some key more than once.
struct Case {
    case: &'static str,
    tags: Vec<TagSpec>,
}

fn duplicate_cases() -> Vec<Case> {
    vec![
        Case {
            case: "one key twice, two different values",
            tags: vec![("env", Some("prod")), ("env", Some("staging"))],
        },
        Case {
            case: "a different key twice, the same value both times",
            tags: vec![("owner", Some("ops")), ("owner", Some("ops"))],
        },
        Case {
            case: "a key stated twice with no value at all",
            tags: vec![("team", None), ("team", None)],
        },
        Case {
            case: "the repeat at the END of a longer list",
            tags: vec![
                ("a", Some("1")),
                ("b", Some("2")),
                ("c", Some("3")),
                ("a", Some("4")),
            ],
        },
        Case {
            case: "the repeat in the MIDDLE of a longer list",
            tags: vec![
                ("first", Some("1")),
                ("second", Some("2")),
                ("second", Some("3")),
                ("third", Some("4")),
            ],
        },
        Case {
            case: "one key stated three times",
            tags: vec![
                ("thrice", Some("1")),
                ("thrice", Some("2")),
                ("thrice", Some("3")),
            ],
        },
        Case {
            case: "a key whose own text would be conspicuous in a message",
            tags: vec![
                ("Name with spaces", Some("x")),
                ("Name with spaces", Some("y")),
            ],
        },
    ]
}

/// Assert one refusal: HTTP 400, the modelled code, and the fixed sentence in full.
fn assert_duplicate_refusal(case: &str, operation: &str, answer: &Wire) {
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (400, "DuplicateTagKeys"),
        "case {case:?} on {operation}: expected 400 DuplicateTagKeys, got {}",
        answer.body
    );
    let message = error_message(answer);
    assert_eq!(
        message, DUPLICATE_TAG_KEYS_MESSAGE,
        "case {case:?} on {operation}: expected {DUPLICATE_TAG_KEYS_MESSAGE:?}, got {message:?}"
    );
}

/// The listener and placement a create needs, which no row varies.
fn create_params(name: &str) -> Vec<(String, String)> {
    vec![
        param("LoadBalancerName", name),
        param("Listeners.member.1.Protocol", "HTTP"),
        param("Listeners.member.1.LoadBalancerPort", "80"),
        param("Listeners.member.1.InstancePort", "8080"),
        param("AvailabilityZones.member.1", &format!("{REGION}a")),
    ]
}

/// Create the load balancer `AddTags` is called on.
async fn create(router: &MultiServiceRouter, name: &str) {
    let created = call(router, REGION, "CreateLoadBalancer", &create_params(name)).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateLoadBalancer {name:?} must answer 200, got {}",
        created.body
    );
}

#[tokio::test]
async fn a_duplicated_tag_key_is_refused_with_one_fixed_sentence() {
    let router = emulator();
    let tagged = "duplicate-tag-key-elb";
    create(&router, tagged).await;

    for Case { case, tags } in duplicate_cases() {
        // `AddTags` on an existing load balancer.
        let mut params = vec![param("LoadBalancerNames.member.1", tagged)];
        params.extend(tag_params(&tags));
        let added = call(&router, REGION, "AddTags", &params).await;
        assert_duplicate_refusal(case, "AddTags", &added);

        // The refused request wrote nothing: the load balancer carries no tag at all.
        let described = call(
            &router,
            REGION,
            "DescribeTags",
            &[param("LoadBalancerNames.member.1", tagged)],
        )
        .await;
        assert!(
            !described.body.contains("<Key>"),
            "case {case:?}: a refused AddTags must store no tag, got {}",
            described.body
        );

        // `CreateLoadBalancer`, which declares the SAME error for the same rule, answers
        // the same sentence — and creates nothing.
        let name = "duplicate-tag-key-create-elb";
        let mut params = create_params(name);
        params.extend(tag_params(&tags));
        let created = call(&router, REGION, "CreateLoadBalancer", &params).await;
        assert_duplicate_refusal(case, "CreateLoadBalancer", &created);
        let missing = call(
            &router,
            REGION,
            "DescribeLoadBalancers",
            &[param("LoadBalancerNames.member.1", name)],
        )
        .await;
        assert_eq!(
            (missing.status, missing.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {case:?}: a refused create must create nothing, got {}",
            missing.body
        );
    }
}
