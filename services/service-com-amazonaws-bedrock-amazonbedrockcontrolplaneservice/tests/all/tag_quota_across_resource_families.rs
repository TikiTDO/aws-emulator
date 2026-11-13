// SPDX-License-Identifier: CC0-1.0

//! The per-resource tag quota, asserted over the resource FAMILIES a tagging call's
//! `resourceARN` resolves — the Automated Reasoning policy family included.
//!
//! The rule under test, one sentence: **every ARN `TaggableResourcesArn` admits that names
//! a resource this account holds resolves to that resource's own tag list, so `TagResource`
//! answers 200 for a request that leaves it carrying at most the 50 tags
//! `TooManyTagsException` documents — counting the keys it already holds together with the
//! request's — refuses a request past that bound in full with 400 `TooManyTagsException`
//! before any pair is applied, and still admits a later within-quota request on the same
//! resource.**
//!
//! `crate::tagging`'s third suite asserts that bound on two families; these rows are the
//! FAMILY extension of the same rule, driven through the harness and the setup helpers
//! those two suites already own (`crate::tagging`, `crate::automated_reasoning_policy_tagging`)
//! rather than through a second copy of them. Rows differ along what the rule generalises
//! over:
//!
//! * the resource FAMILY the ARN names, and so the record the tags are held on: a
//!   guardrail, the DRAFT of an Automated Reasoning policy, one VERSION cut from it, a
//!   prompt router and an application inference profile — each a family
//!   `TaggableResourcesArn` spells an alternative for
//!   (`smithy_query(query="shape",shape="TaggableResourcesArn")`: `:guardrail/…`,
//!   `:automated-reasoning-policy/[a-zA-Z0-9]+(:[a-zA-Z0-9]+)?`,
//!   `:(inference-profile|application-inference-profile)/…`, `prompt-router`);
//! * how the bound is REACHED: nothing held, one tag held, the bound approached in two
//!   calls, the bound already reached;
//! * how far PAST the bound the refused batch goes: one key too many, several;
//! * whether that refused batch also re-values keys the resource already carries, which do
//!   not grow it.
//!
//! The 404 side of the same resolution (an ARN naming no resource) is
//! `crate::automated_reasoning_policy_tagging`'s third suite; this one is about the quota
//! contract behind the call. Every expectation is derived from the model
//! (`smithy_query(query="operation",operation="TagResource,ListTagsForResource")`).

use serde_json::{json, Value};

use crate::automated_reasoning_policy_tagging::{definition, given_policy, given_version};
use crate::support::{emulator, foundation_model_arn, Emulator, ACCOUNT, REGION};
use crate::tagging::{given_guardrail, list_tags, listed, tag, tag_list, MAX_TAGS};

/// Base model ids this account's published catalog carries (`ListFoundationModels` reports
/// them), used as the target and the fallback of the prompt router and as the source of the
/// application inference profile these rows tag.
const ROUTED_MODEL: &str = "anthropic.claude-3-haiku-20240307-v1:0";
const ROUTED_FALLBACK_MODEL: &str = "anthropic.claude-3-sonnet-20240229-v1:0";

/// `count` distinct tags whose keys start at `start`, as the owned pairs a listing is
/// compared against: these rows build batches at an OFFSET — the keys a resource does not
/// carry yet — and read the same pairs back. Each key and value is inside
/// `TagKey`/`TagValue`'s own pattern (`^[a-zA-Z0-9\s._:/=+@-]*$`).
fn numbered_tags(start: usize, count: usize) -> Vec<(String, String)> {
    (start..start + count)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

/// Owned pairs as the `TagList` a request carries.
fn owned_tag_list(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "key": key, "value": value }))
            .collect(),
    )
}

