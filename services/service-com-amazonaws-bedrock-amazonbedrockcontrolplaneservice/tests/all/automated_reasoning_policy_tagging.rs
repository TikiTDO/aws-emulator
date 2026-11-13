// SPDX-License-Identifier: CC0-1.0

//! Tagging an Automated Reasoning policy end-to-end, and taking tags away again, over the
//! same `MultiServiceRouter` the emulator boots.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **A tagging call reaches a resource under the ARN that resource's own reads are
//!    addressed by.** An `AutomatedReasoningPolicyArn` is such an ARN — `TaggableResourcesArn`
//!    admits `:automated-reasoning-policy/[a-zA-Z0-9]+(:[a-zA-Z0-9]+)?`
//!    (`smithy_query(query="shape",shape="TaggableResourcesArn")`) — and each spelling names
//!    its own REVISION: the unversioned ARN the policy's draft, the `:<version>` suffix one
//!    version it cut. So `TagResource` associates tags with the revision the ARN names and
//!    `ListTagsForResource` reports them back, beside the tags that revision's own create
//!    carried, and one revision's tags are never another's.
//! 2. **`UntagResource` removes the keys the request names, and only those**, off the very
//!    list `ListTagsForResource` reports — for every resource family a tagging call reaches,
//!    and whatever the key set names: keys the resource carries, a key it does not, all of
//!    them, none of them.
//! 3. **A `resourceARN` naming no resource this account holds is refused 404
//!    `ResourceNotFoundException`** by all three tagging operations alike — an id nothing
//!    minted, a version nothing cut, the minted id in another region.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="TagResource,UntagResource,ListTagsForResource,CreateAutomatedReasoningPolicy,CreateAutomatedReasoningPolicyVersion")`).
//! The requests go through `crate::support` — the ONE harness every suite in this crate
//! drives the emulator with.

use serde_json::{json, Value};

use crate::support::{emulator, label, Emulator, Reply, ACCOUNT, REGION};

// ---------------------------------------------------------------------------------------
// Requests.
// ---------------------------------------------------------------------------------------

/// `TagResource` is bound to `POST /tagResource`.
async fn tag(emu: &Emulator, resource_arn: &str, tags: &[(&str, &str)]) -> Reply {
    emu.call(
        "POST",
        "/tagResource",
        Some(json!({ "resourceARN": resource_arn, "tags": tag_list(tags) })),
    )
    .await
}

/// `UntagResource` is bound to `POST /untagResource`, with `tagKeys` a `TagKeyList`.
async fn untag(emu: &Emulator, resource_arn: &str, keys: &[&str]) -> Reply {
    emu.call(
        "POST",
        "/untagResource",
        Some(json!({ "resourceARN": resource_arn, "tagKeys": keys })),
    )
    .await
}

/// `ListTagsForResource` is bound to `POST /listTagsForResource`.
async fn list_tags(emu: &Emulator, resource_arn: &str) -> Reply {
    emu.call(
        "POST",
        "/listTagsForResource",
        Some(json!({ "resourceARN": resource_arn })),
    )
    .await
}

/// A `TagList` out of `(key, value)` pairs.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------
// Responses.
// ---------------------------------------------------------------------------------------

/// The tags a listing reported, in the order it reported them.
fn listed(case: &str, reply: &Reply) -> Vec<(String, String)> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let tags = reply.get("tags");
    let rows = tags.as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: tags must be a present array — a resource with no tags carries \
             an EMPTY one — got {:?}",
            reply.body
        )
    });
    rows.iter()
        .map(|row| {
            let key = row
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no key: {row:?}"))
                .to_string();
            let value = row
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a tag has no value: {row:?}"))
                .to_string();
            (key, value)
        })
        .collect()
}

/// `pairs` as the owned pairs a listing is compared against.
fn expected(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

// ---------------------------------------------------------------------------------------
// Resources the rows address.
// ---------------------------------------------------------------------------------------

/// A policy definition with `count` types, variables and rules, as the definition shapes
/// spell them (`smithy_query(query="closure",operation="CreateAutomatedReasoningPolicy")`:
/// a type name is `^\p{L}[\p{L}\p{M}0-9_]*( [\p{L}\p{M}0-9_]+)*$`, a rule id is
/// `^[A-Z][0-9A-Z]{11}$`).
pub(crate) fn definition(count: usize) -> Value {
    let types: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("Type{index}"),
                "description": format!("type {index}"),
                "values": [{ "value": format!("Value{index}A") }],
            })
        })
        .collect();
    let variables: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "name": format!("variable{index}"),
                "type": format!("Type{index}"),
                "description": format!("variable {index}"),
            })
        })
        .collect();
    let rules: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "id": format!("R{:011}", index),
                "expression": format!("variable{index} == Value{index}A"),
            })
        })
        .collect();
    json!({ "version": "1", "types": types, "variables": variables, "rules": rules })
}

