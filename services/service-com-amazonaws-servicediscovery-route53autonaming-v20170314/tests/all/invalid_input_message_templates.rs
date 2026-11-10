// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TEMPLATES this service's `InvalidInput` (and
//! `TooManyTagsException`) bodies are worded from, over tables that span the operations
//! and the member kinds each template covers.
//!
//! Five rules are under test, each asserted over a SET of inputs rather than one:
//!
//! 1. A request refused for a constraint the MODEL declares answers the framework's
//!    counted envelope — `<n> validation error detected: Value '<submitted>' at
//!    '<member path>' failed to satisfy constraint: <requirement>` — and carries that
//!    text in `Message`, the member every error shape of this model declares for it. A
//!    clause about a MAP member quotes the entries the request submitted, in the order
//!    this service states them, rather than the framework's count of them.
//! 2. A namespace update applied to a namespace of another `NamespaceType` reports the
//!    comparison the caller has to make: the type the stored namespace has, and the type
//!    the called operation requires.
//! 3. A filtered listing refuses a filter whose `Condition` its `Name` does not support
//!    with one clause per offending filter, naming the filter, the conditions that filter
//!    supports, and the condition the request asked for.
//! 4. The tag-quota refusal is one FIXED sentence, naming neither the quota, the resource
//!    nor the keys the request carried, wherever the quota is applied.
//! 5. A service-settings update of a service in an API-only (HTTP) namespace is refused
//!    with one FIXED sentence, naming neither the service nor the operation.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, the same path the emulator
//! serves in production, reusing the harness in `cloud_map_core.rs`.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cloud_map_core::{call, dns_namespace, emulator, namespace, ok, service, REGION};

/// The member every error shape of this model declares its text in, which is where a
/// client reads it whichever layer produced the failure.
const MESSAGE_MEMBER: &str = "Message";

/// The `@pattern` the model declares on `NamespaceNamePublic`
/// (`CreatePublicDnsNamespace$Name`): `smithy_query(query="shape",
/// shape="NamespaceNamePublic")`. A clause quotes the expression as the model states it.
const PUBLIC_DNS_NAME_PATTERN: &str =
    r"^([a-zA-Z0-9]([a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z0-9]([a-zA-Z0-9\-]{0,61}[a-zA-Z0-9])?$";

/// The `@length` maximum the model declares on `ServiceAttributesMap`
/// (`smithy_query(query="shape", shape="ServiceAttributesMap")`), which a clause about
/// that member states.
const MAX_ATTRIBUTES: usize = 30;

/// The error code and `Message` of one refused call, with `Message` read ONLY from the
/// member this model declares — a body that reports its text anywhere else answers with
/// none as far as a client of this service is concerned.
async fn refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
) -> (u16, String, Option<String>) {
    let (status, json) = call(router, operation, body).await;
    let code = json["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = json
        .get(MESSAGE_MEMBER)
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, message)
}

/// Assert one row: HTTP 400, the expected error code, and exactly the expected `Message`.
async fn assert_message(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    expected_code: &str,
    expected_message: &str,
) {
    let (status, code, message) = refusal(router, operation, body).await;
    assert_eq!(
        (status, code.as_str()),
        (400, expected_code),
        "case {case:?} on {operation}: expected 400 {expected_code}, got {status} {code} \
         {message:?}"
    );
    assert_eq!(
        message.as_deref(),
        Some(expected_message),
        "case {case:?} on {operation}: expected the Message {expected_message:?}, got {message:?}"
    );
}

