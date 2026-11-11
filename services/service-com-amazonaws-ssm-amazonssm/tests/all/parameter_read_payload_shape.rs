// SPDX-License-Identifier: CC0-1.0

//! The SHAPE of a Parameter Store read payload, end to end: `PutParameter` (and
//! `LabelParameterVersion`) write, and every read reports the record's name, its label
//! and policy lists and the selector it resolved by.
//!
//! The rules under test, each one level more general than any single parameter:
//!
//! * A parameter name is canonicalised once, when the parameter is created: a top-level
//!   name carries no leading separator, while a hierarchical name is its whole path
//!   ("For parameters in a hierarchy, you must include a leading forward slash character
//!   (/)", `smithy_query(query="doc",shape="PutParameterRequest",member="Name")`), so
//!   every read reports the name the parameter's ARN carries and a reported `Name` fed
//!   back into another call addresses the parameter it was read from.
//! * The label and policy lists of a version are lists the version always has
//!   (`ParameterHistory$Labels`, "Labels assigned to the parameter version";
//!   `ParameterMetadata$Policies`, "A list of policies associated with a parameter"), so
//!   a version nothing labelled and no policy document was stored with reports them
//!   EMPTY rather than not at all — and a read never has to tell "none attached" from
//!   "not reported".
//! * `Parameter$Selector` echoes the selector the read resolved by in the documented
//!   `parameter_name:version` / `parameter_name:label` forms
//!   (`smithy_query(query="doc",shape="Parameter",member="Selector")`), separator
//!   included, so appending it to the reported `Name` yields a `Name` that resolves to
//!   the very version served.
//!
//! The suites are TABLES. Their rows differ along what the rules generalise over — the
//! form and depth of the name, how many labels a version holds, how many policies a put
//! carried, whether the selector names a version or a label, and which read reports it —
//! rather than the single parameter a recorded unit happened to read.
//!
//! Every call goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol") — and nothing reaches into a
//! state struct, so a payload that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, router, ACCOUNT, REGION};

/// The policy types Parameter Store supports ("Parameter Store supports the following
/// policy types: Expiration, ExpirationNotification, and NoChangeNotification",
/// `smithy_query(query="doc",shape="InvalidPolicyTypeException")`). The rows below carry
/// a document of one, of two, and of none.
const POLICY_TYPES: &[&str] = &[
    "Expiration",
    "ExpirationNotification",
    "NoChangeNotification",
];

/// The status a stored policy is reported in ("Pending (the policy hasn't been enforced
/// or applied yet)",
/// `smithy_query(query="doc",shape="ParameterInlinePolicy",member="PolicyStatus")`).
const POLICY_STATUS_PENDING: &str = "Pending";

/// `PutParameter`, asserting it succeeded and answering the version it created.
async fn put_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> i64 {
    let (status, response) = call(router, "PutParameter", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutParameter {body} must succeed, got {status} {response}"
    );
    response["Version"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: PutParameter reports a version, got {response}"))
}

/// One operation, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, response) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} {body} must succeed, got {status} {response}"
    );
    response
}

/// The ARN the service mints for the parameter `name` names in [`REGION`]: the canonical
/// name follows the `parameter/` prefix, whichever spelling the put used.
fn parameter_arn(name: &str) -> String {
    format!(
        "arn:aws:ssm:{REGION}:{ACCOUNT}:parameter/{}",
        name.strip_prefix('/').unwrap_or(name)
    )
}

