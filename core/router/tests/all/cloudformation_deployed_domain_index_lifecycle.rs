// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack deploys an `AWS::OpenSearchService::Domain` whose access policy is
//! declared the way a template declares a policy document, and the domain the stack leaves
//! behind serves the owning service's own index control plane for a whole index lifecycle.
//!
//! The rule under test: **a resource provisioner forwards every property under the TYPE the
//! owning service's API member is modelled with — a property whose member is modelled a
//! string carrying a JSON document is sent as that document's TEXT, with its intrinsics
//! already resolved — so the stack reaches `CREATE_COMPLETE`, publishes its outputs, and the
//! resource it created is an ordinary resource of that service, answering that service's own
//! APIs exactly as a directly created one does.**
//!
//! `AWS::OpenSearchService::Domain`'s `AccessPolicies` is such a property:
//! `CreateDomain$AccessPolicies` targets `PolicyDocument`, a string
//! (`smithy_query(query="operation", operation="CreateDomain")`), while a template writes an
//! access policy as the policy itself. A sibling suite
//! ([`crate::cloudformation_opensearch_domain_access_policies`]) owns what the forwarded
//! DOCUMENT is; this one owns the second half of the rule — that the deployed domain is then
//! usable — which is the half nothing could observe while such a stack never created at all:
//! `CreateIndex` reports `CREATED`, `GetIndex` reflects the mapping it was given,
//! `UpdateIndex` reports `UPDATED` and MERGES onto what was there, `DeleteIndex` reports
//! `DELETED`, and `GetIndex`/`UpdateIndex` afterwards refuse with the modelled
//! `ResourceNotFoundException`.
//!
//! The rows differ along what the rule generalises over: the FORM the policy property is
//! written in (an object carrying intrinsics over a template parameter, the quoted text of a
//! document, and no such property at all), which schema sections the index carries (mappings
//! only, settings beside a nested object field, aliases), what the update writes (a sibling
//! property, a property nested inside an existing one, a whole section the create never
//! carried), the JSON value types inside both documents, and the index name's length and
//! character class. The recorded case — an object policy over `${AWS::Partition}`/
//! `${AWS::AccountId}` with a `title: {type: text}` mapping later given `body:
//! {type: keyword}` — is one row of that table.
//!
//! Two services have to agree for the rule to hold (CloudFormation makes the create, Amazon
//! OpenSearch Service serves it and then serves the index calls), so the suite lives here
//! rather than in either service's crate. The wire helpers and the stack poll come from
//! [`crate::cloudformation_provisions_opensearch_domains`]; this suite adds no second copy of
//! them, only the index calls and the one reader of a restJson1 error code they do not have.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    INDEX_STATUS_CREATED, INDEX_STATUS_DELETED, INDEX_STATUS_UPDATED,
};

use crate::cloudformation_provisions_opensearch_domains::{
    build_router, cloudformation, describe_domain, opensearch, resource_reasons, settle_stack,
    xml_text, MISSING_RESOURCE_STATUS,
};
use crate::query_wire::outputs_of;

/// The engine release every row deploys: one the owning service's catalogue offers, so the
/// only thing a row can fail on is what this suite is about.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The template parameter each row's domain name is passed in under, so `DomainName` reaches
/// the provisioner through a `Ref` and the policy document through an `Fn::Sub` variable —
/// the way a deploy names the domain it is creating.
const DOMAIN_NAME_PARAMETER: &str = "DomainNameParam";

/// The marker a row's expected policy document carries where the ARN the OWNING service
/// minted for the domain belongs, substituted from the stack's own outputs before the
/// comparison — never spelled out, since that ARN is the service's to mint.
const DOMAIN_ARN: &str = "@DOMAIN_ARN@";

/// The modelled error a read or a write of an index the domain does not hold is refused with,
/// and the status the owning service answers that refusal on: `ResourceNotFoundException`
/// {client, httpError 409} on `GetIndex`, `UpdateIndex` and `DeleteIndex`
/// (`smithy_query(query="operation", operation="GetIndex,UpdateIndex,DeleteIndex")`), carried
/// on the status that service answers EVERY missing resource on
/// ([`MISSING_RESOURCE_STATUS`]) — which no `@httpError` annotation of its model expresses.
const INDEX_ABSENT: (u16, &str) = (MISSING_RESOURCE_STATUS, "ResourceNotFoundException");

/// The wire error code of a refused `restJson1` answer: the `__type` of the error envelope,
/// without the namespace an envelope may qualify the shape with.
///
/// The shared caller above answers with the parsed body rather than the headers, and this is
/// the body half of the same discriminator.
fn error_code(body: &Value) -> String {
    let raw = body.get("__type").and_then(Value::as_str).unwrap_or_default();
    raw.split('#').next_back().unwrap_or(raw).to_string()
}