/// The framework's clause for one member: the value the request submitted, the member
/// path the walk states it at, and the requirement it failed.
fn one_violation(submitted: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{submitted}' at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// RULE 1, over the `@pattern` class: a public DNS namespace name the model's pattern
/// does not admit is refused with the counted envelope naming the member, the name the
/// request carried and the expression it had to satisfy — whatever makes the name
/// inadmissible. The last rows prove the same pattern ADMITS the names it should, so the
/// refusal is the model's rule and not a reformulation of it.
#[tokio::test]
async fn a_pattern_refusal_quotes_the_submitted_name_the_member_and_the_expression() {
    let router = emulator();

    // Names outside the pattern, one per way of leaving it: a space, a character the
    // label grammar excludes, a label edge the grammar reserves for alphanumerics, no
    // dot at all, and the empty name (the `@length` minimum is 0, so it reaches the
    // pattern).
    let rejected = [
        "bad public ns",
        "ns_underscore.example.com",
        "-leading.example.com",
        "trailing-.example.com",
        "no-dot",
        "",
    ];
    for name in rejected {
        assert_message(
            &router,
            &format!("the name {name:?}"),
            "CreatePublicDnsNamespace",
            json!({ "Name": name }),
            "InvalidInput",
            &one_violation(
                name,
                "name",
                &format!("Member must satisfy regular expression pattern: {PUBLIC_DNS_NAME_PATTERN}"),
            ),
        )
        .await;
    }

    // Names the pattern admits: a two-label name, a hyphen inside a label, digits, and
    // mixed case.
    for name in [
        "ok.example.com",
        "x-1.example.com",
        "9.0",
        "MiXeD.Example.COM",
    ] {
        let (status, json) = call(&router, "CreatePublicDnsNamespace", json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the pattern admits this name, got {status}: {json}"
        );
    }
}

/// RULE 1, over the `@range` and `@required` classes reached through other operations:
/// the same envelope, the same member in the body, each clause stating that class's own
/// requirement. The bounds come from the model (`MaxResults` is `range 1..100`), and both
/// published bounds plus one value past each are covered.
#[tokio::test]
async fn a_bound_refusal_states_its_own_requirement_in_the_same_envelope() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "bounds-ns").await;
    let service_id = service(&router, &namespace_id, "bounds-svc").await;

    // (case, MaxResults, the requirement the clause states)
    let rows: &[(&str, i64, Option<&str>)] = &[
        (
            "one under the published minimum",
            0,
            Some("Member must have value greater than or equal to 1"),
        ),
        ("the published minimum", 1, None),
        ("the published maximum", 100, None),
        (
            "one over the published maximum",
            101,
            Some("Member must have value less than or equal to 100"),
        ),
    ];

    for (case, max_results, requirement) in rows {
        let body = json!({ "ServiceId": service_id, "MaxResults": max_results });
        match requirement {
            Some(requirement) => {
                assert_message(
                    &router,
                    case,
                    "ListInstances",
                    body,
                    "InvalidInput",
                    &one_violation(&max_results.to_string(), "maxResults", requirement),
                )
                .await;
            }
            None => {
                let (status, json) = call(&router, "ListInstances", body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: a value inside the bound is served, got {status}: {json}"
                );
            }
        }
    }
}

/// The attribute keys of the over-long map below, in the order THIS SERVICE states them
/// in a clause: by hash bucket over a 64-bucket table, ascending.
///
/// Hand-computed from the rule in `src/service/submitted_map.rs` rather than from the
/// code: the 33 entries are held in a 64-bucket table, and a key's bucket is the low six
/// bits of its 31-based string hash mixed with its own high half. `"Aa"` and `"BB"` are
/// the classic pair whose hashes are equal (2112), which is a multiple of 64, so both sit
/// in bucket 0 — the order between them is the order the request wrote them, and the
/// request below is serialized with its members sorted. Every single-character key hashes
/// to its own code unit, so `'A'..'E'` take buckets 1..5 and `'a'..'z'` buckets 33..58.
///
/// The order is therefore NEITHER the order the request carried the entries in NOR the
/// order of the keys, which is what makes this row a test of the rule.
const ATTRIBUTES_IN_ITERATION_ORDER: [&str; 33] = [
    "Aa", "BB", "A", "B", "C", "D", "E", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k",
    "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
];

/// The value stored under `key` in that map: distinct per key, so the clause's rendering
/// has to pair each key with its own value.
fn attribute_value(key: &str) -> String {
    format!("v-{key}")
}

/// RULE 1, over the `@length` class on a MAP member: the clause quotes the entries the
/// request submitted — `{<key>=<value>, …}` in the order this service states them — where
/// the framework's own walk has only a count of them, and the requirement names the
/// bound the model declares.
///
/// The rows are the two ways a map can leave `ServiceAttributesMap`'s declared length
/// (`length 1..30`): one entry short of its minimum, and three past its maximum.
#[tokio::test]
async fn a_map_length_refusal_quotes_the_entries_the_request_submitted() {
    let router = emulator();
    let (_, namespace_id) = namespace(&router, "attributes-ns").await;
    let service_id = service(&router, &namespace_id, "attributes-svc").await;

    let mut attributes = Map::new();
    for key in ATTRIBUTES_IN_ITERATION_ORDER {
        attributes.insert(key.to_string(), json!(attribute_value(key)));
    }
    let stated: Vec<String> = ATTRIBUTES_IN_ITERATION_ORDER
        .iter()
        .map(|key| format!("{key}={}", attribute_value(key)))
        .collect();

    // (case, the attributes map, the value the clause quotes, the requirement it states)
    let rows: Vec<(&str, Value, String, String)> = vec![
        (
            "no entries at all, one short of the declared minimum",
            json!({}),
            "{}".to_string(),
            "Member must have length greater than or equal to 1".to_string(),
        ),
        (
            "three entries past the declared maximum",
            Value::Object(attributes),
            format!("{{{}}}", stated.join(", ")),
            format!("Member must have length less than or equal to {MAX_ATTRIBUTES}"),
        ),
    ];

    for (case, submitted, quoted, requirement) in rows {
        assert_message(
            &router,
            case,
            "UpdateServiceAttributes",
            json!({ "ServiceId": service_id, "Attributes": submitted }),
            "InvalidInput",
            &one_violation(&quoted, "attributes", &requirement),
        )
        .await;
    }
}