/// Create a policy and hand back `(policyArn, the definitionHash the create reported)`.
pub(crate) async fn given_policy(emu: &Emulator, body: Value) -> (String, String) {
    let created = emu
        .call("POST", "/automated-reasoning-policies", Some(body))
        .await;
    assert_eq!(
        created.status, 200,
        "case setup: CreateAutomatedReasoningPolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
    (created.text("policyArn"), created.text("definitionHash"))
}

/// `GetAutomatedReasoningPolicy` on `policy_arn`.
async fn get_policy(emu: &Emulator, policy_arn: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/automated-reasoning-policies/{}", label(policy_arn)),
        None,
    )
    .await
}

/// Cut a version of `policy_arn` with `tags` attached, feeding back the `definitionHash` a
/// READ of that policy reports — the concurrency token
/// `CreateAutomatedReasoningPolicyVersion` documents — and hand back the versioned ARN.
pub(crate) async fn given_version(emu: &Emulator, policy_arn: &str, tags: &[(&str, &str)]) -> String {
    let read = get_policy(emu, policy_arn).await;
    assert_eq!(
        read.status, 200,
        "case setup: GetAutomatedReasoningPolicy must answer 200, got {} {:?}",
        read.status, read.body
    );
    let cut = emu
        .call(
            "POST",
            &format!(
                "/automated-reasoning-policies/{}/versions",
                label(policy_arn)
            ),
            Some(json!({
                "lastUpdatedDefinitionHash": read.text("definitionHash"),
                "tags": tag_list(tags),
            })),
        )
        .await;
    assert_eq!(
        cut.status, 200,
        "case setup: CreateAutomatedReasoningPolicyVersion must answer 200 for the hash the \
         read reported, got {} {:?}",
        cut.status, cut.body
    );
    cut.text("policyArn")
}