/// Create a prompt router and hand back the ARN a TAGGING call addresses it by.
///
/// A router is minted under `PromptRouterArn`'s `default-prompt-router/<id>` spelling, and
/// `TaggableResourcesArn` spells the same router `prompt-router/<id>`, so that second
/// spelling is the one a tagging call can carry for it.
async fn given_prompt_router(emu: &Emulator, index: usize) -> String {
    let reply = emu
        .call(
            "POST",
            "/prompt-routers",
            Some(json!({
                "promptRouterName": format!("tag quota router {index}"),
                "models": [{ "modelArn": foundation_model_arn(REGION, ROUTED_MODEL) }],
                "fallbackModel": {
                    "modelArn": foundation_model_arn(REGION, ROUTED_FALLBACK_MODEL),
                },
                "routingCriteria": { "responseQualityDifference": 5.0 },
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case setup {index}: CreatePromptRouter must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    let minted = reply.text("promptRouterArn");
    let addressed = minted.replace(":default-prompt-router/", ":prompt-router/");
    assert_ne!(
        addressed, minted,
        "case setup {index}: a minted router ARN carries the default-prompt-router spelling, \
         got {minted:?}"
    );
    addressed
}

/// Create an application inference profile and hand back its `inferenceProfileArn`.
async fn given_inference_profile(emu: &Emulator, index: usize) -> String {
    let reply = emu
        .call(
            "POST",
            "/inference-profiles",
            Some(json!({
                "inferenceProfileName": format!("tag quota profile {index}"),
                "modelSource": { "copyFrom": foundation_model_arn(REGION, ROUTED_MODEL) },
            })),
        )
        .await;
    assert_eq!(
        reply.status, 201,
        "case setup {index}: CreateInferenceProfile must answer 201, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("inferenceProfileArn")
}

/// Create an Automated Reasoning policy carrying no tags and hand back the unversioned ARN
/// that names its DRAFT.
async fn given_policy_draft(emu: &Emulator, index: usize) -> String {
    let (policy_arn, _) = given_policy(
        emu,
        json!({
            "name": format!("tag quota policy {index}"),
            "policyDefinition": definition(1),
        }),
    )
    .await;
    assert!(
        policy_arn.starts_with(&format!(
            "arn:aws:bedrock:{REGION}:{ACCOUNT}:automated-reasoning-policy/"
        )),
        "case setup {index}: a minted policy ARN must be of this account and region, got \
         {policy_arn:?}"
    );
    policy_arn
}

/// One of the resource families a tagging `resourceARN` resolves, each an alternative
/// `TaggableResourcesArn` spells.
#[derive(Clone, Copy, Debug)]
enum TaggedFamily {
    Guardrail,
    PolicyDraft,
    PolicyVersion,
    PromptRouter,
    InferenceProfile,
}

impl TaggedFamily {
    /// How a case names the family it addresses.
    fn label(self) -> &'static str {
        match self {
            TaggedFamily::Guardrail => "a guardrail",
            TaggedFamily::PolicyDraft => "the draft of an Automated Reasoning policy",
            TaggedFamily::PolicyVersion => "a version of an Automated Reasoning policy",
            TaggedFamily::PromptRouter => "a prompt router",
            TaggedFamily::InferenceProfile => "an application inference profile",
        }
    }

    /// Create one resource of this family, carrying no tags, and hand back the ARN a
    /// tagging call addresses it by.
    async fn given(self, emu: &Emulator, index: usize) -> String {
        match self {
            TaggedFamily::Guardrail => {
                given_guardrail(emu, &format!("tag-quota-guardrail-{index}")).await
            }
            TaggedFamily::PolicyDraft => given_policy_draft(emu, index).await,
            TaggedFamily::PolicyVersion => {
                let policy_arn = given_policy_draft(emu, index).await;
                given_version(emu, &policy_arn, &[]).await
            }
            TaggedFamily::PromptRouter => given_prompt_router(emu, index).await,
            TaggedFamily::InferenceProfile => given_inference_profile(emu, index).await,
        }
    }
}

/// One way of reaching the bound, and one refused batch past it.
struct QuotaRow {
    case: &'static str,
    /// How many tags the resource holds before the refused batch, and whether they are
    /// associated in one call or two.
    held: usize,
    held_in_two_calls: bool,
    /// How many keys past the bound the refused batch goes.
    over_by: usize,
    /// Whether the refused batch ALSO re-values a key the resource already carries — a key
    /// that does not grow it, so the refusal is owed to the fresh keys alone.
    revalues_a_held_key: bool,
}

/// The rows every family is driven through.
const QUOTA_ROWS: &[QuotaRow] = &[
    QuotaRow {
        case: "nothing held, one key too many in one batch",
        held: 0,
        held_in_two_calls: false,
        over_by: 1,
        revalues_a_held_key: false,
    },
    QuotaRow {
        case: "one tag held, several keys too many",
        held: 1,
        held_in_two_calls: false,
        over_by: 7,
        revalues_a_held_key: false,
    },
    QuotaRow {
        case: "the bound approached over two calls, one key too many",
        held: MAX_TAGS - 10,
        held_in_two_calls: true,
        over_by: 1,
        revalues_a_held_key: false,
    },
    QuotaRow {
        case: "the bound already reached, one key too many",
        held: MAX_TAGS,
        held_in_two_calls: true,
        over_by: 1,
        revalues_a_held_key: false,
    },
    QuotaRow {
        case: "a batch whose held keys do not count but whose fresh keys do",
        held: MAX_TAGS - 1,
        held_in_two_calls: false,
        over_by: 1,
        revalues_a_held_key: true,
    },
];

/// Drive every row of [`QUOTA_ROWS`] against one resource family, each row on a resource of
/// its own.
async fn the_quota_contract_holds_for(family: TaggedFamily) {
    let emu = emulator();
    for (index, row) in QUOTA_ROWS.iter().enumerate() {
        let case = format!("{}: {}", family.label(), row.case);
        let arn = family.given(&emu, index).await;

        // The within-quota association the resource starts from, in one call or two.
        let held = numbered_tags(0, row.held);
        if row.held_in_two_calls && row.held > 1 {
            let split = row.held / 2;
            for (part, batch) in [("first", &held[..split]), ("second", &held[split..])] {
                let applied = tag(&emu, &arn, owned_tag_list(batch)).await;
                assert_eq!(
                    applied.status, 200,
                    "case {case:?}: the {part} within-quota batch of {} tags must answer 200, \
                     got {} {:?}",
                    batch.len(),
                    applied.status,
                    applied.body
                );
            }
        } else if row.held > 0 {
            let applied = tag(&emu, &arn, owned_tag_list(&held)).await;
            assert_eq!(
                applied.status, 200,
                "case {case:?}: a within-quota batch of {} tags must answer 200, got {} {:?}",
                row.held, applied.status, applied.body
            );
        }
        // A listing reports one value per key in the order the keys were first associated —
        // the ordering `crate::tagging`'s first suite asserts — so the held set reads back
        // as the batches spelled it.
        let before = listed(&case, &list_tags(&emu, &arn).await);
        assert_eq!(
            before, held,
            "case {case:?}: the listing must report exactly the tags associated"
        );

        // A batch that would push the resource past the bound: refused in full. Only keys
        // the resource does NOT carry grow it, so a re-valued held key in the same batch
        // does not change the count.
        let fresh_count = MAX_TAGS - row.held + row.over_by;
        let mut refused_batch = numbered_tags(row.held, fresh_count);
        if row.revalues_a_held_key {
            assert!(
                !held.is_empty(),
                "case {case:?}: a row re-valuing a held key must hold one"
            );
            refused_batch.push((held[0].0.clone(), "a new value".to_string()));
        }
        let refused = tag(&emu, &arn, owned_tag_list(&refused_batch)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: {fresh_count} fresh keys over {} held is {} past the bound of \
             {MAX_TAGS}, so the request must be refused 400, got {} {:?}",
            row.held, row.over_by, refused.status, refused.body
        );
        assert_eq!(
            refused.error.as_deref(),
            Some("TooManyTagsException"),
            "case {case:?}: the refusal is the modelled TooManyTagsException, got {:?}",
            refused.error
        );
        // `TooManyTagsException$resourceName` is "the name of the resource with too many
        // tags", a `TaggableResourcesArn` — so it is the ARN the request addressed.
        assert_eq!(
            refused.get("resourceName").as_str(),
            Some(arn.as_str()),
            "case {case:?}: the refusal must name the resource addressed, got {:?}",
            refused.body
        );
        let message = refused.get("message");
        let message = message.as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must carry a message, got {:?}",
                refused.body
            )
        });
        assert!(
            message.contains(&MAX_TAGS.to_string()),
            "case {case:?}: the refusal must state the bound {MAX_TAGS}, got {message:?}"
        );

        // NONE of the refused batch was applied: the listing still reports exactly the
        // prior set, the re-valued held key included.
        let after_refusal = listed(&case, &list_tags(&emu, &arn).await);
        assert_eq!(
            after_refusal, before,
            "case {case:?}: a refused batch must apply nothing, so the listing must report the \
             set it found"
        );

        // A within-quota call on the same resource afterwards still succeeds: the refusal
        // left no resource in a state that refuses the next caller. At the bound the only
        // within-quota request is one over keys the resource already carries.
        if row.held < MAX_TAGS {
            let admitted = tag(&emu, &arn, tag_list(&[("after-the-refusal", "ok")])).await;
            assert_eq!(
                admitted.status, 200,
                "case {case:?}: a within-quota TagResource after the refusal must answer 200, \
                 got {} {:?}",
                admitted.status, admitted.body
            );
            let mut want = before.clone();
            want.push(("after-the-refusal".to_string(), "ok".to_string()));
            assert_eq!(
                listed(&case, &list_tags(&emu, &arn).await),
                want,
                "case {case:?}: the tag the admitted call carried must be reported"
            );
        } else {
            let admitted = tag(
                &emu,
                &arn,
                owned_tag_list(&[(before[0].0.clone(), "re-valued".to_string())]),
            )
            .await;
            assert_eq!(
                admitted.status, 200,
                "case {case:?}: re-valuing a held key at the bound must answer 200, got {} {:?}",
                admitted.status, admitted.body
            );
            let mut want = before.clone();
            want[0].1 = "re-valued".to_string();
            assert_eq!(
                listed(&case, &list_tags(&emu, &arn).await),
                want,
                "case {case:?}: a held key must take the new value in place"
            );
        }
    }
}

#[tokio::test]
async fn the_tag_quota_holds_on_a_guardrail() {
    the_quota_contract_holds_for(TaggedFamily::Guardrail).await;
}

#[tokio::test]
async fn the_tag_quota_holds_on_an_automated_reasoning_policy_draft() {
    the_quota_contract_holds_for(TaggedFamily::PolicyDraft).await;
}

#[tokio::test]
async fn the_tag_quota_holds_on_an_automated_reasoning_policy_version() {
    the_quota_contract_holds_for(TaggedFamily::PolicyVersion).await;
}

#[tokio::test]
async fn the_tag_quota_holds_on_a_prompt_router() {
    the_quota_contract_holds_for(TaggedFamily::PromptRouter).await;
}

#[tokio::test]
async fn the_tag_quota_holds_on_an_application_inference_profile() {
    the_quota_contract_holds_for(TaggedFamily::InferenceProfile).await;
}