/// RULE 2: a namespace update applied to a namespace of another type reports the type the
/// stored namespace has and the type the called operation requires — over every
/// (operation, stored type) pair the three namespace updates and the three namespace
/// types form, which is each update against each of the two types it does NOT apply to.
#[tokio::test]
async fn a_namespace_update_of_the_wrong_type_reports_both_types() {
    let router = emulator();
    let (_, http_id) = namespace(&router, "wrong-type-http").await;
    let public_id = dns_namespace(&router, "wrong-type-public").await;
    let private_id = private_dns_namespace(&router, "wrong-type-private.internal").await;

    // (the stored namespace, its type)
    let stored = [
        (http_id, "HTTP"),
        (public_id, "DNS_PUBLIC"),
        (private_id, "DNS_PRIVATE"),
    ];
    // (the operation, the type it requires, the change body it takes)
    let updates = [
        (
            "UpdateHttpNamespace",
            "HTTP",
            json!({ "Description": "changed" }),
        ),
        (
            "UpdatePublicDnsNamespace",
            "DNS_PUBLIC",
            json!({ "Description": "changed" }),
        ),
        (
            "UpdatePrivateDnsNamespace",
            "DNS_PRIVATE",
            json!({ "Description": "changed" }),
        ),
    ];

    for (namespace_id, stored_type) in &stored {
        for (operation, required_type, change) in &updates {
            let body = json!({ "Id": namespace_id, "Namespace": change });
            if stored_type == required_type {
                let (status, json) = call(&router, operation, body).await;
                assert_eq!(
                    status, 200,
                    "case {stored_type} through {operation}: the type matches, got {status}: \
                     {json}"
                );
                continue;
            }
            assert_message(
                &router,
                &format!("a {stored_type} namespace through {operation}"),
                operation,
                body,
                "InvalidInput",
                &format!(
                    "Namespace with incorrect type {stored_type} provided. Please provide \
                     namespace with type {required_type}."
                ),
            )
            .await;
        }
    }
}