/// Create a guardrail and hand back its `guardrailArn`.
async fn given_guardrail(emu: &Emulator, name: &str) -> String {
    let reply = emu
        .create(json!({
            "name": name,
            "blockedInputMessaging": "blocked in",
            "blockedOutputsMessaging": "blocked out",
        }))
        .await;
    assert_eq!(
        reply.status, 202,
        "case {name}: CreateGuardrail must answer 202, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("guardrailArn")
}

// ---------------------------------------------------------------------------------------
// 1. Tags round-trip on every revision of a policy, each revision's own.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn tags_round_trip_on_every_revision_of_an_automated_reasoning_policy() {
    // Rows differ along what the rule generalises over: WHICH revision the ARN names (the
    // draft, the first version, a later version), what its own create carried (no tags, one
    // tag, several) and what the tagging call then associates (a fresh key, a key the
    // revision already carries, nothing).
    struct Row {
        case: &'static str,
        created_with: &'static [(&'static str, &'static str)],
        associated: &'static [(&'static str, &'static str)],
        reported: &'static [(&'static str, &'static str)],
    }

    let rows = [
        Row {
            case: "a revision created with no tags, one tag associated",
            created_with: &[],
            associated: &[("purpose", "probe")],
            reported: &[("purpose", "probe")],
        },
        Row {
            case: "a revision created with tags, a fresh key associated",
            created_with: &[("env", "probe")],
            associated: &[("owner", "fixtures")],
            reported: &[("env", "probe"), ("owner", "fixtures")],
        },
        Row {
            case: "a key the revision already carries takes the new value in place",
            created_with: &[("env", "probe"), ("owner", "fixtures")],
            associated: &[("env", "live")],
            reported: &[("env", "live"), ("owner", "fixtures")],
        },
        Row {
            case: "an empty tag list associates nothing",
            created_with: &[("env", "probe")],
            associated: &[],
            reported: &[("env", "probe")],
        },
    ];

    let emu = emulator();
    for (index, row) in rows.iter().enumerate() {
        // The three ARN spellings a policy is addressable under, each its own revision:
        // the draft, and two versions cut from it. Each is created carrying this row's
        // tags, so what a tagging call does is read against the list the create filled.
        let mut body = json!({
            "name": format!("tagged policy {index}"),
            "policyDefinition": definition(index % 3),
            "tags": tag_list(row.created_with),
        });
        if index % 2 == 0 {
            body["description"] = json!("a described draft");
        }
        let (policy_arn, _) = given_policy(&emu, body).await;
        let first_version = given_version(&emu, &policy_arn, row.created_with).await;
        let second_version = given_version(&emu, &policy_arn, &[]).await;
        assert_ne!(
            first_version, second_version,
            "case {:?}: each cut version has its own ARN",
            row.case
        );

        for (revision, arn) in [
            ("the draft", policy_arn.clone()),
            ("the first version", first_version.clone()),
        ] {
            let case = format!("{} — {revision}", row.case);

            // What the create carried is what a listing reports before anything is
            // associated.
            assert_eq!(
                listed(&case, &list_tags(&emu, &arn).await),
                expected(row.created_with),
                "case {case:?}: a listing reports the tags that revision's create carried"
            );

            let tagged = tag(&emu, &arn, row.associated).await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: TagResource must answer 200 for an ARN this account holds, \
                 got {} {:?}",
                tagged.status, tagged.body
            );
            assert_eq!(
                tagged.body,
                json!({}),
                "case {case:?}: TagResource has no output members, got {:?}",
                tagged.body
            );

            assert_eq!(
                listed(&case, &list_tags(&emu, &arn).await),
                expected(row.reported),
                "case {case:?}: a listing reports one value per key, the last associated"
            );

            // Repeating the call leaves the revision exactly as the first one left it.
            let again = tag(&emu, &arn, row.associated).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: repeating TagResource must answer 200, got {} {:?}",
                again.status, again.body
            );
            assert_eq!(
                listed(&case, &list_tags(&emu, &arn).await),
                expected(row.reported),
                "case {case:?}: tagging is keyed by key, so a repeat adds no second entry"
            );
        }

        // A revision's tags are its OWN: the version nothing was associated with still
        // reports only what its own cut carried.
        assert_eq!(
            listed(row.case, &list_tags(&emu, &second_version).await),
            expected(&[]),
            "case {:?}: tagging one revision must not reach another, so the untouched \
             version still reports the empty list its cut carried",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. UntagResource removes the keys the request names, and only those.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn untagging_removes_the_named_keys_and_leaves_the_rest() {
    // The tags every row starts from, so a row's removal is read against one known list.
    const HELD: &[(&str, &str)] = &[
        ("env", "probe"),
        ("owner", "fixtures"),
        ("cost:center", "4711"),
    ];

    // Rows differ along the KEY SET the request names: one key the resource carries,
    // several, a key it does not carry, every key, and none at all (`TagKeyList` is
    // `@length(0, 200)`, so the empty list is a request the model admits).
    struct Row {
        case: &'static str,
        removed: &'static [&'static str],
        left: &'static [(&'static str, &'static str)],
    }

    let rows = [
        Row {
            case: "one key the resource carries",
            removed: &["env"],
            left: &[("owner", "fixtures"), ("cost:center", "4711")],
        },
        Row {
            case: "two keys the resource carries",
            removed: &["env", "cost:center"],
            left: &[("owner", "fixtures")],
        },
        Row {
            case: "a key the resource does not carry",
            removed: &["absent"],
            left: HELD,
        },
        Row {
            case: "a key it carries beside one it does not",
            removed: &["absent", "owner"],
            left: &[("env", "probe"), ("cost:center", "4711")],
        },
        Row {
            case: "every key the resource carries",
            removed: &["env", "owner", "cost:center"],
            left: &[],
        },
        Row {
            case: "no key at all",
            removed: &[],
            left: HELD,
        },
    ];

    let emu = emulator();
    for (index, row) in rows.iter().enumerate() {
        // The rule is about the TAGS of a resource, so the rows run over the resource
        // families a tagging call reaches: a guardrail, the draft of an Automated
        // Reasoning policy and one version cut from it.
        let guardrail = given_guardrail(&emu, &format!("untagged-guardrail-{index}")).await;
        let (policy_arn, _) = given_policy(
            &emu,
            json!({
                "name": format!("untagged policy {index}"),
                "policyDefinition": definition(1),
            }),
        )
        .await;
        let version_arn = given_version(&emu, &policy_arn, &[]).await;

        for (family, arn) in [
            ("a guardrail", guardrail),
            ("a policy draft", policy_arn),
            ("a policy version", version_arn),
        ] {
            let case = format!("{family}: {}", row.case);
            let tagged = tag(&emu, &arn, HELD).await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: TagResource must answer 200, got {} {:?}",
                tagged.status, tagged.body
            );

            let removed = untag(&emu, &arn, row.removed).await;
            assert_eq!(
                removed.status, 200,
                "case {case:?}: UntagResource must answer 200 for an ARN this account \
                 holds, got {} {:?}",
                removed.status, removed.body
            );
            assert_eq!(
                removed.body,
                json!({}),
                "case {case:?}: UntagResource has no output members, got {:?}",
                removed.body
            );

            assert_eq!(
                listed(&case, &list_tags(&emu, &arn).await),
                expected(row.left),
                "case {case:?}: a listing must report exactly the keys the untag left"
            );

            // Removing the same keys again changes nothing, and the keys that are gone
            // stay gone: the removal is durable rather than a view over the request.
            let again = untag(&emu, &arn, row.removed).await;
            assert_eq!(
                again.status, 200,
                "case {case:?}: repeating UntagResource must answer 200, got {} {:?}",
                again.status, again.body
            );
            assert_eq!(
                listed(&case, &list_tags(&emu, &arn).await),
                expected(row.left),
                "case {case:?}: a removed key must not come back on a later read"
            );

            // A key the untag removed can be associated again, which is what makes the
            // list the one both operations write.
            let retagged = tag(&emu, &arn, &[("env", "again")]).await;
            assert_eq!(
                retagged.status, 200,
                "case {case:?}: TagResource must answer 200 after an untag, got {} {:?}",
                retagged.status, retagged.body
            );
            let after = listed(&case, &list_tags(&emu, &arn).await);
            assert!(
                after.contains(&("env".to_string(), "again".to_string())),
                "case {case:?}: a key associated after the untag must be reported, got \
                 {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 3. An ARN naming no resource this account holds.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_naming_no_policy_revision_is_refused_by_every_tagging_operation() {
    let emu = emulator();
    let (policy_arn, _) = given_policy(
        &emu,
        json!({ "name": "addressed policy", "policyDefinition": definition(1) }),
    )
    .await;
    let version_arn = given_version(&emu, &policy_arn, &[]).await;
    let prefix = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/");
    let id = policy_arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case setup: ARN {policy_arn:?} is not under {prefix:?}"))
        .to_string();

    // Rows: the ARNs that DO name a revision, and the ones that name none — an id nothing
    // minted, a version nothing cut, and the minted id in another region (a policy belongs
    // to the account-region it was created in).
    let rows: Vec<(&str, String, u16)> = vec![
        ("the draft this account holds", policy_arn.clone(), 200),
        ("the version this policy cut", version_arn, 200),
        ("an id nothing minted", format!("{prefix}00000000000z"), 404),
        (
            "a version nothing cut",
            format!("{policy_arn}:7"),
            404,
        ),
        (
            "the minted id in another region",
            format!("arn:aws:bedrock:eu-west-1:{ACCOUNT}:automated-reasoning-policy/{id}"),
            404,
        ),
    ];

    for (case, arn, status) in &rows {
        for (operation, reply) in [
            ("TagResource", tag(&emu, arn, &[("env", "probe")]).await),
            ("UntagResource", untag(&emu, arn, &["env"]).await),
            ("ListTagsForResource", list_tags(&emu, arn).await),
        ] {
            let case = format!("{operation} on {case}");
            assert_eq!(
                reply.status, *status,
                "case {case:?}: {arn:?} must be answered {status}, got {} {:?}",
                reply.status, reply.body
            );
            if *status == 404 {
                assert_eq!(
                    reply.error.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    reply.error
                );
                // The refusal names the resource TYPE the ARN spells and nothing else, the
                // one sentence every tagging call reports a missing resource with.
                assert_eq!(
                    reply.get("message").as_str(),
                    Some("Automated reasoning policy not found"),
                    "case {case:?}: unexpected message, got {:?}",
                    reply.body
                );
            }
        }
    }
}