/// `CreateIndex`: `POST /2021-01-01/opensearch/domain/{DomainName}/index`.
async fn create_index(
    router: &MultiServiceRouter,
    domain: &str,
    index: &str,
    schema: &Value,
) -> (u16, Value) {
    opensearch(
        router,
        "POST",
        &format!("/2021-01-01/opensearch/domain/{domain}/index"),
        Some(json!({ "IndexName": index, "IndexSchema": schema })),
    )
    .await
}

/// `GetIndex`: `GET /2021-01-01/opensearch/domain/{DomainName}/index/{IndexName}`.
async fn get_index(router: &MultiServiceRouter, domain: &str, index: &str) -> (u16, Value) {
    opensearch(
        router,
        "GET",
        &format!("/2021-01-01/opensearch/domain/{domain}/index/{index}"),
        None,
    )
    .await
}

/// `UpdateIndex`: `PUT /2021-01-01/opensearch/domain/{DomainName}/index/{IndexName}`.
async fn update_index(
    router: &MultiServiceRouter,
    domain: &str,
    index: &str,
    schema: &Value,
) -> (u16, Value) {
    opensearch(
        router,
        "PUT",
        &format!("/2021-01-01/opensearch/domain/{domain}/index/{index}"),
        Some(json!({ "IndexSchema": schema })),
    )
    .await
}

/// `DeleteIndex`: `DELETE /2021-01-01/opensearch/domain/{DomainName}/index/{IndexName}`.
async fn delete_index(router: &MultiServiceRouter, domain: &str, index: &str) -> (u16, Value) {
    opensearch(
        router,
        "DELETE",
        &format!("/2021-01-01/opensearch/domain/{domain}/index/{index}"),
        None,
    )
    .await
}

/// One row: the stack, the policy property as the template writes it and the document the
/// domain must then hold, plus the index lifecycle driven against the deployed domain.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The domain the stack's parameter names, inside `DomainName`'s own
    /// `^[a-z][a-z0-9\-]+$` and 3..28 characters.
    domain: &'static str,
    /// `AccessPolicies` as the template declares it, or `None` for a declaration carrying no
    /// such property at all.
    declared: Option<Value>,
    /// The document the domain must carry once the property is resolved and sent, with
    /// [`DOMAIN_ARN`] standing for the ARN the owning service minted; `None` where the row
    /// declares no policy.
    expected_policy: Option<Value>,
    /// The index the lifecycle runs on, inside `IndexName`'s `@length(min: 1, max: 255)`.
    index: &'static str,
    /// The schema the index is created with, which `GetIndex` must then report verbatim.
    created: Value,
    /// The schema the update writes.
    update: Value,
    /// What the index must carry after the update: the create's document with the update
    /// merged onto it.
    merged: Value,
}

/// The table: one row per way a deploy of this type differs from another in the FORM of the
/// policy property, and per way the index work on the deployed domain differs.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded form: a policy OBJECT whose principal and resource are built with
            // `Fn::Sub` over the pseudo parameters and the stack's own parameter, with the
            // mapping the recorded deploy then creates and updates.
            case: "an object policy over the account's root, then a mapping given a second field",
            stack: "sim-cfnidx-object-stack",
            domain: "sim-cfnidx-object",
            declared: Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {
                        "AWS": { "Fn::Sub": "arn:${AWS::Partition}:iam::${AWS::AccountId}:root" },
                    },
                    "Action": "es:*",
                    "Resource": {
                        "Fn::Sub":
                            "arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/${DomainNameParam}/*",
                    },
                }],
            })),
            expected_policy: Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": "@PRINCIPAL@" },
                    "Action": "es:*",
                    "Resource": "@DOMAIN_ARN@/*",
                }],
            })),
            index: "sim2real-idx-behav",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({ "mappings": { "properties": { "body": { "type": "keyword" } } } }),
            merged: json!({
                "mappings": {
                    "properties": {
                        "title": { "type": "text" },
                        "body": { "type": "keyword" },
                    },
                },
            }),
        },
        Row {
            // The other FORM the property is written in: the quoted TEXT of a document, which
            // is already what the member takes. The update writes a property NESTED inside one
            // the create made, beside settings it must not disturb.
            case: "a policy written as the quoted text of a document, then a nested field added",
            stack: "sim-cfnidx-text-stack",
            domain: "sim-cfnidx-text",
            declared: Some(Value::String(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "AWS": "*" },
                        "Action": ["es:ESHttpGet", "es:ESHttpHead"],
                        "Resource": "*",
                    }],
                })
                .to_string(),
            )),
            expected_policy: Some(json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "AWS": "*" },
                    "Action": ["es:ESHttpGet", "es:ESHttpHead"],
                    "Resource": "*",
                }],
            })),
            index: "logs-2026.02_v1",
            created: json!({
                "settings": { "number_of_shards": 2, "number_of_replicas": 0 },
                "mappings": {
                    "properties": {
                        "host": { "type": "keyword" },
                        "payload": {
                            "type": "object",
                            "properties": { "status": { "type": "integer" } },
                        },
                    },
                },
            }),
            update: json!({
                "mappings": {
                    "properties": {
                        "payload": { "properties": { "latency_ms": { "type": "float" } } },
                    },
                },
            }),
            merged: json!({
                "settings": { "number_of_shards": 2, "number_of_replicas": 0 },
                "mappings": {
                    "properties": {
                        "host": { "type": "keyword" },
                        "payload": {
                            "type": "object",
                            "properties": {
                                "status": { "type": "integer" },
                                "latency_ms": { "type": "float" },
                            },
                        },
                    },
                },
            }),
        },
        Row {
            // A declaration carrying no such property at all: nothing is sent for it, and the
            // deployed domain serves the index calls just the same. The shortest name its
            // length bound allows, and an update writing a SECTION the create never carried.
            case: "a domain declaring no access policy, then an update adding a new section",
            stack: "sim-cfnidx-none-stack",
            domain: "sim-cfnidx-none",
            declared: None,
            expected_policy: None,
            index: "a",
            created: json!({
                "aliases": { "current": {} },
                "mappings": { "properties": { "amount": { "type": "double" } } },
            }),
            update: json!({ "settings": { "refresh_interval": "30s" } }),
            merged: json!({
                "aliases": { "current": {} },
                "mappings": { "properties": { "amount": { "type": "double" } } },
                "settings": { "refresh_interval": "30s" },
            }),
        },
    ]
}