/// RULE 3: a filter whose `Condition` its `Name` does not support is refused with one
/// clause per offending filter, naming the filter, what that filter supports and what the
/// request asked for.
///
/// The table spans the three filtered listings, every condition the model's
/// `FilterCondition` enum declares, the filters whose supported set is one condition and
/// those whose set is two, a request carrying TWO offending filters (which is what the
/// per-filter clause exists for), and — in the last rows — conditions each filter DOES
/// support, which are served.
#[tokio::test]
async fn an_unsupported_filter_condition_names_the_filter_and_both_condition_sets() {
    let router = emulator();

    // (case, operation, the filters, the clauses the refusal accumulates)
    let rows: Vec<(&str, &str, Value, Vec<&str>)> = vec![
        (
            "a namespace TYPE filter asked for a range",
            "ListNamespaces",
            json!([{ "Name": "TYPE", "Values": ["HTTP", "DNS_PUBLIC"], "Condition": "BETWEEN" }]),
            vec!["filter TYPE supports condition(s) [BEGINS_WITH, EQ], but got BETWEEN; "],
        ),
        (
            "a namespace NAME filter asked for a set",
            "ListNamespaces",
            json!([{ "Name": "NAME", "Values": ["a"], "Condition": "IN" }]),
            vec!["filter NAME supports condition(s) [BEGINS_WITH, EQ], but got IN; "],
        ),
        (
            "a namespace HTTP_NAME filter asked for a range",
            "ListNamespaces",
            json!([{ "Name": "HTTP_NAME", "Values": ["a"], "Condition": "BETWEEN" }]),
            vec!["filter HTTP_NAME supports condition(s) [BEGINS_WITH, EQ], but got BETWEEN; "],
        ),
        (
            "a namespace RESOURCE_OWNER filter asked for a prefix match",
            "ListNamespaces",
            json!([{ "Name": "RESOURCE_OWNER", "Values": ["SELF"], "Condition": "BEGINS_WITH" }]),
            vec!["filter RESOURCE_OWNER supports condition(s) [EQ], but got BEGINS_WITH; "],
        ),
        (
            "two offending namespace filters in one request",
            "ListNamespaces",
            json!([
                { "Name": "TYPE", "Values": ["HTTP"], "Condition": "IN" },
                { "Name": "RESOURCE_OWNER", "Values": ["SELF"], "Condition": "BEGINS_WITH" },
            ]),
            vec![
                "filter TYPE supports condition(s) [BEGINS_WITH, EQ], but got IN; ",
                "filter RESOURCE_OWNER supports condition(s) [EQ], but got BEGINS_WITH; ",
            ],
        ),
        (
            "a service NAMESPACE_ID filter asked for a prefix match",
            "ListServices",
            json!([{ "Name": "NAMESPACE_ID", "Values": ["ns-1"], "Condition": "BEGINS_WITH" }]),
            vec!["filter NAMESPACE_ID supports condition(s) [EQ], but got BEGINS_WITH; "],
        ),
        (
            "an operation STATUS filter asked for a range",
            "ListOperations",
            json!([{ "Name": "STATUS", "Values": ["SUCCESS"], "Condition": "BETWEEN" }]),
            vec!["filter STATUS supports condition(s) [EQ, IN], but got BETWEEN; "],
        ),
        (
            "an operation UPDATE_DATE filter asked for equality",
            "ListOperations",
            json!([{ "Name": "UPDATE_DATE", "Values": ["0"], "Condition": "EQ" }]),
            vec!["filter UPDATE_DATE supports condition(s) [BETWEEN], but got EQ; "],
        ),
    ];

    for (case, operation, filters, clauses) in rows {
        assert_message(
            &router,
            case,
            operation,
            json!({ "Filters": filters }),
            "InvalidInput",
            &format!("Invalid filter condition(s) provided: {}", clauses.concat()),
        )
        .await;
    }

    // A condition each filter DOES support is served, so the rule refuses only what the
    // supported set leaves out.
    let served: Vec<(&str, Value)> = vec![
        (
            "ListNamespaces",
            json!([{ "Name": "TYPE", "Values": ["HTTP"], "Condition": "EQ" }]),
        ),
        (
            "ListNamespaces",
            json!([{ "Name": "NAME", "Values": ["a"], "Condition": "BEGINS_WITH" }]),
        ),
        (
            "ListNamespaces",
            json!([{ "Name": "RESOURCE_OWNER", "Values": ["SELF"], "Condition": "EQ" }]),
        ),
        (
            "ListServices",
            json!([{ "Name": "RESOURCE_OWNER", "Values": ["SELF"], "Condition": "EQ" }]),
        ),
        (
            "ListOperations",
            json!([{ "Name": "STATUS", "Values": ["SUCCESS", "FAIL"], "Condition": "IN" }]),
        ),
    ];
    for (operation, filters) in served {
        let (status, json) = call(&router, operation, json!({ "Filters": filters })).await;
        assert_eq!(
            status, 200,
            "case {filters} on {operation}: a supported condition is served, got {status}: {json}"
        );
    }
}

