// SPDX-License-Identifier: CC0-1.0

//! The form a TAG KEY has to be in, over every call of this service that takes one.
//!
//! RULE under test: a tag key a request carries has to satisfy the expression the model
//! declares for `TagKey` (`smithy_query(query="shape", shape="TagKey")`:
//! `^(?!aws:)[a-zA-Z+-=._:/]+$`) — so a key that opens with the reserved `aws:` prefix
//! that expression's lookahead excludes, or that carries a character outside its class, is
//! refused with the modelled `ValidationException` (HTTP 400) and NOTHING the call asked
//! for is written. Nothing ahead of a handler applies that expression: the validating walk
//! treats a map's key as a path segment rather than as a constrained leaf, and the
//! expression's lookahead is one its regex engine does not compile, so the rule is the
//! service's own to state wherever a key arrives.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state`: what a resource holds is only believed when a
//! `ListTagsForResource` call reports it back.
//!
//! The table spans what the rule generalises over rather than one key: the reserved prefix
//! (alone, with a suffix, with the colon-separated name AWS's own stack tag carries), a key
//! that merely STARTS with those three letters, the prefix in other cases than the one the
//! expression spells, characters inside the class (its letters, `_`, and every edge of the
//! `+`-to-`=` range it spells) and characters outside it (a space, `@`, `!`, `~`, a
//! non-ASCII letter), and the empty key. Each row is driven through all three entry points
//! a tag key reaches this service by — `RegisterApplication$Tags`, `TagResource$tags` and
//! `UntagResource$tagKeys` — so the rule cannot hold at one of them and not the others.

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::TAG_KEY_PATTERN;

use crate::application_lifecycle::{tag_pairs, valid_registration, Emulator, Reply};
use crate::resource_tags::{application_arn, list_tags, register, registration};
use crate::tag_resource::{pairs, tag};
use crate::untag_resource::untag;
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The region the suite signs for; it is the region every expected ARN carries.
const REGION: &str = "us-east-1";

/// The requirement a refusal states, over the modelled expression with the outer anchors
/// the model spells it with removed — the rendering this service gives every `@pattern`
/// it reports (`tests/all/validation_message_rendering.rs` owns that rule).
fn pattern_requirement() -> String {
    let expression = TAG_KEY_PATTERN
        .strip_prefix('^')
        .and_then(|pattern| pattern.strip_suffix('$'))
        .expect("the modelled TagKey expression carries both outer anchors");
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The whole message a refusal of a `Tags` MAP states: the framework's envelope and
/// clause at the map member, with the requirement its KEYS have to satisfy listed as such
/// and no value quoted, since a map has no scalar rendering to echo.
fn map_refusal() -> String {
    format!(
        "1 validation error detected: Value at 'tags' failed to satisfy constraint: Map keys \
         must satisfy constraint: [{}]",
        pattern_requirement()
    )
}

/// The whole message a refusal of the `tagKeys` LIST states: the clause at the list
/// member, quoting the whole list the caller sent, with the requirement its ELEMENTS have
/// to satisfy nested in it — the wording this service gives every constraint an element of
/// a list breaks.
fn list_refusal(tag_keys: &[String]) -> String {
    format!(
        "1 validation error detected: Value '[{}]' at 'tagKeys' failed to satisfy constraint: \
         Member must satisfy constraint: [{}]",
        tag_keys.join(", "),
        pattern_requirement()
    )
}

/// One tag key, and whether the modelled form admits it.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    key: &'static str,
    /// Whether the key is one the modelled expression admits.
    admitted: bool,
    /// Whether the key can travel as a `tagKeys` query parameter: an EMPTY key cannot —
    /// an empty parameter value carries no element at all — so that row is driven through
    /// the two calls that carry a map.
    in_query: bool,
}

/// A row for a key the form admits.
const fn admitted(name: &'static str, key: &'static str) -> Case {
    Case {
        name,
        key,
        admitted: true,
        in_query: true,
    }
}

