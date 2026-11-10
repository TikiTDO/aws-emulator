// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a refusal message may render.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a refusal states the member it is about and the requirement that
//! member failed, both of which are request-independent, and it renders the offending
//! value only when the model does NOT mark that member's data `@sensitive` — a value
//! taken from a sensitive member is never echoed into the response body. The sensitivity
//! is a property of the MEMBER, not of the request: a refusal about one member of a
//! request renders nothing out of the sensitive members of the same request either.
//!
//! The members: `TagsMap`, which `CreateConfigurationManager$Tags` and
//! `TagResource$Tags` both target, is `@sensitive` and the trait applies to the
//! aggregate, so neither its keys nor its values may be rendered; every other
//! caller-supplied member this service refuses on carries no sensitive trait anywhere in
//! its path, so those refusals keep naming the value they refused
//! (`smithy_query(query="shape", shape="TagsMap,TagKeys,ConfigurationParametersMap")`
//! and `smithy_query(query="operation",
//! operation="CreateConfigurationManager,TagResource,UntagResource,UpdateServiceSettings")`).
//!
//! The tables span what the rule generalises over: which member carries the value, which
//! operation the request reaches, the sensitive key's casing and tail, the sensitive
//! value's length and punctuation, how many entries one request carries, and — on the
//! echoing side — the kind of value a non-sensitive member refuses (a tag key, a
//! configuration type, a configuration version, an account id, a role ARN).

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    QUICK_SETUP_TYPE_CATALOG, RESERVED_TAG_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

use super::reserved_tag_keys::{
    assert_names_no_key, assert_reserved_refusal, call, create, create_ok, emulator, error_code,
    error_message, first_reserved_key, reserved_keys, seeded, tag, tag_object,
    tailed_reserved_keys, untag, KeyEcho,
};

/// Sensitive tag VALUES one request may carry, spanning what the rule generalises over:
/// a plain word, one carrying the punctuation `TagsMap`'s value pattern admits
/// (`smithy_query(query="shape", shape="TagsMap")`), and one at the longest that pattern
/// and its `@length(max: 256)` allow, since a long value is what an unelided echo would
/// amplify.
fn sensitive_values() -> Vec<(String, String)> {
    vec![
        ("a plain value".to_string(), "mine".to_string()),
        (
            "a punctuated value".to_string(),
            "owner=alice:ops+team/eu.1".to_string(),
        ),
        ("a maximum-length value".to_string(), "v".repeat(256)),
    ]
}

/// A reserved tag key this service refuses in a body map: the casings and tails
/// [`reserved_keys`] spans, less the multi-byte row `TagsMap`'s key pattern refuses at
/// the boundary before any handler rule is reached.
fn reserved_body_keys() -> Vec<(String, String)> {
    reserved_keys()
        .into_iter()
        .filter(|(_, key)| key.is_ascii())
        .collect()
}

/// `POST /configurationManager` with one definition the row writes, so a row can refuse
/// on a definition member while the request also carries sensitive tags.
async fn create_definition(
    router: &MultiServiceRouter,
    definition: Value,
    tags: Option<&BTreeMap<String, String>>,
) -> (u16, Value) {
    let mut body = json!({
        "Name": "sensitive-refusal-case",
        "ConfigurationDefinitions": [definition],
    });
    if let Some(tags) = tags {
        body["Tags"] = tag_object(tags);
    }
    call(router, "POST", "/configurationManager", Some(&body)).await
}

/// One definition of the catalog's first published type, deployed somewhere and into a
/// Region, and providing every parameter the table documented for that type marks
/// `(Required)` — all of which every accepted definition must name; the row under test
/// varies one member of this.
fn published_definition() -> Value {
    let type_ = QUICK_SETUP_TYPE_CATALOG[0].type_;
    json!({
        "Type": type_,
        "Parameters": super::type_parameter_fixtures::with_required_parameters(
            type_,
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": "us-east-1",
            }),
        ),
    })
}

