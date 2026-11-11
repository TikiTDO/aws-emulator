// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the topic TAG slice: the list `ListTagsForResource` projects
//! out of a topic's stored tags, the three write paths that fill it (`CreateTopic`'s
//! `Tags`, including the re-create that applies more of them, `TagResource` and
//! `UntagResource`), and the two refusals a tag call answers an ARN with.
//!
//! The rule under test: `ListTagsForResource` reports ONE entry per tag the named topic
//! holds — each key paired with its own value — an EMPTY list for a topic that holds
//! none, and the same document on every read; `TagResource` and `UntagResource` are what
//! change that set, a repeated key overwriting rather than duplicating; an ARN that
//! cannot name a topic is refused as bad input (400 `InvalidParameter`) and an
//! ARN-shaped topic ARN that names no topic is a lookup miss (404 `ResourceNotFound`).
//! The rows below differ in the tag COUNT, the key and value CONTENT (reserved
//! prefixes, unicode, an empty value, both published length bounds), the ORDER the tags
//! were written in and the KIND of topic that holds them — which is what the rule
//! generalises over.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! so nothing but persisted state carries information between calls.

use std::collections::BTreeMap;

use super::topics_subscriptions_publish::{text_of, Answer, Sns};

/// "Adding more than 50 tags to a topic isn't recommended" / "Can't add more than 50
/// tags to a topic" (`TagResource` doc and `TagLimitExceededException` doc): the most
/// tags one topic holds.
const MAX_TAGS_PER_TOPIC: usize = 50;

/// "Length Constraints: Minimum length of 1. Maximum length of 128" —
/// `smithy_query(query="shape", shape="TagKey")` reports `length {min: 1, max: 128}`.
const MAX_TAG_KEY_LEN: usize = 128;

/// `smithy_query(query="shape", shape="TagValue")` reports `length {min: 0, max: 256}`:
/// a tag's value may be empty, and is at most this long.
const MAX_TAG_VALUE_LEN: usize = 256;

/// The body of the response's `<Tags>` element, or `None` when the response carries no
/// `Tags` member at all — which is a DIFFERENT document from one carrying an empty
/// list, and the difference is what a client reading `response["Tags"]` depends on.
fn tags_block(body: &str) -> Option<&str> {
    if let Some(start) = body.find("<Tags>") {
        let rest = &body[start + "<Tags>".len()..];
        let end = rest.find("</Tags>")?;
        return Some(&rest[..end]);
    }
    body.contains("<Tags/>").then_some("")
}

/// The key/value pairs the `<Tags>` list carries, in document order, or `None` when the
/// response carries no `Tags` member.
///
/// A list member's open tag carries the envelope's default namespace
/// (`<member xmlns="http://sns.amazonaws.com/doc/2010-03-31/">`), so the opening is
/// matched up to its first `>` rather than as a bare `<member>`.
fn reported_tags(answer: &Answer) -> Option<Vec<(String, String)>> {
    let block = tags_block(&answer.body)?;
    let mut found = Vec::new();
    let mut rest = block;
    while let Some(start) = rest.find("<member") {
        rest = &rest[start..];
        let Some(open) = rest.find('>') else { break };
        rest = &rest[open + 1..];
        let Some(end) = rest.find("</member>") else { break };
        let member = &rest[..end];
        found.push((text_of(member, "Key"), text_of(member, "Value")));
        rest = &rest[end + "</member>".len()..];
    }
    Some(found)
}

/// Create a topic carrying `tags`, with its ARN. The tags travel as the awsQuery
/// spelling of a list of structures (`Tags.member.<n>.Key` / `.Value`).
async fn topic_with_tags(sns: &Sns, case: &str, name: &str, tags: &[(String, String)]) -> String {
    let mut params: Vec<(String, String)> = vec![("Name".to_string(), name.to_string())];
    // The `.fifo` suffix belongs to a FIFO topic and to no other, so a name carrying it
    // is created as the kind it names.
    if name.ends_with(".fifo") {
        params.push(("Attributes.entry.1.key".to_string(), "FifoTopic".to_string()));
        params.push(("Attributes.entry.1.value".to_string(), "true".to_string()));
    }
    for (at, (key, value)) in tags.iter().enumerate() {
        params.push((format!("Tags.member.{}.Key", at + 1), key.clone()));
        params.push((format!("Tags.member.{}.Value", at + 1), value.clone()));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let answer = sns.call("CreateTopic", &borrowed).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: CreateTopic expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer
        .element("TopicArn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateTopic answered no TopicArn: {}", answer.body))
}