/// A row for a key the form refuses.
const fn refused(name: &'static str, key: &'static str) -> Case {
    Case {
        name,
        key,
        admitted: false,
        in_query: true,
    }
}

/// The rows.
fn cases() -> Vec<Case> {
    vec![
        // The reserved prefix the expression's lookahead excludes. The first row is the
        // key AWS's own CloudFormation stack tag carries.
        refused(
            "the reserved prefix with a colon-separated name",
            "aws:cloudformation:stack-name",
        ),
        refused("the reserved prefix alone", "aws:"),
        refused("the reserved prefix with a name", "aws:env"),
        refused(
            "the reserved prefix with the rest of a path",
            "aws:ec2launchtemplate/id",
        ),
        // Only the prefix is reserved, not the letters: the lookahead is `aws:`, so a key
        // that merely starts with them is a key like any other.
        admitted("three letters that are not the prefix", "awsome"),
        admitted("the letters with no colon after them", "aws"),
        admitted("the letters before another separator", "aws.account"),
        // The lookahead the model spells is those four characters, so a key carrying them
        // in another case is outside it and admitted on its class alone.
        admitted("the prefix in upper case", "AWS:env"),
        admitted("the prefix in mixed case", "Aws:env"),
        // Inside the class: its letters, `_`, and the edges and interior of the `+`-to-`=`
        // range its `+-=` spells (`+ , - . / 0-9 : ; < =`).
        admitted("lower-case letters", "env"),
        admitted("upper-case letters", "Owner"),
        admitted("an underscore", "team_name"),
        admitted("a dot", "cost.center"),
        admitted("a slash", "path/segment"),
        admitted("a colon", "name:space"),
        admitted("a hyphen", "a-b"),
        admitted("the low edge of the range", "plus+key"),
        admitted("the high edge of the range", "eq=key"),
        admitted("a comma inside the range", "x,y"),
        admitted("a semicolon inside the range", "a;b"),
        admitted("an angle bracket inside the range", "a<b"),
        admitted("digits inside the range", "tier1"),
        admitted("every class member at once", "A-z_0.9/:;+=,<"),
        // Outside the class.
        refused("a space", "has space"),
        refused("an at sign", "key@value"),
        refused("an exclamation mark", "key!"),
        refused("a tilde", "key~value"),
        refused("a hash", "key#1"),
        refused("a brace", "key{1}"),
        refused("a non-ASCII letter", "clé"),
        refused("an asterisk", "key*"),
        Case {
            // The expression requires at least one character.
            name: "the empty key",
            key: "",
            admitted: false,
            in_query: false,
        },
    ]
}

/// The `Tags` of a `RegisterApplication` body, as a one-entry map holding `key`.
fn tags_member(key: &str) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), json!("v"));
    Value::Object(map)
}

/// Register `application_id` carrying the one tag `key`.
async fn register_with_tag(emulator: &Emulator, application_id: &str, key: &str) -> Reply {
    let mut body: Map<String, Value> = valid_registration(application_id, "HANA")
        .as_object()
        .cloned()
        .expect("a registration body is a JSON object");
    body.insert("Tags".into(), tags_member(key));
    emulator
        .send("/register-application", Value::Object(body))
        .await
}

/// Assert `reply` is the modelled `ValidationException` (HTTP 400) stating `expected`.
fn assert_refused(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected 400, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
    );
    let stated = message(reply);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected {expected:?}, got {stated:?}"
    );
}