/// `PUT /serviceSettings` — `UpdateServiceSettings`, whose `ExplorerEnablingRoleArn` is
/// an `IAMRoleArn`: an `@arnReference` with no modelled pattern, so the form is a
/// handler rule and its refusal is one of the echoing rows.
async fn update_service_settings(router: &MultiServiceRouter, role_arn: &str) -> (u16, Value) {
    let body = json!({ "ExplorerEnablingRoleArn": role_arn });
    call(router, "PUT", "/serviceSettings", Some(&body)).await
}

/// The refusal was the modelled 400 `ValidationException`, stating its sentence in this
/// service's OWN `Message` member, and named `member`.
fn assert_validation_refusal(case: &str, member: &str, status: u16, answer: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    let message = error_message(answer);
    assert!(
        message.contains(member),
        "case {case:?}: the refusal must name the member {member:?}, got {message:?}"
    );
    message
}

/// The refusal was the 400 the layer in FRONT of the service answers a value outside a
/// member's own constraint with — the same status and code, and its sentence in the
/// LOWERCASE `message` member that layer names — and named `path`, the member's validation
/// path.
fn assert_constraint_refusal(case: &str, path: &str, status: u16, answer: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} / {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    let message = answer
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the lowercase message member, got {answer:?}")
        })
        .to_string();
    assert!(
        message.contains(path),
        "case {case:?}: the refusal must name the member path {path:?}, got {message:?}"
    );
    message
}

/// Both write paths that take the sensitive `Tags` map refuse a reserved key with a
/// message that renders neither the key nor the value, and with the SAME message for
/// every key, value and operation — the message carries nothing out of the request.
#[tokio::test]
async fn a_tags_refusal_renders_no_key_or_value_out_of_the_sensitive_map() {
    let router = emulator();
    let arn = create_ok(&router, "sensitive tags", &seeded()).await;
    let mut answered: Option<String> = None;

    for (key_case, key) in reserved_body_keys() {
        for (value_case, value) in sensitive_values() {
            // The two write paths the sensitive map arrives on, and — for each — a map
            // carrying the refused pair alone and beside an ordinary pair, so the number
            // of entries varies too.
            let pairs: Vec<(&str, BTreeMap<String, String>)> = vec![
                (
                    "alone",
                    BTreeMap::from([(key.clone(), value.clone())]),
                ),
                (
                    "beside an ordinary pair",
                    BTreeMap::from([
                        (key.clone(), value.clone()),
                        ("Tier".to_string(), "gold".to_string()),
                    ]),
                ),
            ];
            for (shape, supplied) in pairs {
                for operation in ["CreateConfigurationManager", "TagResource"] {
                    let case =
                        format!("{operation} with {key_case} and {value_case}, {shape}");
                    let (status, answer) = match operation {
                        "CreateConfigurationManager" => {
                            create(&router, "sensitive-refusal", &supplied).await
                        }
                        _ => tag(&router, &arn, &supplied).await,
                    };
                    assert_reserved_refusal(
                        &case,
                        "Tags",
                        KeyEcho::Withheld,
                        &key,
                        status,
                        &answer,
                    );
                    let message = error_message(&answer);
                    assert!(
                        !message.contains(&value),
                        "case {case:?}: the refusal must render no value out of the \
                         sensitive map, but it carried {value:?}: {message:?}"
                    );
                    // One request-independent message: naming the member and the
                    // requirement leaves nothing for the request to vary.
                    match &answered {
                        None => answered = Some(message),
                        Some(first) => assert_eq!(
                            &message, first,
                            "case {case:?}: every Tags refusal must carry the same \
                             request-independent message, expected {first:?}, got \
                             {message:?}"
                        ),
                    }
                }
            }
        }
    }

    let message = answered.expect("the sensitive-tags table is not empty");
    assert!(
        message.contains(RESERVED_TAG_PREFIX),
        "case \"the shared Tags refusal\": the message must still state the reserved \
         prefix {RESERVED_TAG_PREFIX:?}, got {message:?}"
    );
}