/// RULE 4: the tag-quota refusal is ONE fixed sentence, over every operation that applies
/// the quota and whatever the request carries past it — the sentence names neither the
/// quota, the resource nor the keys.
#[tokio::test]
async fn the_tag_quota_refusal_is_one_fixed_sentence_wherever_it_is_reached() {
    /// The sentence itself, which no request value reaches.
    const ALLOWABLE_TAG_LIMIT: &str = "Allowable tag limit for resource exceeded";
    /// The quota the model documents on `TooManyTagsException`: "The maximum number of
    /// tags that can be applied to a resource is 50."
    const MAX_TAGS: usize = 50;

    let router = emulator();
    let (_, namespace_id) = namespace(&router, "tag-quota-ns").await;
    let namespace_arn = format!(
        "arn:aws:servicediscovery:{REGION}:{}:namespace/{namespace_id}",
        engine::EMULATOR_ACCOUNT_ID
    );

    // One past the quota, and well past it: the sentence does not vary with the count.
    for count in [MAX_TAGS + 1, MAX_TAGS + 37] {
        let tags: Vec<Value> = (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
            .collect();

        // (case, operation, the request)
        let rows: Vec<(&str, &str, Value)> = vec![
            (
                "tagging a stored resource past the quota",
                "TagResource",
                json!({ "ResourceARN": namespace_arn, "Tags": tags }),
            ),
            (
                "creating an HTTP namespace with too many tags",
                "CreateHttpNamespace",
                json!({ "Name": format!("tagged-http-{count}"), "Tags": tags }),
            ),
            (
                "creating a public DNS namespace with too many tags",
                "CreatePublicDnsNamespace",
                json!({ "Name": format!("tagged-{count}.example.com"), "Tags": tags }),
            ),
            (
                "creating a private DNS namespace with too many tags",
                "CreatePrivateDnsNamespace",
                json!({
                    "Name": format!("tagged-{count}.internal"),
                    "Vpc": "vpc-0a1b2c3d",
                    "Tags": tags,
                }),
            ),
            (
                "creating a service with too many tags",
                "CreateService",
                json!({
                    "Name": format!("tagged-svc-{count}"),
                    "NamespaceId": namespace_id,
                    "Tags": tags,
                }),
            ),
        ];

        for (case, operation, body) in rows {
            assert_message(
                &router,
                &format!("{case} ({count} tags)"),
                operation,
                body,
                "TooManyTagsException",
                ALLOWABLE_TAG_LIMIT,
            )
            .await;
        }
    }
}

/// RULE 5: a service-settings update of a service in an API-only (HTTP) namespace is
/// refused with ONE fixed sentence, whatever the request asks to change and whichever
/// service it names. The DNS-namespace rows prove the refusal is the namespace's kind and
/// not the operation itself.
#[tokio::test]
async fn updating_a_service_in_an_api_only_namespace_is_one_fixed_sentence() {
    /// The sentence itself, which names neither the service nor the operation.
    const API_ONLY: &str = "Service in API-only namespace cannot be updated.";

    let router = emulator();
    let (_, http_id) = namespace(&router, "api-only-ns").await;
    let dns_id = dns_namespace(&router, "updatable-ns").await;

    // Two services in the API-only namespace, so the sentence is shown not to carry the
    // service it refused.
    for name in ["api-only-one", "api-only-two"] {
        let service_id = service(&router, &http_id, name).await;
        // (case, the change the request asks for)
        let changes: Vec<(&str, Value)> = vec![
            ("a description alone", json!({ "Description": "changed" })),
            (
                "a description and DNS records",
                json!({
                    "Description": "changed",
                    "DnsConfig": { "DnsRecords": [{ "Type": "A", "TTL": 60 }] },
                }),
            ),
            (
                "a health check alone",
                json!({
                    "HealthCheckConfig": { "Type": "HTTP", "ResourcePath": "/ping" },
                }),
            ),
        ];
        for (case, change) in changes {
            assert_message(
                &router,
                &format!("{case} on {name}"),
                "UpdateService",
                json!({ "Id": service_id, "Service": change }),
                "InvalidInput",
                API_ONLY,
            )
            .await;
        }
    }

    // The same change on a service in a DNS namespace is served: the refusal above is the
    // namespace's kind. The service is created WITH the record the update then changes,
    // because a service's record types are fixed at create.
    let created = ok(
        &router,
        "CreateService",
        json!({
            "Name": "updatable-svc",
            "NamespaceId": dns_id,
            "DnsConfig": { "DnsRecords": [{ "Type": "A", "TTL": 30 }] },
        }),
    )
    .await;
    let service_id = created["Service"]["Id"]
        .as_str()
        .expect("CreateService returns the service it stored")
        .to_string();
    let (status, json) = call(
        &router,
        "UpdateService",
        json!({
            "Id": service_id,
            "Service": {
                "Description": "changed",
                "DnsConfig": { "DnsRecords": [{ "Type": "A", "TTL": 60 }] },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a service in a DNS namespace is updatable, got {status}: {json}"
    );
}

/// Create a private DNS namespace and return its id, through the same create-then-resolve
/// round trip the shared harness uses for the other two namespace kinds.
async fn private_dns_namespace(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(
        router,
        "CreatePrivateDnsNamespace",
        json!({ "Name": name, "Vpc": "vpc-0a1b2c3d" }),
    )
    .await;
    let operation_id = created["OperationId"]
        .as_str()
        .expect("CreatePrivateDnsNamespace returns an OperationId")
        .to_string();
    let filed = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
    filed["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets the namespace it created")
        .to_string()
}