/// RULE: a registration carrying a tag key outside the modelled form is refused, and
/// registers NOTHING — neither the tag nor the application it was carried on; one
/// carrying an admitted key is registered and reports that key back.
#[tokio::test]
async fn a_registration_is_refused_for_a_tag_key_outside_the_modelled_form() {
    for (index, case) in cases().into_iter().enumerate() {
        // One emulator per row: each row is a registration of its own, and an account is
        // allotted only so many registered applications per Region
        // (`service_com_amazonaws_ssmsap_ssmsap::state::APPLICATIONS_QUOTA`), so the rows
        // must not compete for that allotment.
        let emulator = Emulator::new();
        let application_id = format!("tag-key-{index}");
        let reply = register_with_tag(&emulator, &application_id, case.key).await;

        if !case.admitted {
            assert_refused(case.name, &reply, &map_refusal());
            // A refused registration left nothing behind: no application to read, so no
            // tag of it either.
            let fetched = emulator
                .send("/get-application", json!({"ApplicationId": &application_id}))
                .await;
            assert!(
                fetched.status >= 400 && fetched.member("Application").is_none(),
                "case {:?}: a refused registration must leave no application, got {fetched:?}",
                case.name
            );
            continue;
        }

        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
            case.name
        );
        let held = list_tags(&emulator, &application_arn(REGION, &application_id)).await;
        let reported = tag_pairs(held.member("tags"));
        assert_eq!(
            reported.get(case.key).map(String::as_str),
            Some("v"),
            "case {:?}: expected the registered tag to be reported, got {reported:?}",
            case.name
        );
    }
}

/// RULE: the same form holds for `TagResource`, and a refused call changes nothing — the
/// resource keeps exactly the tags it held.
#[tokio::test]
async fn tagging_is_refused_for_a_tag_key_outside_the_modelled_form() {
    let emulator = Emulator::new();
    let application_id = "tag-key-tagging";
    register(
        &emulator,
        "the resource every row tags",
        registration(application_id, &pairs(&[("env", "dev")]), None),
    )
    .await;
    let arn = application_arn(REGION, application_id);

    for case in cases() {
        let reply = tag(&emulator, &arn, &pairs(&[(case.key, "v")])).await;
        let held = tag_pairs(list_tags(&emulator, &arn).await.member("tags"));

        if case.admitted {
            assert_eq!(
                reply.status, 200,
                "case {:?}: expected 200 from TagResource, got {reply:?}",
                case.name
            );
            assert_eq!(
                held.get(case.key).map(String::as_str),
                Some("v"),
                "case {:?}: expected the attached tag to be reported, got {held:?}",
                case.name
            );
            continue;
        }

        assert_refused(case.name, &reply, &map_refusal());
        assert!(
            !held.contains_key(case.key),
            "case {:?}: a refused call must attach nothing, got {held:?}",
            case.name
        );
    }
}

/// RULE: the same form holds for the keys `UntagResource` NAMES, stated at the list the
/// model binds them to — and a refused call removes nothing, including the keys named
/// beside the offending one.
#[tokio::test]
async fn untagging_is_refused_for_a_tag_key_outside_the_modelled_form() {
    let emulator = Emulator::new();
    let application_id = "tag-key-untagging";
    let held_tags = pairs(&[("env", "dev"), ("owner", "sap-team")]);
    register(
        &emulator,
        "the resource every row untags",
        registration(application_id, &held_tags, None),
    )
    .await;
    let arn = application_arn(REGION, application_id);

    for case in cases() {
        if !case.in_query {
            continue;
        }
        // The offending key is named BESIDE a key the resource really holds, so a refused
        // call is one that would otherwise have removed something.
        let named = vec![case.key.to_string(), "env".to_string()];
        let reply = untag(&emulator, &arn, &named).await;
        let held = tag_pairs(list_tags(&emulator, &arn).await.member("tags"));

        if case.admitted {
            assert_eq!(
                reply.status, 200,
                "case {:?}: expected 200 from UntagResource, got {reply:?}",
                case.name
            );
            assert!(
                !held.contains_key("env"),
                "case {:?}: the key named beside an admitted one is removed, got {held:?}",
                case.name
            );
            // Put the key back, so the next row starts from the same tag set.
            assert_eq!(
                tag(&emulator, &arn, &pairs(&[("env", "dev")])).await.status,
                200,
                "case {:?}: the suite restores the tag it removed",
                case.name
            );
            continue;
        }

        assert_refused(case.name, &reply, &list_refusal(&named));
        assert_eq!(
            held.get("env").map(String::as_str),
            Some("dev"),
            "case {:?}: a refused call must remove nothing, got {held:?}",
            case.name
        );
    }
}
