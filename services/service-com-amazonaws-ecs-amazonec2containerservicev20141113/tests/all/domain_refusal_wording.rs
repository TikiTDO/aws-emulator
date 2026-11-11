// SPDX-License-Identifier: CC0-1.0

//! What Amazon ECS SAYS when it refuses a request, and the cross-field rules it refuses
//! requests for, driven end to end over the shared wire harness in [`crate::ecs_wire`].
//!
//! The rules under test, each asserted over a TABLE that spans the class rather than over
//! the one request that exposed it:
//!
//! * A container definition is refused when it leaves unset a member a runnable container
//!   cannot be built without — its `name` or its `image` — and the refusal names that
//!   member of the container structure in one fixed sentence; a `containerDefinitions`
//!   list with no entry is refused as the empty list it is. Neither refusal echoes
//!   anything of the request, and neither mints a revision.
//! * A container definition whose soft memory limit is above its hard one asks for a
//!   reservation the container can never use ("if you specify both, `memory` must be
//!   greater than `memoryReservation`"), so it is refused naming the container the
//!   request gave the pair on; a pair where the hard limit is at least the soft one is
//!   stored and read back.
//! * A constraint Amazon ECS words in its own DOMAIN terms is answered in those terms and
//!   not in the model's: a tag whose characters are outside the set a tag admits, a
//!   capacity-provider-strategy `base` or `weight` outside its range, and a cluster
//!   setting whose value is outside the supported set.
//! * Every capacity-provider name a request associates with a cluster has to resolve to a
//!   provider this account holds — one a caller created, or one of the two Amazon ECS
//!   manages — and a name that resolves to nothing leaves the association untouched.
//! * A deployment configuration's `minimumHealthyPercent` is a percentage of the
//!   service's `desiredCount`, so it lies between 0 and 100; a value outside that range
//!   is refused by both the create and the update, and nothing is written.
//! * A request that carries BOTH a `launchType` and a `capacityProviderStrategy` names
//!   where its tasks run twice, and is refused with one fixed sentence by every operation
//!   that takes the pair.
//! * `DeleteTaskDefinitions` reports a revision that is still `ACTIVE` through
//!   `failures[]` — the ARN the entry named and a reason that states the status and the
//!   remedy, and no other member — and deletes it once it is deregistered.
//!
//! Every case goes through the wire harness, the same front door a client uses, so
//! nothing reaches into the state struct and a response that is not computed from
//! persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    cluster_with_capacity, count, create_auto_scaling_group, list, ok, refusal,
    register_definition, service, string, ACCOUNT, SIGNED_REGION,
};
use crate::resource_tags::{listed, CLUSTER as TAGGED_CLUSTER};
use crate::task_lifecycle::container;

/// A container image, as a container definition names one. Irrelevant to every rule here,
/// so one value.
const IMAGE: &str = "public.ecr.aws/nginx/nginx:latest";

/// The modelled error a container-definition refusal of a registration carries.
const CLIENT_EXCEPTION: &str = "ClientException";

/// The modelled error every other refusal here carries.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The Amazon ECS quota "Capacity providers per cluster", which bounds both lists a
/// cluster's association may carry, from this crate's own statement of it.
const MAX_CLUSTER_CAPACITY_PROVIDERS: usize =
    service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::MAX_CLUSTER_CAPACITY_PROVIDERS;

/// The ARN a revision of `family` carries.
fn definition_arn(family: &str, revision: i64) -> String {
    format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:task-definition/{family}:{revision}")
}

/// The revision ARNs `ListTaskDefinitions` reports for one family — empty when a refused
/// registration minted nothing.
async fn revisions_of(router: &MultiServiceRouter, family: &str, case: &str) -> Vec<Value> {
    let body = ok(
        router,
        "ListTaskDefinitions",
        json!({ "familyPrefix": family }),
        case,
    )
    .await;
    list(&body, "taskDefinitionArns", case)
}

/// The one cluster a describe reports, with the optional fields `include` asks for.
async fn described_cluster(
    router: &MultiServiceRouter,
    cluster: &str,
    include: Value,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeClusters",
        json!({ "clusters": [cluster], "include": include }),
        case,
    )
    .await;
    let clusters = list(&body, "clusters", case);
    assert_eq!(
        clusters.len(),
        1,
        "case {case:?}: expected exactly one cluster back, got {body}"
    );
    clusters[0].clone()
}

// ---------------------------------------------------------------------------
// A container definition that leaves out a member a container needs
// ---------------------------------------------------------------------------

/// The sentence a container definition missing `member` earns.
fn container_member_sentence(member: &str) -> String {
    format!("Container.{member} should not be null or empty.")
}