/// The template one row deploys: the domain, named through the stack's parameter, with the
/// identifiers the owning service mints as outputs.
fn template(row: &Row) -> String {
    let mut properties = Map::new();
    properties.insert(
        "DomainName".to_string(),
        json!({ "Ref": DOMAIN_NAME_PARAMETER }),
    );
    properties.insert("EngineVersion".to_string(), json!(ENGINE_VERSION));
    properties.insert(
        "ClusterConfig".to_string(),
        json!({
            "InstanceType": "t3.small.search",
            "InstanceCount": 1,
            "DedicatedMasterEnabled": false,
            "ZoneAwarenessEnabled": false,
        }),
    );
    properties.insert(
        "EBSOptions".to_string(),
        json!({ "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 }),
    );
    if let Some(declared) = row.declared.clone() {
        properties.insert("AccessPolicies".to_string(), declared);
    }

    let mut parameters = Map::new();
    parameters.insert(
        DOMAIN_NAME_PARAMETER.to_string(),
        json!({ "Type": "String" }),
    );

    json!({
        "Parameters": Value::Object(parameters),
        "Resources": {
            "Domain": {
                "Type": "AWS::OpenSearchService::Domain",
                "Properties": Value::Object(properties),
            },
        },
        "Outputs": {
            "DomainRef": { "Value": { "Ref": "Domain" } },
            "DomainArn": { "Value": { "Fn::GetAtt": ["Domain", "Arn"] } },
            "DomainEndpoint": { "Value": { "Fn::GetAtt": ["Domain", "DomainEndpoint"] } },
        },
    })
    .to_string()
}

/// A row's expected policy document with the identifiers the owning services minted put in
/// place of the markers it carries: the domain's own ARN, and the account root the `Fn::Sub`
/// principal resolves to — which is read OUT of that ARN rather than spelled here, so the
/// account this emulator mints ARNs for is never restated.
fn expected_policy(expected: &Value, domain_arn: &str) -> Value {
    let partition = domain_arn.split(':').nth(1).unwrap_or_default();
    let account = domain_arn.split(':').nth(4).unwrap_or_default();
    let filled = expected
        .to_string()
        .replace(DOMAIN_ARN, domain_arn)
        .replace(
            "@PRINCIPAL@",
            &format!("arn:{partition}:iam::{account}:root"),
        );
    serde_json::from_str(&filled).expect("the row's expected document is JSON")
}

/// Deploy one row's stack and answer the outputs a completed deployment published.
async fn deploy(router: &MultiServiceRouter, row: &Row) -> BTreeMap<String, String> {
    let case = row.case;
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", row.stack),
            ("TemplateBody", &template(row)),
            ("Parameters.member.1.ParameterKey", DOMAIN_NAME_PARAMETER),
            ("Parameters.member.1.ParameterValue", row.domain),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
    );

    let described = settle_stack(router, case, row.stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus"),
        "CREATE_COMPLETE",
        "case {case:?}: a declared policy document must not fail the resource, got {described} \
         with reasons {:?}",
        resource_reasons(router, row.stack).await
    );

    let outputs: BTreeMap<String, String> = outputs_of(&described);
    for key in ["DomainRef", "DomainArn", "DomainEndpoint"] {
        assert!(
            outputs.get(key).is_some_and(|value| !value.is_empty()),
            "case {case:?}: expected {key} to carry a resolved value, got {outputs:?}"
        );
    }
    assert_eq!(
        outputs.get("DomainRef").map(String::as_str),
        Some(row.domain),
        "case {case:?}: Ref returns the domain name the parameter named, got {outputs:?}"
    );
    outputs
}

/// Every row's stack deploys, the domain carries its policy under the member's modelled type,
/// and that domain then serves the whole index lifecycle the owning service models.
#[tokio::test]
async fn a_deployed_domain_serves_the_index_control_plane() {
    let router = build_router();

    for row in rows() {
        let case = row.case;
        let outputs = deploy(&router, &row).await;
        let domain_arn = outputs
            .get("DomainArn")
            .map(String::as_str)
            .unwrap_or_default();

        // The domain the stack reports is a real, settled domain of the owning service, read
        // back through that service's own descriptor rather than off the stack.
        let (status, reported) = describe_domain(&router, row.domain).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the deployed domain to read back, got {status} with \
             {reported}"
        );
        assert_eq!(
            reported.get("Created").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: a completed stack must leave a created domain, got {reported}"
        );
        assert_eq!(
            reported.get("Processing").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a stack reported complete must not still be provisioning its \
             domain, got {reported}"
        );
        assert_eq!(
            reported.get("ARN").and_then(Value::as_str),
            Some(domain_arn),
            "case {case:?}: expected the output to be the service's own ARN, got {reported}"
        );

        // The policy member is modelled a STRING, so a document the template wrote as an
        // object reaches the domain as that document's text, with its intrinsics resolved.
        let carried = reported.get("AccessPolicies");
        match &row.expected_policy {
            // A declaration carrying no policy leaves the domain with the one the SERVICE
            // sets for a create that named none, which is the empty document.
            None => assert_eq!(
                carried.and_then(Value::as_str),
                Some(""),
                "case {case:?}: a domain declaring no policy holds the empty document, got \
                 {carried:?}"
            ),
            Some(expected) => {
                let text = carried.and_then(Value::as_str).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the member is modelled a string, so the domain must \
                         carry its text, got {carried:?}"
                    )
                });
                let parsed: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                    panic!(
                        "case {case:?}: expected the carried text to be the document's own \
                         JSON, got {text:?} ({error})"
                    )
                });
                assert_eq!(
                    parsed,
                    expected_policy(expected, domain_arn),
                    "case {case:?}: expected the resolved document, got {text:?}"
                );
            }
        }

        // A create on the deployed domain reports the status its enum models for one, and the
        // schema reads back as it was given.
        let (status, body) = create_index(&router, row.domain, row.index, &row.created).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateIndex on the deployed domain, got {status} \
             with {body}"
        );
        assert_eq!(
            body.get("Status").and_then(Value::as_str),
            Some(INDEX_STATUS_CREATED),
            "case {case:?}: expected the status of a create, got {body}"
        );
        let (status, body) = get_index(&router, row.domain, row.index).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIndex, got {status} with {body}"
        );
        assert_eq!(
            body.get("IndexSchema"),
            Some(&row.created),
            "case {case:?}: expected the created schema to read back, got {body}"
        );

        // An update MERGES: what it names is written and what it does not name stays.
        let (status, body) = update_index(&router, row.domain, row.index, &row.update).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateIndex, got {status} with {body}"
        );
        assert_eq!(
            body.get("Status").and_then(Value::as_str),
            Some(INDEX_STATUS_UPDATED),
            "case {case:?}: expected the status of an update, got {body}"
        );
        let (status, body) = get_index(&router, row.domain, row.index).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetIndex after the update, got {status} with \
             {body}"
        );
        assert_eq!(
            body.get("IndexSchema"),
            Some(&row.merged),
            "case {case:?}: expected the update to merge onto the created schema, got {body}"
        );

        // And a delete removes it, after which a read and a write of that name are refused
        // with the modelled error rather than answered.
        let (status, body) = delete_index(&router, row.domain, row.index).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DeleteIndex, got {status} with {body}"
        );
        assert_eq!(
            body.get("Status").and_then(Value::as_str),
            Some(INDEX_STATUS_DELETED),
            "case {case:?}: expected the status of a delete, got {body}"
        );
        let (status, body) = get_index(&router, row.domain, row.index).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            INDEX_ABSENT,
            "case {case:?}: expected GetIndex on the deleted index to be refused, got {status} \
             with {body}"
        );
        let (status, body) = update_index(&router, row.domain, row.index, &row.update).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            INDEX_ABSENT,
            "case {case:?}: expected UpdateIndex on the deleted index to be refused, got \
             {status} with {body}"
        );
    }
}
