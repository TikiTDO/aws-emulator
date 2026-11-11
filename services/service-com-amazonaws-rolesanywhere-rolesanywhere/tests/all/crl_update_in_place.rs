// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateCrl` as an IN-PLACE PARTIAL write on the stored
//! certificate revocation list.
//!
//! The rule under test: an update addresses one stored record by id and writes only the
//! members the request carries — a member it omits keeps the value the import or an earlier
//! update stored, a member the operation has no input for is not its to change, `updatedAt`
//! becomes the instant of THIS write while `createdAt` never moves — and the single stored
//! record is what every later path reports. `UpdateCrlRequest` carries `crlId` (`@httpLabel`,
//! `@required`), `name` and `crlData`, and answers a `CrlDetail`
//! (`smithy_query(query="operation", operation="UpdateCrl",
//! model_path="services/service-com-amazonaws-rolesanywhere-rolesanywhere/smithy-model/com-amazonaws-rolesanywhere-rolesanywhere.json")`),
//! so `enabled`, `trustAnchorArn`, the ids and the resource's tag set are outside its reach.
//!
//! Three tables, each differing along what the rule generalises over.
//!
//! * A SEQUENCE of updates on one record, so each step has to compose with what the step
//!   before it wrote rather than with the import: the rows differ in which members they
//!   carry, in the value's size and bytes (both published `crlData` `@length` bounds, a
//!   non-UTF-8 blob) and name length (both published `ResourceName` `@length` bounds), and
//!   one row repeats its predecessor verbatim, so a re-sent write is observable only in
//!   `updatedAt`. Each step is checked on the write's own answer, on `GetCrl`, on the
//!   `ListCrls` projection and on `ListTagsForResource`.
//! * The members the operation has no input for, across rows that differ in the `enabled`
//!   state the record is in when the update lands (left to the import's default, imported
//!   off, imported on), in how many tags it holds and in which trust anchor it links to.
//! * Input outside the PUBLISHED constraints of the three input members, each bound tested
//!   at the bound and one past it, which must be refused and must store nothing.
//!
//! Instants are per-run values and are never asserted literally: what is asserted is that
//! `updatedAt` falls inside the window the test's own clock brackets the call with, which is
//! what makes it the write's instant rather than a copy of an earlier one.
//!
//! The harness below is local to this suite, as in each sibling suite.

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A placeholder CRL in PEM form, as the import stores it.
const CRL_PEM: &str = "-----BEGIN X509 CRL-----\nZW11bGF0ZWQtdGVzdC1jcmw=\n-----END X509 CRL-----\n";

/// The Region the suite signs for.
const REGION: &str = "us-west-2";

/// `ResourceName` is `@length(1, 255)` with `@pattern("^[ a-zA-Z0-9-_]*$")`, and
/// `UpdateCrlRequest$crlData` is a `Blob` `@length(1, 300000)`
/// (`smithy_query(query="shape", shape="UpdateCrlRequest")`).
const NAME_MIN_LENGTH: usize = 1;
const NAME_MAX_LENGTH: usize = 255;
const NAME_PATTERN: &str = "^[ a-zA-Z0-9-_]*$";
const CRL_DATA_MIN_LENGTH: usize = 1;
const CRL_DATA_MAX_LENGTH: usize = 300_000;

/// `Uuid` is `@length(36, 36)` with
/// `@pattern("[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}")`, and `crlId` targets it.
const UUID_LENGTH: usize = 36;
const UUID_PATTERN: &str = "[a-f0-9]{8}-([a-z0-9]{4}-){3}[a-z0-9]{12}";