#[tokio::test]
async fn a_container_definition_missing_a_member_is_refused_naming_that_member() {
    let router = service();

    // The rows differ along WHICH member is missing, HOW it is missing (absent, empty,
    // only spaces), how many containers the list carries and where the bad one sits.
    let rows: [(&str, Value, &str); 9] = [
        (
            "no name member at all",
            json!([{ "image": IMAGE, "memory": 128, "essential": true }]),
            "name",
        ),
        (
            "an empty name",
            json!([{ "name": "", "image": IMAGE, "memory": 128 }]),
            "name",
        ),
        (
            "a name of nothing but spaces",
            json!([{ "name": "   ", "image": IMAGE, "memory": 128 }]),
            "name",
        ),
        (
            "the SECOND container of two has no name",
            json!([
                { "name": "app", "image": IMAGE, "memory": 128 },
                { "image": IMAGE, "memory": 128 },
            ]),
            "name",
        ),
        (
            "no image member at all",
            json!([{ "name": "app", "memory": 128, "essential": true }]),
            "image",
        ),
        (
            "an empty image",
            json!([{ "name": "app", "image": "", "memory": 128 }]),
            "image",
        ),
        (
            "an image of nothing but spaces",
            json!([{ "name": "sidecar", "image": " ", "memory": 128 }]),
            "image",
        ),
        (
            "the LAST container of three has no image",
            json!([
                { "name": "app", "image": IMAGE, "memory": 128 },
                { "name": "log-router", "image": IMAGE, "memory": 64 },
                { "name": "proxy", "memory": 64 },
            ]),
            "image",
        ),
        (
            // Both are missing, so the refusal is the one for the member examined first:
            // the name identifies the container the rest of the sentence would be about.
            "neither member",
            json!([{ "memory": 128 }]),
            "name",
        ),
    ];

    for (index, (case, definitions, member)) in rows.iter().enumerate() {
        let family = format!("missingmember{index}");
        let (code, message) = refusal(
            &router,
            "RegisterTaskDefinition",
            json!({ "family": family, "containerDefinitions": definitions }),
            case,
        )
        .await;
        assert_eq!(
            code, CLIENT_EXCEPTION,
            "case {case:?}: expected a {CLIENT_EXCEPTION}, got {code} with {message:?}"
        );
        let expected = container_member_sentence(member);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to read {expected:?}, got {message:?}"
        );
        let minted = revisions_of(&router, &family, case).await;
        assert!(
            minted.is_empty(),
            "case {case:?}: a refused registration mints no revision, got {minted:?}"
        );
    }
}

#[tokio::test]
async fn an_empty_container_definition_list_is_refused_as_an_empty_list() {
    let router = service();

    /// The sentence a registration describing no container at all earns.
    const EMPTY_LIST: &str = "Container list cannot be empty.";

    // The same empty list, under families that differ in name and in whether the family
    // already holds a revision: the refusal is a property of the list, not of the family.
    let family_with_a_revision = "emptylistafter";
    register_definition(&router, family_with_a_revision, "setup").await;

    for (case, family) in [
        ("a family nothing was ever registered under", "emptylistnew"),
        (
            "a family that already holds a revision",
            family_with_a_revision,
        ),
    ] {
        let (code, message) = refusal(
            &router,
            "RegisterTaskDefinition",
            json!({ "family": family, "containerDefinitions": [] }),
            case,
        )
        .await;
        assert_eq!(
            code, CLIENT_EXCEPTION,
            "case {case:?}: expected a {CLIENT_EXCEPTION}, got {code} with {message:?}"
        );
        assert_eq!(
            message, EMPTY_LIST,
            "case {case:?}: expected the refusal to read {EMPTY_LIST:?}, got {message:?}"
        );
    }

    // Neither call minted anything: the new family holds nothing and the other still
    // holds just its one revision.
    let minted = revisions_of(&router, "emptylistnew", "after").await;
    assert!(
        minted.is_empty(),
        "case \"after\": the refused registration minted no revision, got {minted:?}"
    );
    let kept = revisions_of(&router, family_with_a_revision, "after").await;
    assert_eq!(
        kept,
        vec![Value::String(definition_arn(family_with_a_revision, 1))],
        "case \"after\": the family must still hold exactly the revision it had, got \
         {kept:?}"
    );
}

// ---------------------------------------------------------------------------
// A soft memory limit above the hard one
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_memory_reservation_above_the_memory_limit_is_refused_naming_the_container() {
    let router = service();

    /// The sentence a container whose soft limit is above its hard one earns.
    fn expected(container_name: &str) -> String {
        format!(
            "Invalid setting for container '{container_name}'. 'memory' must be greater \
             than or equal to 'memoryReservation'."
        )
    }

    // The rows differ along the distance between the two amounts, their magnitudes and
    // the container the pair sits on — including the pair just one MiB apart, which is
    // the smallest violation there is.
    let rows: [(&str, &str, i64, i64); 5] = [
        ("the soft limit at twice the hard one", "app", 128, 256),
        ("the soft limit one MiB above the hard one", "a", 128, 129),
        (
            "the smallest hard limit under a large soft one",
            "metrics_agent_0",
            1,
            4096,
        ),
        ("both amounts large", "x-ray-daemon", 8192, 16384),
        (
            "the second container of two carries the pair",
            "log-router",
            64,
            65,
        ),
    ];

    for (index, (case, name, memory, reservation)) in rows.iter().enumerate() {
        let family = format!("softlimit{index}");
        let bad = json!({
            "name": name,
            "image": IMAGE,
            "memory": memory,
            "memoryReservation": reservation,
        });
        let definitions = if index == 4 {
            json!([container("app", IMAGE), bad])
        } else {
            json!([bad])
        };
        let (code, message) = refusal(
            &router,
            "RegisterTaskDefinition",
            json!({ "family": family, "containerDefinitions": definitions }),
            case,
        )
        .await;
        assert_eq!(
            code, CLIENT_EXCEPTION,
            "case {case:?}: expected a {CLIENT_EXCEPTION}, got {code} with {message:?}"
        );
        let want = expected(name);
        assert_eq!(
            message, want,
            "case {case:?}: expected the refusal to read {want:?}, got {message:?}"
        );
        let minted = revisions_of(&router, &family, case).await;
        assert!(
            minted.is_empty(),
            "case {case:?}: a refused registration mints no revision, got {minted:?}"
        );
    }
}