/// A refusal about ANOTHER member of the same request still echoes that member's value,
/// and still renders nothing out of the sensitive map the same request carries: the gate
/// is the member the value came from, not the request it arrived in.
#[tokio::test]
async fn a_refusal_about_another_member_renders_no_tag_data() {
    let router = emulator();
    let keys = tailed_reserved_keys(true);
    let supplied: BTreeMap<String, String> = keys
        .iter()
        .map(|key| (key.clone(), "owner=alice:ops".to_string()))
        .collect();

    // An account id that is not twelve digits, refused on the definition's parameters —
    // a non-sensitive member, so its value is named.
    let mut definition = published_definition();
    definition["Parameters"] = json!({
        "TargetAccounts": "12345",
        "TargetRegions": "us-east-1",
    });
    let case = "a malformed account id beside sensitive tags";
    let (status, answer) = create_definition(&router, definition, Some(&supplied)).await;
    let message = assert_validation_refusal(case, "ConfigurationDefinitions.Parameters", status, &answer);
    assert!(
        message.contains("'12345'"),
        "case {case:?}: the refusal must name the refused account id, got {message:?}"
    );
    assert_names_no_key(case, &message, &keys);
    assert!(
        !message.contains("owner=alice:ops"),
        "case {case:?}: the refusal must render no value out of the sensitive map, got \
         {message:?}"
    );
}

/// Every other caller-supplied member this service refuses on is NON-sensitive, so its
/// refusal names the value it refused: the rows span the kinds of value those members
/// carry, each on a member the sensitivity catalog does not cover.
#[tokio::test]
async fn a_non_sensitive_member_refusal_names_the_refused_value() {
    let router = emulator();
    let arn = create_ok(&router, "non-sensitive members", &seeded()).await;

    // A tag key out of `UntagResource$TagKeys`, a bare `list<String>`: the one tag-key
    // member with no sensitive trait in its path, so the key is named.
    let keys = tailed_reserved_keys(false);
    let expected = first_reserved_key(&keys);
    let named: Vec<&str> = keys.iter().map(String::as_str).collect();
    let (status, answer) = untag(&router, &arn, &named).await;
    assert_reserved_refusal(
        "UntagResource with several reserved keys",
        "TagKeys",
        KeyEcho::Named,
        &expected,
        status,
        &answer,
    );

    // A configuration type the catalog does not publish.
    let mut definition = published_definition();
    definition["Type"] = json!("AWSQuickSetupType-NoSuchTemplate");
    let case = "an unpublished configuration type";
    let (status, answer) = create_definition(&router, definition, None).await;
    let message = assert_validation_refusal(case, "ConfigurationDefinitions.Type", status, &answer);
    assert!(
        message.contains("'AWSQuickSetupType-NoSuchTemplate'"),
        "case {case:?}: the refusal must name the refused type, got {message:?}"
    );

    // A configuration version the published type does not offer.
    let mut definition = published_definition();
    definition["TypeVersion"] = json!("99.99");
    let case = "an unpublished configuration version";
    let (status, answer) = create_definition(&router, definition, None).await;
    let message =
        assert_validation_refusal(case, "ConfigurationDefinitions.TypeVersion", status, &answer);
    assert!(
        message.contains("'99.99'"),
        "case {case:?}: the refusal must name the refused version, got {message:?}"
    );

    // An `IAMRoleArn` that is not the ARN of a role, on each of the two members that
    // take one. Such a refusal restates a VALUE CONSTRAINT, so it names the member by
    // the validation path the boundary layer names one by.
    let malformed_role_arn = "arn:aws:iam::123456789012:user/not-a-role";
    let mut definition = published_definition();
    definition["LocalDeploymentAdministrationRoleArn"] = json!(malformed_role_arn);
    let case = "a malformed administration role ARN";
    let (status, answer) = create_definition(&router, definition, None).await;
    let message = assert_constraint_refusal(
        case,
        "configurationDefinitions.1.member.localDeploymentAdministrationRoleArn",
        status,
        &answer,
    );
    assert!(
        message.contains(&format!("'{malformed_role_arn}'")),
        "case {case:?}: the refusal must name the refused ARN, got {message:?}"
    );

    let case = "a malformed explorer-enabling role ARN";
    let (status, answer) = update_service_settings(&router, malformed_role_arn).await;
    let message = assert_constraint_refusal(case, "explorerEnablingRoleArn", status, &answer);
    assert!(
        message.contains(&format!("'{malformed_role_arn}'")),
        "case {case:?}: the refusal must name the refused ARN, got {message:?}"
    );
}