/// A client over one emulator: one engine, one mounted service, so every request in a test
/// observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body and the wire error code.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Answer {
    fn message(&self) -> &str {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The `crl` member of a CRL operation's answer.
    fn crl(&self) -> Value {
        self.body.get("crl").cloned().unwrap_or(Value::Null)
    }
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how the
            // handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn create_trust_anchor(&self, name: &str) -> Answer {
        self.send(
            "POST",
            "/trustanchors",
            Some(json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            })),
        )
        .await
    }

    async fn import_crl(&self, body: Value) -> Answer {
        self.send("POST", "/crls", Some(body)).await
    }

    async fn get_crl(&self, id: &str) -> Answer {
        self.send("GET", &format!("/crl/{id}"), None).await
    }

    async fn update_crl(&self, id: &str, body: Value) -> Answer {
        self.send("PATCH", &format!("/crl/{id}"), Some(body)).await
    }

    /// The `ListCrls` element whose `crlId` is `id`, over every page.
    async fn listed_crl(&self, case: &str, id: &str) -> Value {
        let mut next: Option<String> = None;
        loop {
            let uri = match &next {
                Some(token) => format!("/crls?pageSize=2&nextToken={token}"),
                None => "/crls?pageSize=2".to_string(),
            };
            let answer = self.send("GET", &uri, None).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected ListCrls 200, got {answer:?}"
            );
            let entries = answer
                .body
                .get("crls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(found) = entries
                .iter()
                .find(|entry| entry.get("crlId").and_then(Value::as_str) == Some(id))
            {
                return found.clone();
            }
            next = answer
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next.is_none() {
                panic!("case {case:?}: ListCrls never reported the CRL {id:?}");
            }
        }
    }

    /// The tag set `ListTagsForResource` reports for an ARN, keyed as the service holds it.
    async fn resource_tags(&self, case: &str, resource_arn: &str) -> BTreeMap<String, String> {
        let answer = self
            .send(
                "GET",
                &format!("/ListTagsForResource?resourceArn={resource_arn}"),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListTagsForResource 200, got {answer:?}"
        );
        answer
            .body
            .get("tags")
            .and_then(Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| {
                        let key = tag.get("key").and_then(Value::as_str)?;
                        let value = tag.get("value").and_then(Value::as_str)?;
                        Some((key.to_string(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Create a trust anchor and answer its ARN.
async fn trust_anchor_arn(client: &Client, name: &str) -> String {
    let created = client.create_trust_anchor(name).await;
    assert_eq!(
        created.status, 201,
        "trust anchor {name:?}: expected CreateTrustAnchor 201, got {created:?}"
    );
    created
        .body
        .pointer("/trustAnchor/trustAnchorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("trust anchor {name:?}: no trustAnchorArn in {created:?}"))
        .to_string()
}

/// A string member of a returned `CrlDetail`.
fn member(case: &str, crl: &Value, name: &str) -> Value {
    crl.get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {crl:?}"))
}

/// The seconds-since-epoch of an ISO-8601 `date-time` member.
fn instant_secs(case: &str, crl: &Value, name: &str) -> f64 {
    let text = member(case, crl, name);
    let text = text
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: {name} must be a date-time string, got {text:?}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {name} {text:?} must parse, got {error}"))
        .as_secs_f64()
}

/// The test's own clock, in the same units a returned instant parses to.
fn clock_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// Import one CRL and answer `(id, arn, the detail the import returned)`.
async fn import(client: &Client, case: &str, body: Value) -> (String, String, Value) {
    let imported = client.import_crl(body).await;
    assert_eq!(
        imported.status, 201,
        "case {case:?}: expected ImportCrl 201, got {imported:?}"
    );
    let crl = imported.crl();
    let id = crl
        .get("crlId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no crlId in {imported:?}"))
        .to_string();
    let arn = crl
        .get("crlArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no crlArn in {imported:?}"))
        .to_string();
    (id, arn, crl)
}

/// One step of the update sequence: the body sent, and the members it replaces.
struct Step {
    case: &'static str,
    body: Value,
    name: Option<String>,
    data: Option<Vec<u8>>,
}

/// A SEQUENCE of updates on ONE record composes: each step writes the members it carries on
/// top of what the step before it left, every member it omits survives, and the write is
/// what the operation's own answer, `GetCrl` and the `ListCrls` projection all report.
#[tokio::test]
async fn update_crl_composes_in_place_across_a_sequence_of_writes() {
    let client = Client::new();
    let anchor = trust_anchor_arn(&client, "sequence-anchor").await;
    let (id, arn, imported) = import(
        &client,
        "import",
        json!({
            "name": "sequence crl",
            "crlData": aws_smithy_types::base64::encode(CRL_PEM.as_bytes()),
            "trustAnchorArn": anchor,
            "enabled": true,
            "tags": [{"key": "owner", "value": "platform"}],
        }),
    )
    .await;
    let imported_tags = client.resource_tags("import", &arn).await;

    let steps: Vec<Step> = vec![
        Step {
            case: "name-only",
            body: json!({"name": "renamed once"}),
            name: Some("renamed once".to_string()),
            data: None,
        },
        Step {
            case: "data-only-keeps-the-new-name",
            body: json!({"crlData": aws_smithy_types::base64::encode(b"second revision")}),
            name: None,
            data: Some(b"second revision".to_vec()),
        },
        Step {
            // A `Blob` is bytes, not text: every byte value has to survive a write.
            case: "binary-data",
            body: json!({
                "crlData": aws_smithy_types::base64::encode((0u8..=255).collect::<Vec<u8>>()),
            }),
            name: None,
            data: Some((0u8..=255).collect()),
        },
        Step {
            case: "both-members-at-the-published-ceilings",
            body: json!({
                "name": "c".repeat(NAME_MAX_LENGTH),
                "crlData": aws_smithy_types::base64::encode(vec![b'z'; CRL_DATA_MAX_LENGTH]),
            }),
            name: Some("c".repeat(NAME_MAX_LENGTH)),
            data: Some(vec![b'z'; CRL_DATA_MAX_LENGTH]),
        },
        Step {
            case: "both-members-at-the-published-floors",
            body: json!({
                "name": "c".repeat(NAME_MIN_LENGTH),
                "crlData": aws_smithy_types::base64::encode(vec![b'-'; CRL_DATA_MIN_LENGTH]),
            }),
            name: Some("c".repeat(NAME_MIN_LENGTH)),
            data: Some(vec![b'-'; CRL_DATA_MIN_LENGTH]),
        },
        Step {
            // The same body again: a re-sent write leaves the same values, so only
            // `updatedAt` can differ.
            case: "the-same-body-again",
            body: json!({
                "name": "c".repeat(NAME_MIN_LENGTH),
                "crlData": aws_smithy_types::base64::encode(vec![b'-'; CRL_DATA_MIN_LENGTH]),
            }),
            name: Some("c".repeat(NAME_MIN_LENGTH)),
            data: Some(vec![b'-'; CRL_DATA_MIN_LENGTH]),
        },
        Step {
            case: "neither-member",
            body: json!({}),
            name: None,
            data: None,
        },
    ];

    let mut expected_name = member("import", &imported, "name")
        .as_str()
        .expect("the import reports a name")
        .to_string();
    let mut expected_data = member("import", &imported, "crlData")
        .as_str()
        .expect("the import reports crlData")
        .to_string();
    let created_at = member("import", &imported, "createdAt");
    let mut previous_updated = instant_secs("import", &imported, "updatedAt");

    for step in &steps {
        let case = step.case;
        if let Some(name) = &step.name {
            expected_name = name.clone();
        }
        if let Some(data) = &step.data {
            expected_data = aws_smithy_types::base64::encode(data);
        }

        let before = clock_secs();
        let answer = client.update_crl(&id, step.body.clone()).await;
        let after = clock_secs();
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateCrl 200, got {answer:?}"
        );
        let crl = answer.crl();

        assert_eq!(
            member(case, &crl, "name").as_str(),
            Some(expected_name.as_str()),
            "case {case:?}: expected the name {expected_name:?} after this step, got {:?}",
            member(case, &crl, "name")
        );
        assert_eq!(
            member(case, &crl, "crlData").as_str(),
            Some(expected_data.as_str()),
            "case {case:?}: expected {} base64 characters of crlData, got {}",
            expected_data.len(),
            member(case, &crl, "crlData")
                .as_str()
                .unwrap_or_default()
                .len()
        );

        // Members this operation has no input for are not its to change, and `createdAt`
        // belongs to the import.
        for untouched in ["crlId", "crlArn", "trustAnchorArn", "enabled", "createdAt"] {
            assert_eq!(
                member(case, &crl, untouched),
                member("import", &imported, untouched),
                "case {case:?}: {untouched} is not this operation's to change, got {crl:?}"
            );
        }
        assert_eq!(
            member(case, &crl, "createdAt"),
            created_at,
            "case {case:?}: createdAt must stay the instant of the import, got {crl:?}"
        );

        // `updatedAt` is "the ISO-8601 timestamp when the CRL was last updated", so it is
        // the instant of THIS call — inside the window the test brackets it with — and it
        // never goes backwards.
        let updated = instant_secs(case, &crl, "updatedAt");
        assert!(
            updated >= before && updated <= after,
            "case {case:?}: updatedAt must be the instant of this write, got {updated} outside \
             [{before}, {after}]"
        );
        assert!(
            updated >= previous_updated,
            "case {case:?}: updatedAt must not precede the previous write's, got {updated} \
             after {previous_updated}"
        );
        assert!(
            updated >= instant_secs(case, &crl, "createdAt"),
            "case {case:?}: updatedAt must not precede createdAt, got {crl:?}"
        );
        previous_updated = updated;

        // One stored record: the read and the list projection report what the write
        // answered, and the resource's tag set is untouched by it.
        let read = client.get_crl(&id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetCrl 200, got {read:?}"
        );
        assert_eq!(
            read.crl(),
            crl,
            "case {case:?}: GetCrl must report the record the update wrote"
        );
        assert_eq!(
            client.listed_crl(case, &id).await,
            crl,
            "case {case:?}: the ListCrls projection must report the record the update wrote"
        );
        assert_eq!(
            client.resource_tags(case, &arn).await,
            imported_tags,
            "case {case:?}: an update must not touch the resource's tag set"
        );
    }
}

/// An update writes NOTHING outside its own input members, whatever state the record is in:
/// its `enabled` flag, its link to a trust anchor, its ids and its tag set all survive.
///
/// The rows differ in the `enabled` state the update lands on, in how many tags the record
/// holds, and in which of two trust anchors it links to.
#[tokio::test]
async fn update_crl_keeps_the_members_it_has_no_input_for() {
    let client = Client::new();
    let first_anchor = trust_anchor_arn(&client, "sidecar-anchor-one").await;
    let second_anchor = trust_anchor_arn(&client, "sidecar anchor two").await;

    /// One row: how the record is imported, and which trust anchor it links to.
    struct Row {
        case: &'static str,
        anchor: String,
        imported_enabled: Option<bool>,
        tags: Vec<(&'static str, &'static str)>,
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "enabled-left-to-the-default-no-tags",
            anchor: first_anchor.clone(),
            imported_enabled: None,
            tags: Vec::new(),
        },
        Row {
            case: "imported-on-one-tag",
            anchor: second_anchor.clone(),
            imported_enabled: Some(true),
            tags: vec![("owner", "platform")],
        },
        Row {
            case: "imported-off-two-tags",
            anchor: first_anchor.clone(),
            imported_enabled: Some(false),
            tags: vec![("owner", "platform"), ("tier", "prod")],
        },
        Row {
            case: "imported-on-three-tags",
            anchor: second_anchor.clone(),
            imported_enabled: Some(true),
            tags: vec![("owner", "platform"), ("tier", "prod"), ("team", "iam")],
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut body = json!({
            "name": format!("sidecar {case}").replace('-', " "),
            "crlData": aws_smithy_types::base64::encode(CRL_PEM.as_bytes()),
            "trustAnchorArn": row.anchor,
            "tags": row
                .tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect::<Vec<Value>>(),
        });
        if let Some(enabled) = row.imported_enabled {
            body["enabled"] = json!(enabled);
        }
        let (id, arn, imported) = import(&client, case, body).await;

        let expected_enabled = member(case, &imported, "enabled");
        assert_eq!(
            expected_enabled,
            json!(row.imported_enabled.unwrap_or(false)),
            "case {case:?}: the import must store the enabled flag it carried, got {imported:?}"
        );
        let tags_before = client.resource_tags(case, &arn).await;
        assert_eq!(
            tags_before.len(),
            row.tags.len(),
            "case {case:?}: the import must store the tags it carried, got {tags_before:?}"
        );

        let new_name = format!("{case} renamed").replace('-', " ");
        let new_data = format!("revision for {case}").into_bytes();
        let answer = client
            .update_crl(
                &id,
                json!({
                    "name": new_name,
                    "crlData": aws_smithy_types::base64::encode(&new_data),
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateCrl 200, got {answer:?}"
        );
        let crl = answer.crl();

        assert_eq!(
            member(case, &crl, "name").as_str(),
            Some(new_name.as_str()),
            "case {case:?}: the update must write the submitted name, got {crl:?}"
        );
        assert_eq!(
            member(case, &crl, "crlData").as_str(),
            Some(aws_smithy_types::base64::encode(&new_data).as_str()),
            "case {case:?}: the update must write the submitted crlData, got {crl:?}"
        );
        assert_eq!(
            member(case, &crl, "enabled"),
            expected_enabled,
            "case {case:?}: the update must leave the enabled flag as it stood, got {crl:?}"
        );
        for untouched in ["crlId", "crlArn", "trustAnchorArn", "createdAt"] {
            assert_eq!(
                member(case, &crl, untouched),
                member(case, &imported, untouched),
                "case {case:?}: {untouched} is not this operation's to change, got {crl:?}"
            );
        }
        assert_eq!(
            client.resource_tags(case, &arn).await,
            tags_before,
            "case {case:?}: the update must leave the tag set as it stood"
        );
        assert_eq!(
            client.get_crl(&id).await.crl(),
            crl,
            "case {case:?}: GetCrl must report the record the update wrote"
        );
    }
}

/// Input outside a PUBLISHED constraint of an update's members is refused as the modelled
/// `ValidationException` (400) naming the member and the bound, and the stored record is
/// left exactly as it was: a refused write stores nothing.
///
/// Every bound of the three input members is covered at the bound (served, in the sequence
/// test above) and one past it here, in both directions where the model gives two.
#[tokio::test]
async fn update_crl_refuses_input_outside_the_published_constraints_and_stores_nothing() {
    let client = Client::new();
    let anchor = trust_anchor_arn(&client, "constraint-anchor").await;
    let (id, _, imported) = import(
        &client,
        "import",
        json!({
            "name": "constraint crl",
            "crlData": aws_smithy_types::base64::encode(CRL_PEM.as_bytes()),
            "trustAnchorArn": anchor,
        }),
    )
    .await;

    // Each row: the path id, the body, and the member plus requirement the refusal must
    // state — the wording the boundary validator words every constraint with.
    let cases: Vec<(&str, String, Value, String)> = vec![
        (
            "name-below-the-length-floor",
            id.clone(),
            json!({"name": ""}),
            format!(
                "at 'name' failed to satisfy constraint: Member must have length greater than or \
                 equal to {NAME_MIN_LENGTH}"
            ),
        ),
        (
            "name-one-past-the-length-ceiling",
            id.clone(),
            json!({"name": "n".repeat(NAME_MAX_LENGTH + 1)}),
            format!(
                "at 'name' failed to satisfy constraint: Member must have length less than or \
                 equal to {NAME_MAX_LENGTH}"
            ),
        ),
        (
            "name-outside-the-published-pattern",
            id.clone(),
            json!({"name": "no!punctuation"}),
            format!(
                "at 'name' failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {NAME_PATTERN}"
            ),
        ),
        (
            "crl-data-below-the-length-floor",
            id.clone(),
            json!({"crlData": aws_smithy_types::base64::encode(Vec::<u8>::new())}),
            format!(
                "at 'crlData' failed to satisfy constraint: Member must have length greater than \
                 or equal to {CRL_DATA_MIN_LENGTH}"
            ),
        ),
        (
            "crl-data-one-past-the-length-ceiling",
            id.clone(),
            json!({
                "crlData": aws_smithy_types::base64::encode(vec![b'y'; CRL_DATA_MAX_LENGTH + 1]),
            }),
            format!(
                "at 'crlData' failed to satisfy constraint: Member must have length less than or \
                 equal to {CRL_DATA_MAX_LENGTH}"
            ),
        ),
        (
            "crl-id-one-character-short",
            "0".repeat(UUID_LENGTH - 1),
            json!({"name": "renamed"}),
            format!(
                "at 'crlId' failed to satisfy constraint: Member must have length greater than or \
                 equal to {UUID_LENGTH}"
            ),
        ),
        (
            "crl-id-one-character-long",
            "0".repeat(UUID_LENGTH + 1),
            json!({"name": "renamed"}),
            format!(
                "at 'crlId' failed to satisfy constraint: Member must have length less than or \
                 equal to {UUID_LENGTH}"
            ),
        ),
        (
            "crl-id-outside-the-published-pattern",
            "0123ABCD-4567-89EF-0123-456789ABCDEF".to_string(),
            json!({"name": "renamed"}),
            format!(
                "at 'crlId' failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: {UUID_PATTERN}"
            ),
        ),
    ];

    for (case, path_id, body, expected_clause) in &cases {
        let answer = client.update_crl(path_id, body.clone()).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: input outside a published constraint must be refused, got {answer:?}"
        );
        assert!(
            answer.message().contains(expected_clause.as_str()),
            "case {case:?}: the refusal must state {expected_clause:?}, got {:?}",
            answer.message()
        );
        let read = client.get_crl(&id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored CRL must still read back, got {read:?}"
        );
        assert_eq!(
            read.crl(),
            imported,
            "case {case:?}: a refused update must store nothing"
        );
    }

    // The control row: a request inside every constraint is served, and only then does the
    // stored record change.
    let served = client
        .update_crl(
            &id,
            json!({
                "name": "n".repeat(NAME_MAX_LENGTH),
                "crlData": aws_smithy_types::base64::encode(vec![b'y'; CRL_DATA_MAX_LENGTH]),
            }),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "case \"inside-every-constraint\": expected UpdateCrl 200, got {served:?}"
    );
    assert_eq!(
        member("inside-every-constraint", &served.crl(), "name").as_str(),
        Some("n".repeat(NAME_MAX_LENGTH).as_str()),
        "case \"inside-every-constraint\": the served update must write the submitted name"
    );
}