#[tokio::test]
async fn a_memory_pair_the_limit_admits_is_stored_and_read_back() {
    let router = service();

    // The accepted side of the same rule: the hard limit above the soft one, the two
    // EQUAL (the bound itself), and each amount on its own with no pair to compare.
    let rows: [(&str, Value, Option<i64>, Option<i64>); 4] = [
        (
            "the hard limit above the soft one",
            json!({ "memory": 300, "memoryReservation": 128 }),
            Some(300),
            Some(128),
        ),
        (
            "the two amounts equal",
            json!({ "memory": 256, "memoryReservation": 256 }),
            Some(256),
            Some(256),
        ),
        (
            "a hard limit with no reservation beside it",
            json!({ "memory": 512 }),
            Some(512),
            None,
        ),
        (
            "a reservation with no hard limit beside it",
            json!({ "memoryReservation": 512 }),
            None,
            Some(512),
        ),
    ];

    for (index, (case, reservations, memory, reservation)) in rows.iter().enumerate() {
        let family = format!("goodlimit{index}");
        let mut definition = json!({ "name": "app", "image": IMAGE });
        for (member, value) in reservations.as_object().expect("an object of members") {
            definition[member] = value.clone();
        }
        ok(
            &router,
            "RegisterTaskDefinition",
            json!({
                "family": family,
                // A task-level memory, so a definition that names only a reservation
                // still satisfies the "one of the two" rule.
                "memory": "1024",
                "containerDefinitions": [definition],
            }),
            case,
        )
        .await;

        let body = ok(
            &router,
            "DescribeTaskDefinition",
            json!({ "taskDefinition": family }),
            case,
        )
        .await;
        let stored = list(&body["taskDefinition"], "containerDefinitions", case);
        assert_eq!(
            stored.len(),
            1,
            "case {case:?}: expected one stored container, got {body}"
        );
        for (member, want) in [("memory", memory), ("memoryReservation", reservation)] {
            let actual = stored[0][member].as_i64();
            assert_eq!(
                actual, *want,
                "case {case:?}: expected the stored {member} to be {want:?}, got {actual:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A tag whose characters are outside the set a tag admits
// ---------------------------------------------------------------------------

/// The characters `TagKey`/`TagValue` admit, from their shared `@pattern`
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` and the `Tag` doc ("Generally allowed characters
/// are: letters, numbers, and spaces representable in UTF-8, and the following
/// characters: + - = . _ : / @"). A character outside it earns the tag sentence.
const INVALID_TAG_CHARACTERS: &str = "Some tags contain invalid characters. Valid characters: \
                                      UTF-8 letters, spaces, numbers and _ . / = + - : @.";

/// The `tags` lists whose characters are outside that set. They differ along WHICH
/// character is outside it (a pictograph, punctuation the set omits, a control
/// character), whether the KEY or the VALUE carries it, and where the entry sits.
fn invalid_character_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a pictograph in the value",
            json!([{ "key": "env", "value": "prod-\u{1F389}" }]),
        ),
        (
            "a pictograph in the key",
            json!([{ "key": "env\u{1F389}", "value": "prod" }]),
        ),
        (
            "a per-cent sign in the value",
            json!([{ "key": "usage", "value": "50%" }]),
        ),
        (
            "an exclamation mark in the key",
            json!([{ "key": "owner!", "value": "team-a" }]),
        ),
        (
            "a hash in the value",
            json!([{ "key": "ticket", "value": "#42" }]),
        ),
        (
            "brackets in the value",
            json!([{ "key": "owner", "value": "team(a)" }]),
        ),
        (
            "a newline in the value",
            json!([{ "key": "note", "value": "line\nbreak" }]),
        ),
        (
            "the bad entry sits second in a longer list",
            json!([
                { "key": "env", "value": "prod" },
                { "key": "owner", "value": "team*a" },
                { "key": "tier", "value": "web" },
            ]),
        ),
    ]
}

#[tokio::test]
async fn a_tag_character_outside_the_set_is_refused_with_the_tag_sentence() {
    let router = crate::reserved_tag_prefix::service_with_setup().await;
    let cluster_arn = format!("arn:aws:ecs:{SIGNED_REGION}:{ACCOUNT}:cluster/{TAGGED_CLUSTER}");
    ok(
        &router,
        "TagResource",
        json!({
            "resourceArn": cluster_arn,
            "tags": [{ "key": "env", "value": "prod" }],
        }),
        "setup",
    )
    .await;

    // The rule holds wherever a `tags` list is carried, so every implemented operation
    // that takes one is driven with every row.
    for operation in crate::reserved_tag_prefix::TAGGING_OPERATIONS {
        for (index, (row, tags)) in invalid_character_rows().iter().enumerate() {
            let case = format!("{operation}: {row}");
            let name = if operation == "RegisterContainerInstance" {
                format!("i-{index:016x}")
            } else {
                format!("badchars{index}")
            };
            let (code, message) = refusal(
                &router,
                operation,
                crate::reserved_tag_prefix::request_with_tags(operation, &name, &cluster_arn, tags),
                &case,
            )
            .await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected an {INVALID_PARAMETER} for {tags}, got {code} \
                 with {message:?}"
            );
            assert_eq!(
                message, INVALID_TAG_CHARACTERS,
                "case {case:?}: expected the refusal to read {INVALID_TAG_CHARACTERS:?}, \
                 got {message:?}"
            );

            let records = crate::reserved_tag_prefix::created(&router, operation, &name, &case).await;
            assert!(
                records.is_empty(),
                "case {case:?}: a refused {operation} must create nothing, but {name} now \
                 reads back as {records:?}"
            );
        }
    }

    // And the refused writes left the cluster's own tag set exactly as it was.
    let kept = listed(&router, &cluster_arn, "after").await;
    assert_eq!(
        kept,
        vec![("env".to_string(), "prod".to_string())],
        "case \"after\": a refused TagResource writes nothing, got {kept:?}"
    );
}

#[tokio::test]
async fn a_tag_of_characters_the_set_admits_is_stored_as_given() {
    let router = service();

    // The accepted side: letters of other scripts, every punctuation character the set
    // names, a space and a digit — read back through `ListTagsForResource`.
    let rows: [(&str, Value); 4] = [
        (
            "letters outside ASCII",
            json!([{ "key": "\u{65E5}\u{672C}\u{8A9E}", "value": "\u{30C6}\u{30B9}\u{30C8}" }]),
        ),
        (
            "every punctuation character the set names",
            json!([{ "key": "a_b.c:d/e=f+g-h@i", "value": "j_k.l:m/n=o+p-q@r" }]),
        ),
        (
            "spaces and digits",
            json!([{ "key": "cost center", "value": "42 42" }]),
        ),
        (
            "an empty value, which the member's length admits",
            json!([{ "key": "empty", "value": "" }]),
        ),
    ];

    for (index, (case, tags)) in rows.iter().enumerate() {
        let cluster = format!("goodchars{index}");
        let body = ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": cluster, "tags": tags }),
            case,
        )
        .await;
        let arn = string(&body["cluster"], "clusterArn", case);
        let actual = listed(&router, &arn, case).await;
        let want = crate::resource_tags::pairs(
            tags.as_array().expect("the row's tags are a list"),
            case,
        );
        assert_eq!(
            actual, want,
            "case {case:?}: expected {arn} to carry {want:?}, got {actual:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A capacity-provider-strategy base or weight outside its range
// ---------------------------------------------------------------------------

/// The sentence a strategy entry's `member` outside its range earns, with the range the
/// member's own `@range` trait states (`base` 0..100000, `weight` 0..1000).
fn strategy_value_sentence(member: &str, value: &str, min: i64, max: i64) -> String {
    format!(
        "The specified {member} value '{value}' is invalid. Specify an integer between \
         {min} and {max} and try again."
    )
}

#[tokio::test]
async fn a_strategy_base_or_weight_outside_its_range_is_refused_in_the_strategys_own_words() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({ "clusterName": "strategybounds" }),
        "setup",
    )
    .await;

    // One row per (member, bound crossed, distance past it): just past each published
    // bound and far past it, for both members of the entry.
    let rows: [(&str, &str, i64, i64, i64); 6] = [
        ("a base one below its minimum", "base", -1, 0, 100_000),
        ("a base far below its minimum", "base", -100_000, 0, 100_000),
        ("a base one above its maximum", "base", 100_001, 0, 100_000),
        ("a weight one below its minimum", "weight", -1, 0, 1_000),
        ("a weight one above its maximum", "weight", 1_001, 0, 1_000),
        ("a weight far above its maximum", "weight", 100_000, 0, 1_000),
    ];

    for (case, member, value, min, max) in rows {
        let mut entry = json!({ "capacityProvider": "FARGATE", "weight": 1 });
        entry[member] = json!(value);
        let want = strategy_value_sentence(member, &value.to_string(), min, max);

        // Both the member that sets a cluster's default strategy and the one that places
        // a task carry the same entry shape, so both are driven.
        for (operation, request) in [
            (
                "PutClusterCapacityProviders",
                json!({
                    "cluster": "strategybounds",
                    "capacityProviders": ["FARGATE"],
                    "defaultCapacityProviderStrategy": [entry.clone()],
                }),
            ),
            (
                "CreateCluster",
                json!({
                    "clusterName": "strategybounds2",
                    "capacityProviders": ["FARGATE"],
                    "defaultCapacityProviderStrategy": [entry.clone()],
                }),
            ),
        ] {
            let case = format!("{operation}: {case}");
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected an {INVALID_PARAMETER}, got {code} with {message:?}"
            );
            assert_eq!(
                message, want,
                "case {case:?}: expected the refusal to read {want:?}, got {message:?}"
            );
        }
    }

    // Each published bound ITSELF is accepted and stored. One cluster per row: an
    // association starts an attachments update, and a cluster whose attachments are
    // updating takes no second association.
    for (case, base, weight) in [
        ("both minimums", 0, 0),
        ("both maximums", 100_000, 1_000),
        ("the maximum base with the minimum weight", 100_000, 0),
    ] {
        let cluster = format!("bound-{base}-{weight}");
        ok(
            &router,
            "CreateCluster",
            json!({ "clusterName": cluster }),
            case,
        )
        .await;
        let answered = ok(
            &router,
            "PutClusterCapacityProviders",
            json!({
                "cluster": cluster,
                "capacityProviders": ["FARGATE"],
                "defaultCapacityProviderStrategy": [
                    { "capacityProvider": "FARGATE", "base": base, "weight": weight },
                ],
            }),
            case,
        )
        .await;
        let strategy = list(
            &answered["cluster"],
            "defaultCapacityProviderStrategy",
            case,
        );
        assert_eq!(
            strategy.len(),
            1,
            "case {case:?}: expected the one entry back, got {answered}"
        );
        assert_eq!(
            (
                count(&strategy[0], "base", case),
                count(&strategy[0], "weight", case)
            ),
            (base, weight),
            "case {case:?}: expected the bound values to be stored, got {answered}"
        );
    }
}

// ---------------------------------------------------------------------------
// A capacity provider that does not exist
// ---------------------------------------------------------------------------

/// The sentence a request naming a capacity provider this account holds none of earns.
const CAPACITY_PROVIDER_NOT_FOUND: &str = "The specified capacity provider was not found. \
                                           Specify a valid capacity provider and try again.";

#[tokio::test]
async fn a_capacity_provider_that_does_not_exist_is_refused_and_changes_no_association() {
    let router = service();
    create_auto_scaling_group(&router, "asg-real", "setup").await;
    ok(
        &router,
        "CreateCapacityProvider",
        json!({
            "name": "real-provider",
            "autoScalingGroupProvider": { "autoScalingGroupArn": "asg-real" },
        }),
        "setup",
    )
    .await;
    // The association the refused calls must leave alone, written by the create so no
    // attachments update is in flight while the rows run.
    ok(
        &router,
        "CreateCluster",
        json!({
            "clusterName": "unknownprovider",
            "capacityProviders": ["real-provider"],
            "defaultCapacityProviderStrategy": [
                { "capacityProvider": "real-provider", "weight": 1 },
            ],
        }),
        "setup",
    )
    .await;
    let before = described_cluster(&router, "unknownprovider", json!(["ATTACHMENTS"]), "setup").await;

    // The rows differ along which list carries the unknown name, how many names the list
    // holds, and what the name looks like — a name nothing was created under, a deleted
    // provider's name, and a Fargate-looking name that is not one of the two.
    let rows: [(&str, Value, Value); 5] = [
        (
            "an unknown name in both lists",
            json!(["never-created"]),
            json!([{ "capacityProvider": "never-created", "weight": 1 }]),
        ),
        (
            "an unknown name beside a real one",
            json!(["real-provider", "never-created"]),
            json!([{ "capacityProvider": "real-provider", "weight": 1 }]),
        ),
        (
            "an unknown name in the strategy only",
            json!(["real-provider", "also-never-created"]),
            json!([{ "capacityProvider": "also-never-created", "weight": 1 }]),
        ),
        (
            "a name that only looks like a managed provider",
            json!(["FARGATE_SPOTS"]),
            json!([{ "capacityProvider": "FARGATE_SPOTS", "weight": 1 }]),
        ),
        (
            "an unknown name with no strategy at all",
            json!(["still-never-created"]),
            json!([]),
        ),
    ];

    for (case, providers, strategy) in rows {
        let (code, message) = refusal(
            &router,
            "PutClusterCapacityProviders",
            json!({
                "cluster": "unknownprovider",
                "capacityProviders": providers,
                "defaultCapacityProviderStrategy": strategy,
            }),
            case,
        )
        .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected an {INVALID_PARAMETER}, got {code} with {message:?}"
        );
        assert_eq!(
            message, CAPACITY_PROVIDER_NOT_FOUND,
            "case {case:?}: expected the refusal to read {CAPACITY_PROVIDER_NOT_FOUND:?}, \
             got {message:?}"
        );

        // The association is exactly what it was: a refused put writes nothing.
        let after = described_cluster(&router, "unknownprovider", json!(["ATTACHMENTS"]), case).await;
        for member in ["capacityProviders", "defaultCapacityProviderStrategy"] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: expected {member} to be untouched at {}, got {}",
                before[member], after[member]
            );
        }
    }

    // A create that names one is refused the same way, and creates no cluster.
    let case = "CreateCluster names an unknown provider";
    let (code, message) = refusal(
        &router,
        "CreateCluster",
        json!({
            "clusterName": "withunknownprovider",
            "capacityProviders": ["never-created"],
        }),
        case,
    )
    .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case {case:?}: expected an {INVALID_PARAMETER}, got {code} with {message:?}"
    );
    assert_eq!(
        message, CAPACITY_PROVIDER_NOT_FOUND,
        "case {case:?}: expected the refusal to read {CAPACITY_PROVIDER_NOT_FOUND:?}, got \
         {message:?}"
    );
    let body = ok(
        &router,
        "DescribeClusters",
        json!({ "clusters": ["withunknownprovider"] }),
        case,
    )
    .await;
    assert!(
        list(&body, "clusters", case).is_empty(),
        "case {case:?}: a refused create leaves no cluster, got {body}"
    );

    // Each list a create carries is capped at the same quota the later association is
    // capped at, before any name is resolved against the store.
    let case = "CreateCluster one capacity provider past the quota";
    let past_quota: Vec<String> = (0..=MAX_CLUSTER_CAPACITY_PROVIDERS)
        .map(|index| format!("quota-{index}"))
        .collect();
    let (code, message) = refusal(
        &router,
        "CreateCluster",
        json!({ "clusterName": "pastquota", "capacityProviders": past_quota }),
        case,
    )
    .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case {case:?}: expected an {INVALID_PARAMETER}, got {code} with {message:?}"
    );
    let want = format!("capacity providers can have at most {MAX_CLUSTER_CAPACITY_PROVIDERS} items.");
    assert_eq!(
        message, want,
        "case {case:?}: expected the refusal to read {want:?}, got {message:?}"
    );

    // And the providers that DO exist — a created one and each of the two Amazon ECS
    // manages — are associated without complaint.
    for (case, name) in [
        ("a created provider", "real-provider"),
        ("the Fargate provider", "FARGATE"),
        ("the Fargate Spot provider", "FARGATE_SPOT"),
    ] {
        let cluster = format!("holder-{name}");
        ok(
            &router,
            "CreateCluster",
            json!({
                "clusterName": cluster,
                "capacityProviders": [name],
                "defaultCapacityProviderStrategy": [
                    { "capacityProvider": name, "weight": 1 },
                ],
            }),
            case,
        )
        .await;
        let read = described_cluster(&router, &cluster, json!(["ATTACHMENTS"]), case).await;
        assert_eq!(
            list(&read, "capacityProviders", case),
            vec![Value::String(name.to_string())],
            "case {case:?}: expected the association to hold {name}, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// A cluster setting value outside the supported set
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_setting_value_outside_the_set_is_refused_naming_the_member() {
    let router = service();
    ok(
        &router,
        "CreateCluster",
        json!({
            "clusterName": "settingvalues",
            "settings": [{ "name": "containerInsights", "value": "enabled" }],
        }),
        "setup",
    )
    .await;

    /// The sentence a setting whose value is outside the set earns: the member of the
    /// setting structure, and nothing of the value.
    const INVALID_SETTING: &str = "Invalid setting 'value'";

    // The rows differ along the value that is outside the set: a word of its own, the
    // right word in the wrong case, a value another ECS setting DOES take, and the empty
    // string.
    let values = [
        "not-a-real-value",
        "ENABLED",
        "Enhanced",
        "on",
        "off",
        "",
        "enabled ",
    ];

    for value in values {
        for (operation, request) in [
            (
                "UpdateClusterSettings",
                json!({
                    "cluster": "settingvalues",
                    "settings": [{ "name": "containerInsights", "value": value }],
                }),
            ),
            (
                "UpdateCluster",
                json!({
                    "cluster": "settingvalues",
                    "settings": [{ "name": "containerInsights", "value": value }],
                }),
            ),
            (
                "CreateCluster",
                json!({
                    "clusterName": "settingvalues2",
                    "settings": [{ "name": "containerInsights", "value": value }],
                }),
            ),
        ] {
            let case = format!("{operation} with {value:?}");
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected an {INVALID_PARAMETER}, got {code} with {message:?}"
            );
            assert_eq!(
                message, INVALID_SETTING,
                "case {case:?}: expected the refusal to read {INVALID_SETTING:?}, got \
                 {message:?}"
            );
        }
    }

    // The cluster still carries the setting it was created with: a refused update wrote
    // nothing.
    let read = described_cluster(&router, "settingvalues", json!(["SETTINGS"]), "after").await;
    let settings = list(&read, "settings", "after");
    assert_eq!(
        settings.len(),
        1,
        "case \"after\": expected the one stored setting, got {read}"
    );
    assert_eq!(
        (
            string(&settings[0], "name", "after"),
            string(&settings[0], "value", "after")
        ),
        ("containerInsights".to_string(), "enabled".to_string()),
        "case \"after\": a refused settings update writes nothing, got {read}"
    );
}

// ---------------------------------------------------------------------------
// A deployment configuration percentage outside its range
// ---------------------------------------------------------------------------

/// The cluster the service rules below act in.
const SERVICE_CLUSTER: &str = "deployconfig";

/// The family that cluster's services run.
const SERVICE_FAMILY: &str = "deployconfigfamily";

/// A router with one cluster that has capacity and one revision to run.
async fn service_fixture() -> MultiServiceRouter {
    let router = service();
    cluster_with_capacity(&router, SERVICE_CLUSTER, "setup").await;
    register_definition(&router, SERVICE_FAMILY, "setup").await;
    router
}

/// The `deploymentConfiguration` `DescribeServices` reports for one service.
async fn deployment_configuration_of(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> Value {
    let body = ok(
        router,
        "DescribeServices",
        json!({ "cluster": SERVICE_CLUSTER, "services": [name] }),
        case,
    )
    .await;
    let services = list(&body, "services", case);
    assert_eq!(
        services.len(),
        1,
        "case {case:?}: expected exactly one service back, got {body}"
    );
    services[0]["deploymentConfiguration"].clone()
}

#[tokio::test]
async fn a_minimum_healthy_percent_outside_its_range_is_refused_by_create_and_update() {
    let router = service_fixture().await;

    // The service both halves of the rule are checked against.
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": SERVICE_CLUSTER,
            "serviceName": "percents",
            "taskDefinition": SERVICE_FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        "setup",
    )
    .await;
    let before = deployment_configuration_of(&router, "percents", "setup").await;

    // The rows differ along which bound is crossed, how far past it the value is, and
    // what else the configuration carries.
    let rows: [(&str, Value, &str); 5] = [
        (
            "one past the ceiling",
            json!({ "minimumHealthyPercent": 101, "maximumPercent": 200 }),
            "minimumHealthyPercent must be at most 100",
        ),
        (
            "half again past the ceiling",
            json!({ "minimumHealthyPercent": 150, "maximumPercent": 100 }),
            "minimumHealthyPercent must be at most 100",
        ),
        (
            "far past the ceiling, with nothing else named",
            json!({ "minimumHealthyPercent": 100_000 }),
            "minimumHealthyPercent must be at most 100",
        ),
        (
            "one below the floor",
            json!({ "minimumHealthyPercent": -1 }),
            "minimumHealthyPercent must be at least 0",
        ),
        (
            "far below the floor, beside a legal maximum",
            json!({ "minimumHealthyPercent": -1000, "maximumPercent": 200 }),
            "minimumHealthyPercent must be at least 0",
        ),
    ];

    for (index, (case, configuration, want)) in rows.iter().enumerate() {
        for (operation, request) in [
            (
                "UpdateService",
                json!({
                    "cluster": SERVICE_CLUSTER,
                    "service": "percents",
                    "deploymentConfiguration": configuration,
                }),
            ),
            (
                "CreateService",
                json!({
                    "cluster": SERVICE_CLUSTER,
                    "serviceName": format!("percents{index}"),
                    "taskDefinition": SERVICE_FAMILY,
                    "desiredCount": 0,
                    "launchType": "EC2",
                    "deploymentConfiguration": configuration,
                }),
            ),
        ] {
            let case = format!("{operation}: {case}");
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected an {INVALID_PARAMETER}, got {code} with {message:?}"
            );
            assert_eq!(
                &message, want,
                "case {case:?}: expected the refusal to read {want:?}, got {message:?}"
            );
        }

        // The refused create minted no service.
        let body = ok(
            &router,
            "DescribeServices",
            json!({
                "cluster": SERVICE_CLUSTER,
                "services": [format!("percents{index}")],
            }),
            case,
        )
        .await;
        assert!(
            list(&body, "services", case).is_empty(),
            "case {case:?}: a refused create leaves no service, got {body}"
        );
    }

    // And the refused updates left the stored configuration where it was.
    let after = deployment_configuration_of(&router, "percents", "after").await;
    assert_eq!(
        after, before,
        "case \"after\": a refused update writes nothing, expected {before}, got {after}"
    );
}

#[tokio::test]
async fn a_minimum_healthy_percent_inside_its_range_is_stored_and_read_back() {
    let router = service_fixture().await;

    // Every value inside the range the rule admits, each published bound included.
    for (index, percent) in [0_i64, 1, 50, 99, 100].into_iter().enumerate() {
        let case = &format!("minimumHealthyPercent {percent}");
        let name = format!("inrange{index}");
        ok(
            &router,
            "CreateService",
            json!({
                "cluster": SERVICE_CLUSTER,
                "serviceName": name,
                "taskDefinition": SERVICE_FAMILY,
                "desiredCount": 0,
                "launchType": "EC2",
                "deploymentConfiguration": { "minimumHealthyPercent": percent },
            }),
            case,
        )
        .await;
        let created = deployment_configuration_of(&router, &name, case).await;
        assert_eq!(
            count(&created, "minimumHealthyPercent", case),
            percent,
            "case {case:?}: expected the created service to store the percent, got \
             {created}"
        );

        // And the same value through the update path, on the one service that already
        // holds another.
        ok(
            &router,
            "UpdateService",
            json!({
                "cluster": SERVICE_CLUSTER,
                "service": name,
                "deploymentConfiguration": {
                    "minimumHealthyPercent": percent,
                    "maximumPercent": 200,
                },
            }),
            case,
        )
        .await;
        let updated = deployment_configuration_of(&router, &name, case).await;
        assert_eq!(
            (
                count(&updated, "minimumHealthyPercent", case),
                count(&updated, "maximumPercent", case)
            ),
            (percent, 200),
            "case {case:?}: expected the update to store both percents, got {updated}"
        );
    }
}

// ---------------------------------------------------------------------------
// A launch type beside a capacity provider strategy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn naming_both_a_launch_type_and_a_strategy_is_refused_with_one_sentence() {
    let router = service_fixture().await;
    ok(
        &router,
        "PutClusterCapacityProviders",
        json!({
            "cluster": SERVICE_CLUSTER,
            "capacityProviders": ["FARGATE", "FARGATE_SPOT"],
            "defaultCapacityProviderStrategy": [],
        }),
        "setup",
    )
    .await;

    /// The sentence a request naming both earns.
    const BOTH: &str = "Specifying both a launch type and capacity provider strategy is not \
                        supported. Remove one and try again.";

    // The rows differ along the launch type, the provider the strategy names and how many
    // entries it has — and along the operation, since both operations that place work take
    // the pair.
    let rows: [(&str, &str, Value); 3] = [
        (
            "EC2 beside one Fargate entry",
            "EC2",
            json!([{ "capacityProvider": "FARGATE", "weight": 1 }]),
        ),
        (
            "FARGATE beside the spot provider",
            "FARGATE",
            json!([{ "capacityProvider": "FARGATE_SPOT", "weight": 1 }]),
        ),
        (
            "EXTERNAL beside a two-entry strategy",
            "EXTERNAL",
            json!([
                { "capacityProvider": "FARGATE", "weight": 1, "base": 1 },
                { "capacityProvider": "FARGATE_SPOT", "weight": 4 },
            ]),
        ),
    ];

    for (index, (case, launch_type, strategy)) in rows.iter().enumerate() {
        for (operation, request) in [
            (
                "CreateService",
                json!({
                    "cluster": SERVICE_CLUSTER,
                    "serviceName": format!("bothways{index}"),
                    "taskDefinition": SERVICE_FAMILY,
                    "desiredCount": 0,
                    "launchType": launch_type,
                    "capacityProviderStrategy": strategy,
                }),
            ),
            (
                "RunTask",
                json!({
                    "cluster": SERVICE_CLUSTER,
                    "taskDefinition": SERVICE_FAMILY,
                    "launchType": launch_type,
                    "capacityProviderStrategy": strategy,
                }),
            ),
        ] {
            let case = format!("{operation}: {case}");
            let (code, message) = refusal(&router, operation, request, &case).await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case:?}: expected an {INVALID_PARAMETER}, got {code} with {message:?}"
            );
            assert_eq!(
                message, BOTH,
                "case {case:?}: expected the refusal to read {BOTH:?}, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Deleting a revision that is still ACTIVE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_active_revision_fails_its_delete_entry_with_the_deregister_reason() {
    let router = service();

    /// The `Failure$reason` a revision that is still `ACTIVE` earns.
    const STILL_ACTIVE: &str = "The specified task definition is still in ACTIVE status. \
                                Please deregister the target and try again.";

    let family = "activedelete";
    for _ in 1..=3 {
        register_definition(&router, family, "setup").await;
    }

    // The rows differ along HOW the entry names the revision and how many entries the
    // request carries: the failure is per entry, and the ARN it reports is the one the
    // entry pointed at.
    let rows: [(&str, Vec<String>, Vec<String>); 3] = [
        (
            "one entry by family:revision",
            vec![format!("{family}:1")],
            vec![definition_arn(family, 1)],
        ),
        (
            "one entry by ARN",
            vec![definition_arn(family, 2)],
            vec![definition_arn(family, 2)],
        ),
        (
            "two ACTIVE entries in one request",
            vec![format!("{family}:1"), definition_arn(family, 3)],
            vec![definition_arn(family, 1), definition_arn(family, 3)],
        ),
    ];

    for (case, references, expected_arns) in rows {
        let body = ok(
            &router,
            "DeleteTaskDefinitions",
            json!({ "taskDefinitions": references }),
            case,
        )
        .await;
        let failures = list(&body, "failures", case);
        assert_eq!(
            failures.len(),
            expected_arns.len(),
            "case {case:?}: expected one failure per ACTIVE entry, got {body}"
        );
        for (failure, arn) in failures.iter().zip(expected_arns.iter()) {
            // The entry carries the ARN and the reason, and NOTHING else: no detail.
            assert_eq!(
                failure,
                &json!({ "arn": arn, "reason": STILL_ACTIVE }),
                "case {case:?}: expected exactly the arn and the reason, got {failure}"
            );
        }
        assert!(
            list(&body, "taskDefinitions", case).is_empty(),
            "case {case:?}: nothing was deletable, so no revision is reported, got {body}"
        );
    }

    // Deregister the revision and the same call goes through: the reason named the step
    // that clears it.
    let case = "deregistered, then deleted";
    ok(
        &router,
        "DeregisterTaskDefinition",
        json!({ "taskDefinition": format!("{family}:1") }),
        case,
    )
    .await;
    let body = ok(
        &router,
        "DeleteTaskDefinitions",
        json!({ "taskDefinitions": [format!("{family}:1")] }),
        case,
    )
    .await;
    assert!(
        list(&body, "failures", case).is_empty(),
        "case {case:?}: a deregistered revision deletes without a failure, got {body}"
    );
    let deleted = list(&body, "taskDefinitions", case);
    assert_eq!(
        deleted.len(),
        1,
        "case {case:?}: expected the one revision back, got {body}"
    );
    assert_eq!(
        string(&deleted[0], "status", case),
        "DELETE_IN_PROGRESS",
        "case {case:?}: expected the deleted revision to report its new status, got {body}"
    );
}