/// The entries of a list-shaped read, under the member that carries them.
fn list(case: &str, response: &Value, member: &str) -> Vec<Value> {
    response[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the answer carries a {member} list: {response}"))
        .clone()
}

/// The `Labels` of one history entry, sorted, or `None` when the member is absent — the
/// distinction the rule is about.
fn labels(entry: &Value) -> Option<Vec<String>> {
    let mut held: Vec<String> = entry["Labels"]
        .as_array()?
        .iter()
        .map(|label| label.as_str().unwrap_or_default().to_string())
        .collect();
    held.sort();
    Some(held)
}

/// The `PolicyType` of every policy one entry reports, or `None` when the member is
/// absent.
fn policy_types(entry: &Value) -> Option<Vec<String>> {
    Some(
        entry["Policies"]
            .as_array()?
            .iter()
            .map(|policy| policy["PolicyType"].as_str().unwrap_or_default().to_string())
            .collect(),
    )
}

/// A `Policies` document carrying one policy of each of the first `count` policy types.
fn policy_document(count: usize) -> Value {
    json!(POLICY_TYPES
        .iter()
        .take(count)
        .map(|policy_type| json!({
            "Type": policy_type,
            "Version": "1.0",
            "Attributes": { "Timestamp": "2035-01-01T00:00:00.000Z" }
        }))
        .collect::<Vec<Value>>())
}

/// A name is canonicalised once, at create: every read reports the canonical rendering —
/// the one the parameter's ARN carries — and that reported name addresses the same
/// parameter when it is fed back into another call.
#[tokio::test]
async fn every_read_reports_the_canonical_name_of_the_parameter() {
    let router = router();

    // (case, the name the put used, the name every read must report)
    let cases: &[(&str, &str, &str)] = &[
        ("a top-level name", "PayloadTop", "PayloadTop"),
        (
            "the same top-level name spelled with the separator",
            "/PayloadRooted",
            "PayloadRooted",
        ),
        ("a two-level name", "/Payload/two", "/Payload/two"),
        (
            "a five-level name",
            "/Payload/a/b/c/five",
            "/Payload/a/b/c/five",
        ),
        (
            "a top-level name of only symbols the name rule admits",
            "/Payload_top-level.0",
            "Payload_top-level.0",
        ),
    ];

    for (case, written, expected) in cases {
        put_ok(
            &router,
            case,
            &json!({ "Name": written, "Type": "String", "Value": "v1" }),
        )
        .await;

        // Every read of one parameter, each reporting the name under its own shape:
        // `Parameter`, `ParameterHistory` and `ParameterMetadata`.
        let single = ok(&router, case, "GetParameter", &json!({ "Name": written })).await;
        let batch = ok(&router, case, "GetParameters", &json!({ "Names": [written] })).await;
        let history = ok(
            &router,
            case,
            "GetParameterHistory",
            &json!({ "Name": written }),
        )
        .await;
        let described = ok(
            &router,
            case,
            "DescribeParameters",
            &json!({ "ParameterFilters": [{ "Key": "Name", "Values": [written] }] }),
        )
        .await;

        let reported: Vec<(&str, Value)> = vec![
            ("GetParameter", single["Parameter"].clone()),
            ("GetParameters", list(case, &batch, "Parameters")[0].clone()),
            (
                "GetParameterHistory",
                list(case, &history, "Parameters")[0].clone(),
            ),
            (
                "DescribeParameters",
                list(case, &described, "Parameters")[0].clone(),
            ),
        ];
        for (operation, record) in &reported {
            assert_eq!(
                record["Name"].as_str(),
                Some(*expected),
                "case {case:?}: {operation} must report the name as {expected:?}, got {record}"
            );
        }
        // The ARN carries the same rendering after its `parameter/` prefix, so the two
        // spellings of the one parameter cannot drift apart.
        assert_eq!(
            single["Parameter"]["ARN"].as_str(),
            Some(parameter_arn(written).as_str()),
            "case {case:?}: the ARN names the canonical parameter, got {single}"
        );

        // The round trip: the reported name addresses the parameter it was read from,
        // and a write through it appends a version to that same record rather than
        // creating a second parameter.
        let version = put_ok(
            &router,
            case,
            &json!({ "Name": expected, "Value": "v2", "Overwrite": true }),
        )
        .await;
        assert_eq!(
            version, 2,
            "case {case:?}: a write through the reported name appends to the same parameter, \
             got version {version}"
        );
        let reread = ok(&router, case, "GetParameter", &json!({ "Name": expected })).await;
        assert_eq!(
            (
                reread["Parameter"]["Name"].as_str(),
                reread["Parameter"]["Value"].as_str(),
            ),
            (Some(*expected), Some("v2")),
            "case {case:?}: reading the reported name serves the same parameter, got {reread}"
        );
    }
}

/// Every history entry carries its own label list, its own policy list and the writer of
/// the version it reports — empty lists for a version nothing labelled and no policy
/// document was stored with.
#[tokio::test]
async fn every_history_entry_carries_its_labels_policies_and_writer() {
    let router = router();

    // (case, the name, how many labels to attach to the newest version, how many
    // policies the newest put carries)
    let cases: &[(&str, &str, usize, usize)] = &[
        ("no labels and no policies", "/Shape/bare", 0, 0),
        ("one label, no policies", "/Shape/one-label", 1, 0),
        ("three labels, no policies", "/Shape/three-labels", 3, 0),
        ("no labels, one policy", "/Shape/one-policy", 0, 1),
        ("two labels, two policies", "/Shape/both", 2, 2),
        ("no labels, three policies", "/Shape/three-policies", 0, 3),
    ];

    for (case, name, label_count, policy_count) in cases {
        // Two versions, so an entry's lists are seen to belong to the version that holds
        // them: only the newest carries the labels and the policy document.
        put_ok(
            &router,
            case,
            &json!({ "Name": name, "Type": "String", "Value": "first" }),
        )
        .await;
        let mut put = json!({ "Name": name, "Value": "second", "Overwrite": true });
        if *policy_count > 0 {
            // "Standard parameters ... can't be configured to use parameter policies"
            // (`smithy_query(query="doc",shape="PutParameterRequest",member="Tier")`).
            put["Tier"] = json!("Advanced");
            put["Policies"] = json!(policy_document(*policy_count).to_string());
        }
        let newest = put_ok(&router, case, &put).await;

        let attached: Vec<String> = (0..*label_count)
            .map(|index| format!("Label{index}"))
            .collect();
        if !attached.is_empty() {
            ok(
                &router,
                case,
                "LabelParameterVersion",
                &json!({ "Name": name, "ParameterVersion": newest, "Labels": attached }),
            )
            .await;
        }
        let mut expected_labels = attached.clone();
        expected_labels.sort();
        let expected_policies: Vec<String> = POLICY_TYPES
            .iter()
            .take(*policy_count)
            .map(|policy_type| (*policy_type).to_string())
            .collect();

        let history = ok(&router, case, "GetParameterHistory", &json!({ "Name": name })).await;
        let entries = list(case, &history, "Parameters");
        assert_eq!(
            entries.len(),
            2,
            "case {case:?}: the history reports one entry per put, got {history}"
        );
        for entry in &entries {
            let version = entry["Version"].as_i64();
            let is_newest = version == Some(newest);
            assert_eq!(
                labels(entry),
                Some(if is_newest {
                    expected_labels.clone()
                } else {
                    Vec::new()
                }),
                "case {case:?}: version {version:?} must report its own Labels list, got {entry}"
            );
            assert_eq!(
                policy_types(entry),
                Some(if is_newest {
                    expected_policies.clone()
                } else {
                    Vec::new()
                }),
                "case {case:?}: version {version:?} must report its own Policies list, got \
                 {entry}"
            );
            for policy in entry["Policies"].as_array().unwrap_or(&Vec::new()) {
                assert_eq!(
                    policy["PolicyStatus"].as_str(),
                    Some(POLICY_STATUS_PENDING),
                    "case {case:?}: a stored policy nothing applied is pending, got {policy}"
                );
                let text = policy["PolicyText"].as_str().unwrap_or_default();
                let parsed: Value = serde_json::from_str(text).unwrap_or_else(|_| {
                    panic!("case {case:?}: PolicyText is the JSON text of the policy: {policy}")
                });
                assert_eq!(
                    parsed["Type"].as_str(),
                    policy["PolicyType"].as_str(),
                    "case {case:?}: PolicyText declares the type reported beside it, got {policy}"
                );
            }
            // "ARN of the Amazon Web Services user who last changed the parameter": the
            // writer the put was signed as, reported for every entry.
            let writer = entry["LastModifiedUser"].as_str().unwrap_or_default();
            assert!(
                writer.starts_with("arn:"),
                "case {case:?}: version {version:?} must report the ARN of its writer, got \
                 {entry}"
            );
        }

        // The metadata read of the same parameter carries the newest version's policy
        // list and writer under `ParameterMetadata`.
        let described = ok(
            &router,
            case,
            "DescribeParameters",
            &json!({ "ParameterFilters": [{ "Key": "Name", "Values": [name] }] }),
        )
        .await;
        let metadata = list(case, &described, "Parameters");
        assert_eq!(
            metadata.len(),
            1,
            "case {case:?}: the describe reports the one parameter, got {described}"
        );
        assert_eq!(
            policy_types(&metadata[0]),
            Some(expected_policies.clone()),
            "case {case:?}: the describe reports the newest version's Policies list, got \
             {described}"
        );
        assert!(
            metadata[0]["LastModifiedUser"]
                .as_str()
                .unwrap_or_default()
                .starts_with("arn:"),
            "case {case:?}: the describe reports the ARN of the writer, got {described}"
        );
        // `ParameterMetadata` models no `Labels` member, so a metadata listing reports
        // none however many labels the version holds.
        assert!(
            metadata[0].get("Labels").is_none(),
            "case {case:?}: a metadata listing carries no Labels member, got {described}"
        );
    }
}

/// A read echoes the selector it resolved by in the documented form, separator and all,
/// so the reported `Name` and `Selector` concatenate back into a `Name` that resolves to
/// the very version served.
#[tokio::test]
async fn a_read_echoes_the_selector_it_resolved_by() {
    let router = router();
    let name = "/Shape/selected";
    let values = ["first", "second", "third"];
    for (index, value) in values.iter().enumerate() {
        let mut body = json!({ "Name": name, "Type": "String", "Value": value });
        if index > 0 {
            body["Overwrite"] = json!(true);
        }
        put_ok(&router, value, &body).await;
    }
    // A label on the oldest version, so a selector that is not a version number is
    // covered too.
    ok(
        &router,
        "label the oldest version",
        "LabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 1, "Labels": ["Release.1-0"] }),
    )
    .await;

    // (case, the selector the request carried, the value it resolves to, the selector
    // the answer must echo)
    let cases: &[(&str, Option<&str>, &str, Option<&str>)] = &[
        ("no selector at all", None, "third", None),
        ("the oldest version", Some("1"), "first", Some(":1")),
        ("a middle version", Some("2"), "second", Some(":2")),
        ("the newest version", Some("3"), "third", Some(":3")),
        (
            "a label",
            Some("Release.1-0"),
            "first",
            Some(":Release.1-0"),
        ),
    ];

    for (case, selector, value, echoed) in cases {
        let asked = match selector {
            Some(selector) => format!("{name}:{selector}"),
            None => name.to_string(),
        };
        let single = ok(&router, case, "GetParameter", &json!({ "Name": &asked })).await;
        let batch = ok(&router, case, "GetParameters", &json!({ "Names": [&asked] })).await;
        for (operation, parameter) in [
            ("GetParameter", single["Parameter"].clone()),
            ("GetParameters", list(case, &batch, "Parameters")[0].clone()),
        ] {
            assert_eq!(
                (
                    parameter["Value"].as_str(),
                    parameter["Selector"].as_str(),
                    parameter["Name"].as_str(),
                ),
                (Some(*value), *echoed, Some(name)),
                "case {case:?}: {operation} must serve {value:?} and echo the selector \
                 {echoed:?}, got {parameter}"
            );

            // The invariant: Name + Selector is a reference that resolves to the same
            // version. A read that echoed a selector without its separator would ask
            // for a parameter nobody created here.
            let rebuilt = format!(
                "{}{}",
                parameter["Name"].as_str().unwrap_or_default(),
                parameter["Selector"].as_str().unwrap_or_default()
            );
            let again = ok(&router, case, "GetParameter", &json!({ "Name": &rebuilt })).await;
            assert_eq!(
                (
                    again["Parameter"]["Value"].as_str(),
                    again["Parameter"]["Version"].as_i64(),
                ),
                (
                    parameter["Value"].as_str(),
                    parameter["Version"].as_i64(),
                ),
                "case {case:?}: the reference {rebuilt:?} rebuilt from {operation}'s answer must \
                 serve the same version, got {again}"
            );
        }
    }
}