/// A tag read, asserting the call was answered, with the pairs it reported.
async fn read_tags(sns: &Sns, case: &str, arn: &str) -> Vec<(String, String)> {
    let answer = sns.call("ListTagsForResource", &[("ResourceArn", arn)]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {} / {}",
        answer.status, answer.body
    );
    reported_tags(&answer).unwrap_or_else(|| {
        panic!(
            "case {case:?}: the response must carry a Tags member, even an empty one, got {}",
            answer.body
        )
    })
}

/// The tag set a topic HOLDS after `written` was applied in order: a key written twice
/// keeps its last value, because "a new tag with a key identical to that of an existing
/// tag overwrites the existing tag" (`TagResource` doc), and the set is keyed, so the
/// order a client wrote them in is not part of it.
fn held(written: &[(String, String)]) -> Vec<(String, String)> {
    written
        .iter()
        .cloned()
        .collect::<BTreeMap<String, String>>()
        .into_iter()
        .collect()
}

/// One row of the projection table: the case, the topic's name, and the tags written.
struct TagCase {
    case: &'static str,
    name: String,
    written: Vec<(String, String)>,
}

/// Borrowed pairs as owned ones, so a row can be spelled with literals.
fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// ListTagsForResource reports exactly the tag set the topic holds — one entry per tag,
/// each key paired with its own value — and reports it identically on every read.
#[tokio::test]
async fn list_tags_for_resource_reports_the_tag_set_the_topic_holds() {
    let sns = Sns::new();
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let fifty: Vec<(String, String)> = (0..MAX_TAGS_PER_TOPIC)
        .map(|at| (format!("key-{at:02}"), format!("value-{at}")))
        .collect();

    let cases: Vec<TagCase> = vec![
        TagCase {
            case: "a topic carrying no tag at all",
            name: "tags-none".to_string(),
            written: Vec::new(),
        },
        TagCase {
            case: "one tag",
            name: "tags-one".to_string(),
            written: pairs(&[("team", "platform")]),
        },
        TagCase {
            case: "several tags written out of key order",
            name: "tags-unordered".to_string(),
            written: pairs(&[("zone", "b"), ("alpha", "1"), ("Mixed", "case")]),
        },
        TagCase {
            case: "a tag whose value is empty, the published value floor",
            name: "tags-empty-value".to_string(),
            written: pairs(&[("owner", "")]),
        },
        TagCase {
            // A key that merely CONTAINS the reserved prefix is not in the namespace it
            // reserves, so it is a key like any other and is reported with the rest.
            case: "a key that merely contains the reserved prefix",
            name: "tags-reserved".to_string(),
            written: pairs(&[
                ("cost:aws:share", "probe-stack"),
                ("team-aws:owner", "ProbeTopic"),
            ]),
        },
        TagCase {
            case: "keys and values at both published length bounds",
            name: "tags-at-bounds".to_string(),
            written: vec![(long_key.clone(), long_value.clone())],
        },
        TagCase {
            case: "non-ascii in the key and in the value",
            name: "tags-unicode".to_string(),
            written: pairs(&[("équipe", "plâtform"), ("チーム", "プラット")]),
        },
        TagCase {
            case: "values carrying the characters the envelope escapes",
            name: "tags-escaped".to_string(),
            written: pairs(&[("document", r#"{"a":"<b> & 'c'"}"#)]),
        },
        TagCase {
            case: "the documented 50-tag maximum",
            name: "tags-fifty".to_string(),
            written: fifty,
        },
        TagCase {
            case: "one key written twice in one request",
            name: "tags-overwritten".to_string(),
            written: pairs(&[("stage", "beta"), ("stage", "prod")]),
        },
        TagCase {
            case: "a FIFO topic",
            name: "tags-ordered.fifo".to_string(),
            written: pairs(&[("team", "payments")]),
        },
    ];

    for TagCase {
        case,
        name,
        written,
    } in &cases
    {
        let arn = topic_with_tags(&sns, case, name, written).await;
        let expected = held(written);
        let reported = read_tags(&sns, case, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the tags {expected:?}, got {reported:?}"
        );
        // A read reports, so a second read of the same topic answers the same document:
        // nothing the first read did can change what the next caller sees.
        let again = read_tags(&sns, case, &arn).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report {reported:?}, got {again:?}"
        );
    }

    // Each topic keeps its OWN tags: the last row's read does not leak into the first
    // row's topic, and a tag set is per resource rather than per account.
    let first = cases.first().expect("the table has rows");
    let untagged = read_tags(&sns, first.case, &sns.topic(&first.name).await).await;
    assert!(
        untagged.is_empty(),
        "case {:?}: an untagged topic must still report an empty list, got {untagged:?}",
        first.case
    );
}

/// The tags a create applies are what a later read reports: the write path and the
/// read path share one record, so a tag is not remembered by the request that set it.
///
/// `CreateTopic` is idempotent ("if the requester already owns a topic with the
/// specified name, that topic's ARN is returned without creating a new topic"), and
/// idempotent means the request asks for what the topic already is — so a re-create
/// carrying the tags the topic holds answers its ARN, one carrying any other tag set is
/// refused, and `TagResource` is what changes the set afterwards.
#[tokio::test]
async fn tags_written_by_a_create_read_back_through_the_tag_list() {
    let sns = Sns::new();
    let case = "a topic created with tags";
    let written = pairs(&[("team", "platform")]);
    let arn = topic_with_tags(&sns, case, "tags-recreated", &written).await;

    let reported = read_tags(&sns, case, &arn).await;
    let expected = held(&written);
    assert_eq!(
        reported, expected,
        "case {case:?}: after the create, expected {expected:?}, got {reported:?}"
    );

    // (case, the tags the re-create carries, whether it is answered)
    let steps: Vec<ReCreateCase> = vec![
        ("no tags at all", Vec::new(), true),
        ("the tags the topic holds", pairs(&[("team", "platform")]), true),
        ("a tag the topic does not hold", pairs(&[("cost-centre", "4711")]), false),
        (
            "another value for a tag it holds",
            pairs(&[("team", "payments")]),
            false,
        ),
    ];

    for (step, tags, answered) in &steps {
        let again = sns
            .call("CreateTopic", &create_params(&tags_params("tags-recreated", tags)))
            .await;
        if *answered {
            assert_eq!(
                again.status, 200,
                "case {step:?}: an idempotent re-create expected 200, got {} / {}",
                again.status, again.body
            );
            assert_eq!(
                again.element("TopicArn").as_deref(),
                Some(arn.as_str()),
                "case {step:?}: the re-create must answer the same ARN, got {}",
                again.body
            );
        } else {
            assert_eq!(
                again.status, 400,
                "case {step:?}: a re-create asking for other tags expected 400, got {} / {}",
                again.status, again.body
            );
            assert_eq!(
                again.code(),
                "InvalidParameter",
                "case {step:?}: expected the InvalidParameter code, got {}",
                again.body
            );
        }
        // The tag set the topic holds is the one the create applied, either way.
        let reported = read_tags(&sns, case, &arn).await;
        assert_eq!(
            reported, expected,
            "case {step:?}: the stored tags must stay {expected:?}, got {reported:?}"
        );
    }

    // And `TagResource` is what adds one afterwards.
    tag(&sns, case, &arn, &pairs(&[("cost-centre", "4711")])).await;
    let reported = read_tags(&sns, case, &arn).await;
    assert_eq!(
        reported,
        held(&pairs(&[("team", "platform"), ("cost-centre", "4711")])),
        "the tag write must add the key the re-create could not, got {reported:?}"
    );
}

/// One re-create row: the case, the tags the request carries, and whether SNS answers it
/// rather than refusing it as a request for a topic the account already holds under
/// other tags.
type ReCreateCase = (&'static str, Vec<(String, String)>, bool);

/// The awsQuery form parameters a `CreateTopic` carrying `tags` on `name` is made of.
fn tags_params(name: &str, tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = vec![("Name".to_string(), name.to_string())];
    for (at, (key, value)) in tags.iter().enumerate() {
        params.push((format!("Tags.member.{}.Key", at + 1), key.clone()));
        params.push((format!("Tags.member.{}.Value", at + 1), value.clone()));
    }
    params
}

/// Borrow an owned parameter list for one call.
fn create_params(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// One row of the refusal table: the case, the ARN read, and the status and awsQuery
/// code it is answered with.
struct RefusalCase {
    case: &'static str,
    resource_arn: String,
    status: u16,
    code: &'static str,
}

/// A `ResourceArn` that could not name a topic in ANY account is bad input — 400
/// `InvalidParameter`, decided before the store is consulted — and only an ARN-shaped
/// topic ARN that names no topic of this account is a lookup miss, 404
/// `ResourceNotFound`. Neither is the off-model `NotImplemented` the scaffold answered.
#[tokio::test]
async fn a_tag_read_refuses_an_arn_that_cannot_name_a_topic() {
    let sns = Sns::new();
    let arn = sns.topic("tags-refusals").await;
    let account = arn.split(':').nth(4).unwrap_or_default().to_string();
    let deleted = sns.topic("tags-deleted").await;
    let dropped = sns.call("DeleteTopic", &[("TopicArn", &deleted)]).await;
    assert_eq!(
        dropped.status, 200,
        "DeleteTopic expected 200, got {} / {}",
        dropped.status, dropped.body
    );

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a value that is not an ARN at all",
            resource_arn: "tags-refusals".to_string(),
            status: 400,
            code: "InvalidParameter",
        },
        RefusalCase {
            case: "an ARN one element short",
            resource_arn: format!("arn:aws:sns:us-east-1:{account}"),
            status: 400,
            code: "InvalidParameter",
        },
        RefusalCase {
            case: "six elements that do not open with arn:",
            resource_arn: format!("urn:aws:sns:us-east-1:{account}:tags-refusals"),
            status: 400,
            code: "InvalidParameter",
        },
        RefusalCase {
            case: "an ARN of another service",
            resource_arn: format!("arn:aws:sqs:us-east-1:{account}:tags-refusals"),
            status: 400,
            code: "InvalidParameter",
        },
        RefusalCase {
            case: "an SNS subscription ARN rather than a topic ARN",
            resource_arn: format!("{arn}:00000000-0000-4000-8000-000000000000"),
            status: 400,
            code: "InvalidParameter",
        },
        RefusalCase {
            case: "an SNS platform endpoint ARN rather than a topic ARN",
            resource_arn: format!("arn:aws:sns:us-east-1:{account}:endpoint/GCM/app/abcd"),
            status: 400,
            code: "InvalidParameter",
        },
        RefusalCase {
            case: "a topic ARN naming no topic of this account",
            resource_arn: format!("arn:aws:sns:us-east-1:{account}:no-such-topic"),
            status: 404,
            code: "ResourceNotFound",
        },
        RefusalCase {
            // A regional endpoint serves the topics of its own region only, so an ARN
            // naming another region is a value this endpoint cannot act on at all rather
            // than a topic that is merely absent here.
            case: "a topic ARN of another region",
            resource_arn: format!("arn:aws:sns:eu-west-1:{account}:tags-refusals"),
            status: 400,
            code: "InvalidParameter",
        },
        RefusalCase {
            case: "the ARN of a topic that was deleted",
            resource_arn: deleted.clone(),
            status: 404,
            code: "ResourceNotFound",
        },
    ];

    // Every tag operation takes the same `ResourceArn` and judges it by the same rule,
    // so each row is driven through all three: the read and both writes.
    let actions: &[(&str, &[(&str, &str)])] = &[
        ("ListTagsForResource", &[]),
        (
            "TagResource",
            &[("Tags.member.1.Key", "team"), ("Tags.member.1.Value", "platform")],
        ),
        ("UntagResource", &[("TagKeys.member.1", "team")]),
    ];

    for RefusalCase {
        case,
        resource_arn,
        status,
        code,
    } in &cases
    {
        for (action, extra) in actions {
            let mut params: Vec<(&str, &str)> = vec![("ResourceArn", resource_arn)];
            params.extend_from_slice(extra);
            let answer = sns.call(action, &params).await;
            assert_eq!(
                answer.status, *status,
                "case {case:?} on {action}: expected {status}, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.code(),
                *code,
                "case {case:?} on {action}: expected the {code:?} code, got {}",
                answer.body
            );
            // The refusal names the member and never reproduces the value the caller
            // sent.
            let message = answer.element("Message").unwrap_or_default();
            assert!(
                !message.contains(resource_arn.as_str()),
                "case {case:?} on {action}: the refusal must not echo the ARN, got \
                 {message:?}"
            );
        }
    }

    // The boundary: the topic's own ARN still reads back, so the refusals above turn
    // away only what cannot name a topic — and no refused write left a tag behind.
    let read = read_tags(&sns, "the topic's own ARN", &arn).await;
    assert!(
        read.is_empty(),
        "an untagged topic must read back as an empty list, got {read:?}"
    );
}

/// One write of the tag table: either tags added by `TagResource` or keys removed by
/// `UntagResource`.
enum TagWrite {
    Add(Vec<(String, String)>),
    Remove(Vec<String>),
}

/// One row of the write table: the case and the write it performs on the one topic.
struct WriteCase {
    case: &'static str,
    write: TagWrite,
}

/// Add tags to a topic, asserting the call was accepted.
async fn tag(sns: &Sns, case: &str, arn: &str, tags: &[(String, String)]) {
    let mut params: Vec<(String, String)> = vec![("ResourceArn".to_string(), arn.to_string())];
    for (at, (key, value)) in tags.iter().enumerate() {
        params.push((format!("Tags.member.{}.Key", at + 1), key.clone()));
        params.push((format!("Tags.member.{}.Value", at + 1), value.clone()));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let answer = sns.call("TagResource", &borrowed).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: TagResource expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// Remove tag keys from a topic, asserting the call was accepted.
async fn untag(sns: &Sns, case: &str, arn: &str, keys: &[String]) {
    let mut params: Vec<(String, String)> = vec![("ResourceArn".to_string(), arn.to_string())];
    for (at, key) in keys.iter().enumerate() {
        params.push((format!("TagKeys.member.{}", at + 1), key.clone()));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let answer = sns.call("UntagResource", &borrowed).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: UntagResource expected 200, got {} / {}",
        answer.status, answer.body
    );
}

/// TagResource and UntagResource are what CHANGE the set a tag read reports: each write
/// is followed by the read that must reflect every write so far, so no tag is remembered
/// by the request that set it.
///
/// The rows are successive writes to ONE topic, differing in the write's kind, its
/// count, whether it repeats a key the topic already holds, whether it removes a key the
/// topic does not hold, and the content of the keys and values.
#[tokio::test]
async fn tag_resource_and_untag_resource_change_the_set_the_tag_read_reports() {
    let sns = Sns::new();
    let arn = sns.topic("tags-written").await;
    let mut held: BTreeMap<String, String> = BTreeMap::new();

    let cases: Vec<WriteCase> = vec![
        WriteCase {
            case: "one tag onto an untagged topic",
            write: TagWrite::Add(pairs(&[("team", "platform")])),
        },
        WriteCase {
            case: "several tags at once",
            write: TagWrite::Add(pairs(&[
                ("cost-centre", "4711"),
                ("stage", "beta"),
                ("owner", ""),
            ])),
        },
        WriteCase {
            // "A new tag with a key identical to that of an existing tag overwrites the
            // existing tag" (`TagResource` doc).
            case: "a key the topic already holds",
            write: TagWrite::Add(pairs(&[("stage", "prod")])),
        },
        WriteCase {
            case: "non-ascii content in the key and the value",
            write: TagWrite::Add(pairs(&[("équipe", "plâtform")])),
        },
        WriteCase {
            case: "one key removed",
            write: TagWrite::Remove(vec!["stage".to_string()]),
        },
        WriteCase {
            case: "a key the topic does not hold",
            write: TagWrite::Remove(vec!["never-set".to_string()]),
        },
        WriteCase {
            case: "several keys removed, one of them not held",
            write: TagWrite::Remove(vec![
                "cost-centre".to_string(),
                "owner".to_string(),
                "never-set".to_string(),
            ]),
        },
        WriteCase {
            case: "a non-ascii key removed",
            write: TagWrite::Remove(vec!["équipe".to_string()]),
        },
        WriteCase {
            case: "every remaining key removed",
            write: TagWrite::Remove(vec!["team".to_string()]),
        },
        WriteCase {
            case: "a tag added again after the topic was emptied",
            write: TagWrite::Add(pairs(&[("team", "payments")])),
        },
    ];

    for WriteCase { case, write } in &cases {
        match write {
            TagWrite::Add(tags) => {
                tag(&sns, case, &arn, tags).await;
                for (key, value) in tags {
                    held.insert(key.clone(), value.clone());
                }
            }
            TagWrite::Remove(keys) => {
                untag(&sns, case, &arn, keys).await;
                for key in keys {
                    held.remove(key);
                }
            }
        }
        let expected: Vec<(String, String)> = held.clone().into_iter().collect();
        let reported = read_tags(&sns, case, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the tags {expected:?}, got {reported:?}"
        );
    }
}

/// A write that would leave a topic holding more than the documented 50 tags is refused
/// with 400 `TagLimitExceeded`, and the topic keeps exactly the set it had: the ceiling
/// is on what the topic WOULD hold, so a write repeating keys it already holds still
/// fits.
#[tokio::test]
async fn a_write_past_the_documented_tag_limit_is_refused_and_changes_nothing() {
    let sns = Sns::new();
    let case = "a topic filled to the documented tag limit";
    let full: Vec<(String, String)> = (0..MAX_TAGS_PER_TOPIC)
        .map(|at| (format!("key-{at:02}"), format!("value-{at}")))
        .collect();
    let arn = topic_with_tags(&sns, case, "tags-at-the-limit", &full).await;
    let expected = held(&full);
    assert_eq!(
        read_tags(&sns, case, &arn).await,
        expected,
        "case {case:?}: the topic must hold every one of the {MAX_TAGS_PER_TOPIC} tags"
    );

    // One more KEY crosses the limit: refused, and nothing is stored.
    let over = pairs(&[("one-too-many", "x")]);
    let mut params: Vec<(&str, &str)> = vec![("ResourceArn", &arn)];
    params.push(("Tags.member.1.Key", over[0].0.as_str()));
    params.push(("Tags.member.1.Value", over[0].1.as_str()));
    let answer = sns.call("TagResource", &params).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: one tag past the limit must be refused 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.code(),
        "TagLimitExceeded",
        "case {case:?}: expected the TagLimitExceeded code, got {}",
        answer.body
    );
    assert_eq!(
        read_tags(&sns, case, &arn).await,
        expected,
        "case {case:?}: a refused write must leave the tag set exactly as it was"
    );

    // A write of the same COUNT that only overwrites keys the topic already holds fits,
    // because the ceiling is on the set the write would leave behind.
    let overwrite: Vec<(String, String)> = full
        .iter()
        .map(|(key, _)| (key.clone(), "rewritten".to_string()))
        .collect();
    tag(&sns, case, &arn, &overwrite).await;
    assert_eq!(
        read_tags(&sns, case, &arn).await,
        held(&overwrite),
        "case {case:?}: a write that only overwrites held keys must be applied"
    );

    // And once a key is removed there is room for a new one again.
    untag(&sns, case, &arn, &["key-00".to_string()]).await;
    tag(&sns, case, &arn, &over).await;
    let mut after: Vec<(String, String)> = overwrite
        .iter()
        .filter(|(key, _)| key != "key-00")
        .cloned()
        .collect();
    after.extend(over.iter().cloned());
    assert_eq!(
        read_tags(&sns, case, &arn).await,
        held(&after),
        "case {case:?}: a tag must fit again once another was removed"
    );
}
