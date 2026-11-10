// SPDX-License-Identifier: CC0-1.0

//! WHICH ORDER a refusal of a value outside an enumerated type lists that type's
//! admissible values in, over the real wire path: every call goes through the router's
//! `dispatch`, so the clause is read exactly as a client reads it.
//!
//! The rule: a value outside an enumerated type is refused HTTP 400 `ValidationException`
//! whose clause names the values that type admits, and those values are listed in the order
//! this front door walks its own table of them — a table keyed on each value's own text,
//! walked bucket by bucket (`service_common::validation_message::hashed_enum_order`) —
//! rather than in the order the model declares them.
//!
//! The order is derived from the values themselves, so it belongs to the enumerated TYPE:
//! one type is listed alike whichever member carries it, in whatever operation, and whatever
//! the request's shape. That is what the rows below walk — the enum, the member, the
//! operation, the depth the value sits at (top level, a structure, a structure inside a
//! list), the binding it arrived in (body or query string), how many values the type admits
//! (one, two, four, five, eight and sixteen) and the form of the rejected value — while the
//! guards at the end state the derivation independently of any request, and the one clause a
//! HANDLER states for a discriminator is checked to list its set the same way.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    DataSourceConfigurationProblem, KnowledgeBaseConfigurationProblem,
};
use service_common::validation_message::{enum_value_bucket, hashed_enum_order};

use crate::flow_definition_validation::Emulator;

/// Every enumerated type a REQUEST member exercised below carries, as the values it
/// declares, in declaration order.
///
/// Source: `smithy_query(query="shape", shape="<Enum>")` for each of `FlowNodeType`
/// (`FlowNode$type`), `FlowNodeIODataType` (`FlowNodeOutput$type`), `FlowConnectionType`
/// (`FlowConnection$type`), `IncludedData` (`GetFlow$includedData`), `KnowledgeBaseType`
/// (`KnowledgeBaseConfiguration$type`), `DataSourceType`
/// (`DataSourceConfiguration$type`), `DataDeletionPolicy`
/// (`CreateDataSource$dataDeletionPolicy`), `IngestionJobSortByAttribute` and `SortOrder`
/// (`IngestionJobSortBy$attribute` / `$order`) and `IngestionJobFilterOperator`
/// (`IngestionJobFilter$operator`).
const MODELLED_ENUMS: &[(&str, &[&str])] = &[
    (
        "FlowNodeType",
        &[
            "Input",
            "Output",
            "KnowledgeBase",
            "Condition",
            "Lex",
            "Prompt",
            "LambdaFunction",
            "Storage",
            "Agent",
            "Retrieval",
            "Iterator",
            "Collector",
            "InlineCode",
            "Loop",
            "LoopInput",
            "LoopController",
        ],
    ),
    (
        "FlowNodeIODataType",
        &["String", "Number", "Boolean", "Object", "Array"],
    ),
    ("FlowConnectionType", &["Data", "Conditional"]),
    ("IncludedData", &["ALL_DATA", "METADATA_ONLY"]),
    ("KnowledgeBaseType", &["VECTOR", "KENDRA", "SQL", "MANAGED"]),
    (
        "DataSourceType",
        &[
            "S3",
            "WEB",
            "CONFLUENCE",
            "SALESFORCE",
            "SHAREPOINT",
            "CUSTOM",
            "REDSHIFT_METADATA",
            "MANAGED_KNOWLEDGE_BASE_CONNECTOR",
        ],
    ),
    ("DataDeletionPolicy", &["RETAIN", "DELETE"]),
    ("IngestionJobSortByAttribute", &["STATUS", "STARTED_AT"]),
    ("SortOrder", &["ASCENDING", "DESCENDING"]),
    ("IngestionJobFilterOperator", &["EQ"]),
];

/// The values `name` declares, read out of [`MODELLED_ENUMS`] so each value set is written
/// in ONE place: a row names the enum, never a second copy of its values.
fn declared(name: &str) -> &'static [&'static str] {
    MODELLED_ENUMS
        .iter()
        .find(|(enumerated, _)| *enumerated == name)
        .map(|(_, values)| *values)
        .unwrap_or_else(|| panic!("case {name:?}: this model declares no such enum"))
}

/// A role ARN matching the modelled `FlowExecutionRoleArn` pattern, so a `CreateFlow`
/// row's only fault is the enum member under test.
const EXECUTION_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern, for the same reason.
const KNOWLEDGE_BASE_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// An identifier matching the modelled `Id`/`FlowIdentifier` pattern (ten alphanumerics):
/// a boundary refusal is answered before any handler looks a resource up, so these rows
/// need no stored resource — the positive controls at the end show the same request with a
/// value INSIDE the set getting past the boundary.
const IDENTIFIER: &str = "0123456789";

/// The whole message a request with ONE enum-constrained member outside its set is refused
/// with: the AWS envelope over the admitted set, with the set put through the SHARED
/// derivation rather than transcribed — so a service that grew a second copy of the order
/// and drifted from it fails here.
///
/// `quoted` is the value the clause echoes back, which is `None` for a member the model
/// marks `@sensitive` (everything inside a `FlowDefinition` is) and `Some` for one it does
/// not.
fn expected_message(quoted: Option<&str>, member: &str, enumerated: &str) -> String {
    let subject = match quoted {
        Some(value) => format!("Value '{value}'"),
        None => "Value".to_string(),
    };
    format!(
        "1 validation error detected: {subject} at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        hashed_enum_order(declared(enumerated)).join(", ")
    )
}

/// The values a clause brackets, read back out of the message it stated.
fn bracketed(case: &str, message: &str) -> Vec<String> {
    let opened = message
        .split_once("enum value set: [")
        .unwrap_or_else(|| {
            panic!("case {case:?}: the message states no enum value set: {message:?}")
        })
        .1;
    let set = opened
        .split_once(']')
        .unwrap_or_else(|| panic!("case {case:?}: the value set is not closed: {message:?}"))
        .0;
    set.split(", ").map(str::to_string).collect()
}

/// One row: a request whose single offending member is outside its enum, the member path
/// the clause is reported at, and the enum whose values that clause has to list.
struct Row {
    case: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    member: String,
    quoted: Option<String>,
    enumerated: &'static str,
}

/// A node declaring one `String` output, with `type` and the output's `type` supplied so a
/// row can put either of them outside its enum.
fn node(name: &str, node_type: &str, output_type: &str) -> Value {
    json!({
        "name": name,
        "type": node_type,
        "configuration": { "input": {} },
        "outputs": [{ "name": "document", "type": output_type }],
    })
}

/// The flow's ending node, declaring one `String` input.
fn output_node() -> Value {
    json!({
        "name": "FlowOutput",
        "type": "Output",
        "configuration": { "output": {} },
        "inputs": [{ "name": "document", "type": "String", "expression": "$.data" }],
    })
}

/// The data connection carrying the first node's output into the last node's input, with
/// `connection_type` supplied so a row can put it outside its enum.
fn connection(connection_type: &str) -> Value {
    json!({
        "name": "InputToOutput",
        "type": connection_type,
        "source": "FlowInput",
        "target": "FlowOutput",
        "configuration": { "data": { "sourceOutput": "document", "targetInput": "document" } },
    })
}

/// A two-node definition whose nodes and connection are well formed apart from whatever a
/// row put outside its enum.
fn definition(first: Value, connection_type: &str) -> Value {
    json!({
        "nodes": [first, output_node()],
        "connections": [connection(connection_type)],
    })
}

/// A `ListIngestionJobs` path: the operation reaching the deepest enum members of this
/// model.
fn ingestion_jobs() -> String {
    format!("/knowledgebases/{IDENTIFIER}/datasources/{IDENTIFIER}/ingestionjobs/")
}

/// Every row: each enumerated type above, through the operations and bindings that carry
/// it, over the forms a rejected value takes.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // `FlowNode$type`, the recorded case's member, at BOTH node positions and over the
    // forms a rejected value takes: an unknown name, the empty string, a declared value
    // recased, a value belonging to ANOTHER enum of this model, a prefix of a declared
    // value and a declared value with trailing space. None of them may reach the listing.
    for rejected in [
        "NotARealFlowNodeType",
        "",
        "input",
        // `FlowConnectionType` declares `Data`; `FlowNodeType` does not.
        "Data",
        "Loop ",
        "LoopContr",
    ] {
        rows.push(Row {
            case: format!("ValidateFlowDefinition with nodes.1.type {rejected:?}"),
            method: "POST",
            uri: "/flows/validate-definition".to_string(),
            body: Some(json!({
                "definition": definition(node("FlowInput", rejected, "String"), "Data"),
            })),
            member: "definition.nodes.1.member.type".to_string(),
            quoted: None,
            enumerated: "FlowNodeType",
        });
    }
    rows.push(Row {
        case: "ValidateFlowDefinition with nodes.2.type outside its enum".to_string(),
        method: "POST",
        uri: "/flows/validate-definition".to_string(),
        body: Some(json!({
            "definition": json!({
                "nodes": [
                    node("FlowInput", "Input", "String"),
                    {
                        "name": "FlowOutput",
                        "type": "NotARealFlowNodeType",
                        "configuration": { "output": {} },
                        "inputs": [
                            { "name": "document", "type": "String", "expression": "$.data" },
                        ],
                    },
                ],
                "connections": [connection("Data")],
            }),
        })),
        member: "definition.nodes.2.member.type".to_string(),
        quoted: None,
        enumerated: "FlowNodeType",
    });

    // The same enum, the same member, carried by an operation that STORES the definition
    // rather than reporting on it: the listing belongs to the type, not to the operation.
    rows.push(Row {
        case: "CreateFlow with nodes.1.type outside its enum".to_string(),
        method: "POST",
        uri: "/flows/".to_string(),
        body: Some(json!({
            "name": "EnumValueSetOrder",
            "executionRoleArn": EXECUTION_ROLE,
            "definition": definition(node("FlowInput", "Inputs", "String"), "Data"),
        })),
        member: "definition.nodes.1.member.type".to_string(),
        quoted: None,
        enumerated: "FlowNodeType",
    });

    // `FlowNodeOutput$type`: a structure inside a list inside a structure inside a list —
    // the deepest enum a flow definition carries.
    rows.push(Row {
        case: "ValidateFlowDefinition with a node output type outside its enum".to_string(),
        method: "POST",
        uri: "/flows/validate-definition".to_string(),
        body: Some(json!({
            "definition": definition(node("FlowInput", "Input", "Text"), "Data"),
        })),
        member: "definition.nodes.1.member.outputs.1.member.type".to_string(),
        quoted: None,
        enumerated: "FlowNodeIODataType",
    });

    // `FlowConnection$type`: a two-valued enum in the definition's other list.
    rows.push(Row {
        case: "ValidateFlowDefinition with a connection type outside its enum".to_string(),
        method: "POST",
        uri: "/flows/validate-definition".to_string(),
        body: Some(json!({
            "definition": definition(node("FlowInput", "Input", "String"), "Dataflow"),
        })),
        member: "definition.connections.1.member.type".to_string(),
        quoted: None,
        enumerated: "FlowConnectionType",
    });

    // `GetFlow$includedData`: the QUERY STRING binding, on a member the model does not mark
    // `@sensitive`, so this clause echoes the value as well as listing the set.
    for rejected in ["NEITHER_VALUE", "all_data"] {
        rows.push(Row {
            case: format!("GetFlow with includedData {rejected:?}"),
            method: "GET",
            uri: format!("/flows/{IDENTIFIER}/?includedData={rejected}"),
            body: None,
            member: "includedData".to_string(),
            quoted: Some(rejected.to_string()),
            enumerated: "IncludedData",
        });
    }

    // `KnowledgeBaseConfiguration$type` and `DataSourceConfiguration$type`: the two
    // discriminators of another resource family, one level into the body.
    rows.push(Row {
        case: "CreateKnowledgeBase with a knowledgeBaseConfiguration type outside its enum"
            .to_string(),
        method: "PUT",
        uri: "/knowledgebases/".to_string(),
        body: Some(json!({
            "name": "EnumValueSetOrder",
            "roleArn": KNOWLEDGE_BASE_ROLE,
            "knowledgeBaseConfiguration": { "type": "VECTORS" },
        })),
        member: "knowledgeBaseConfiguration.type".to_string(),
        quoted: Some("VECTORS".to_string()),
        enumerated: "KnowledgeBaseType",
    });
    rows.push(Row {
        case: "CreateDataSource with a dataSourceConfiguration type outside its enum".to_string(),
        method: "PUT",
        uri: format!("/knowledgebases/{IDENTIFIER}/datasources/"),
        body: Some(json!({
            "name": "EnumValueSetOrder",
            "dataSourceConfiguration": { "type": "S4" },
        })),
        member: "dataSourceConfiguration.type".to_string(),
        quoted: Some("S4".to_string()),
        enumerated: "DataSourceType",
    });

    // `CreateDataSource$dataDeletionPolicy`: a top-level body member of the same request,
    // whose configuration is INSIDE its own set, so one request states one clause.
    rows.push(Row {
        case: "CreateDataSource with a dataDeletionPolicy outside its enum".to_string(),
        method: "PUT",
        uri: format!("/knowledgebases/{IDENTIFIER}/datasources/"),
        body: Some(json!({
            "name": "EnumValueSetOrder",
            "dataSourceConfiguration": { "type": "CUSTOM" },
            "dataDeletionPolicy": "KEEP",
        })),
        member: "dataDeletionPolicy".to_string(),
        quoted: Some("KEEP".to_string()),
        enumerated: "DataDeletionPolicy",
    });

    // `IngestionJobSortBy$attribute` / `$order`: two enums of one structure, so a row states
    // which of the two the clause is about.
    rows.push(Row {
        case: "ListIngestionJobs with a sortBy attribute outside its enum".to_string(),
        method: "POST",
        uri: ingestion_jobs(),
        body: Some(json!({ "sortBy": { "attribute": "WHEN", "order": "ASCENDING" } })),
        member: "sortBy.attribute".to_string(),
        quoted: Some("WHEN".to_string()),
        enumerated: "IngestionJobSortByAttribute",
    });
    rows.push(Row {
        case: "ListIngestionJobs with a sortBy order outside its enum".to_string(),
        method: "POST",
        uri: ingestion_jobs(),
        body: Some(json!({ "sortBy": { "attribute": "STATUS", "order": "Ascending" } })),
        member: "sortBy.order".to_string(),
        quoted: Some("Ascending".to_string()),
        enumerated: "SortOrder",
    });

    // `IngestionJobFilter$operator`: a SINGLE-valued enum, inside a structure inside a list.
    rows.push(Row {
        case: "ListIngestionJobs with a filter operator outside its enum".to_string(),
        method: "POST",
        uri: ingestion_jobs(),
        body: Some(json!({
            "filters": [{ "attribute": "STATUS", "operator": "NE", "values": ["COMPLETE"] }],
        })),
        member: "filters.1.member.operator".to_string(),
        quoted: Some("NE".to_string()),
        enumerated: "IngestionJobFilterOperator",
    });

    rows
}

#[tokio::test]
async fn an_enum_refusal_lists_the_values_in_the_order_the_front_door_states_them() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = &row.case;
        // Repeated, because a listing read off a container walked in a per-process order
        // could differ between two refusals of the same request.
        for attempt in 1..=2 {
            let refused = emulator
                .call(row.method, &row.uri, row.body.clone())
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?} attempt {attempt}: expected 400 ValidationException, got {refused:?}"
            );
            let message = refused.message(case);
            let expected = expected_message(row.quoted.as_deref(), &row.member, row.enumerated);
            assert_eq!(
                message, expected,
                "case {case:?} attempt {attempt}: expected the refusal {expected:?}, \
                 got {message:?}"
            );
            // Whatever the order, the membership is the model's: the clause neither adds a
            // value the model does not declare nor drops one it does.
            let stated: BTreeSet<String> = bracketed(case, &message).into_iter().collect();
            let modelled: BTreeSet<String> = declared(row.enumerated)
                .iter()
                .map(|value| (*value).to_string())
                .collect();
            assert_eq!(
                stated, modelled,
                "case {case:?} attempt {attempt}: the set stated must hold exactly the values \
                 the model declares, got {message:?}"
            );
        }
    }
}

/// The positive control for every row above: the same requests with the member's value
/// INSIDE its set get past the boundary, so each row is refused for its enum and not for
/// something every request of that shape fails.
#[tokio::test]
async fn a_value_inside_the_set_is_not_refused_at_the_boundary() {
    let emulator = Emulator::new();

    for (case, method, uri, body, expected_status) in [
        (
            "ValidateFlowDefinition with a declared node type",
            "POST",
            "/flows/validate-definition".to_string(),
            Some(json!({
                "definition": definition(node("FlowInput", "Input", "String"), "Data"),
            })),
            200,
        ),
        (
            "CreateFlow with a declared node type",
            "POST",
            "/flows/".to_string(),
            Some(json!({
                "name": "EnumValueSetOrder",
                "executionRoleArn": EXECUTION_ROLE,
                "definition": definition(node("FlowInput", "Input", "String"), "Data"),
            })),
            201,
        ),
        (
            // No flow is stored, so a declared `includedData` reaches the handler and is
            // answered for the flow that is not there rather than at the boundary.
            "GetFlow with a declared includedData",
            "GET",
            format!("/flows/{IDENTIFIER}/?includedData=METADATA_ONLY"),
            None,
            404,
        ),
        (
            // Likewise: the knowledge base this data source would belong to is not there.
            "CreateDataSource with a declared type and deletion policy",
            "PUT",
            format!("/knowledgebases/{IDENTIFIER}/datasources/"),
            Some(json!({
                "name": "EnumValueSetOrder",
                "dataSourceConfiguration": { "type": "CUSTOM" },
                "dataDeletionPolicy": "RETAIN",
            })),
            404,
        ),
        (
            // `ListIngestionJobs` is still scaffolded, so values INSIDE their sets are
            // answered 501 rather than 404 — which is itself the positive control for its
            // rows above: a declared value reaches the (unimplemented) handler, and only a
            // value outside the set is refused at the boundary.
            "ListIngestionJobs with a declared sortBy and filter",
            "POST",
            ingestion_jobs(),
            Some(json!({
                "sortBy": { "attribute": "STATUS", "order": "ASCENDING" },
                "filters": [{ "attribute": "STATUS", "operator": "EQ", "values": ["COMPLETE"] }],
            })),
            501,
        ),
    ] {
        let answer = emulator.call(method, &uri, body).await;
        assert_ne!(
            answer.status, 400,
            "case {case:?}: a value inside the set must not be refused at the boundary, \
             got {answer:?}"
        );
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
    }
}

/// GUARD on the expectations above, which read their listing from the shared derivation:
/// the `FlowNodeType` set is listed `[LoopController, Output, Loop, Prompt, Agent,
/// LoopInput, InlineCode, Input, Collector, Iterator, KnowledgeBase, Retrieval, Storage,
/// Condition, LambdaFunction, Lex]`, which is the listing a recorded refusal of a
/// `FlowNode$type` outside the set carries — and the model declares that set
/// `[Input, Output, KnowledgeBase, …, LoopController]`, so the listing is NOT the
/// declaration.
#[test]
fn the_flow_node_type_value_set_is_listed_as_the_recorded_refusal_lists_it() {
    const RECORDED: &[&str] = &[
        "LoopController",
        "Output",
        "Loop",
        "Prompt",
        "Agent",
        "LoopInput",
        "InlineCode",
        "Input",
        "Collector",
        "Iterator",
        "KnowledgeBase",
        "Retrieval",
        "Storage",
        "Condition",
        "LambdaFunction",
        "Lex",
    ];

    // Four pairs of this type's values share a bucket of the table (`Loop`/`Prompt`,
    // `LoopInput`/`InlineCode`, `Storage`/`Condition`, `LambdaFunction`/`Lex`), and a
    // bucket holding two values is read from the head each was pushed onto, so for THOSE
    // pairs the listing follows the order the values were declared in — which is why the
    // listing is derived from the model's declaration and not from any other presentation
    // of the same set.
    let model = declared("FlowNodeType");
    let listed = hashed_enum_order(model);
    assert_eq!(
        listed, RECORDED,
        "case the model's declaration order: expected {RECORDED:?}, got {listed:?}"
    );
    assert_ne!(
        listed,
        model.to_vec(),
        "case the declaration order: the listing must not be the model's declaration, got {:?}",
        model
    );
}

/// The two clauses a HANDLER states for a discriminator, which the rows above cannot reach
/// over the wire: `KnowledgeBaseConfiguration$type` and `DataSourceConfiguration$type` both
/// target their enum in the model, so the boundary refuses a value outside the set before
/// the handler judges which sibling member that kind needs. They state the same sentence as
/// the boundary, so they list the set the same way — asserted here directly on the state
/// module's refusal so the two orders cannot drift apart unobserved.
///
/// The `DataSourceType` row also states that the listing does not depend on the order the
/// handler's own table of kinds happens to hold: that table pairs each kind with its
/// configuration member in an order of its own, and the expectation is derived from the
/// MODEL's declaration.
#[test]
fn a_handler_stated_discriminator_clause_lists_the_set_the_same_way() {
    for (case, message, member, enumerated, rejected) in [
        (
            "an unknown knowledgeBaseConfiguration type",
            KnowledgeBaseConfigurationProblem::UnknownType("VECTORS".to_string()).message(),
            "knowledgeBaseConfiguration.type",
            "KnowledgeBaseType",
            "VECTORS",
        ),
        (
            "an unknown dataSourceConfiguration type",
            DataSourceConfigurationProblem::UnknownType("S4".to_string()).message(),
            "dataSourceConfiguration.type",
            "DataSourceType",
            "S4",
        ),
    ] {
        let expected = format!(
            "Value '{rejected}' at '{member}' failed to satisfy constraint: \
             Member must satisfy enum value set: [{}]",
            hashed_enum_order(declared(enumerated)).join(", ")
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
    }
}

/// GUARD on the derivation itself, over EVERY enumerated type the rows above carry: the
/// listing holds exactly the values the type admits — none added, none dropped, none
/// repeated — and, for a type no two of whose values share a bucket of the table, it does
/// not depend on the order the values were presented in.
#[test]
fn every_modelled_value_set_is_listed_whole_and_independently_of_its_presentation() {
    for (name, values) in MODELLED_ENUMS {
        let listed = hashed_enum_order(values);
        let mut listed_sorted = listed.clone();
        listed_sorted.sort_unstable();
        let mut declared_sorted = values.to_vec();
        declared_sorted.sort_unstable();
        assert_eq!(
            listed_sorted, declared_sorted,
            "case {name:?}: the listing must hold exactly the declared values, got {listed:?}"
        );

        // Two values in one bucket are read in the order they were pushed onto it, so the
        // claim below is made only for a type whose values sit in buckets of their own.
        let mut buckets: Vec<u32> = values.iter().map(|value| enum_value_bucket(value)).collect();
        buckets.sort_unstable();
        if buckets.windows(2).any(|pair| pair[0] == pair[1]) {
            continue;
        }
        let reversed: Vec<&str> = values.iter().rev().copied().collect();
        let from_reversed = hashed_enum_order(&reversed);
        assert_eq!(
            from_reversed, listed,
            "case {name:?}: the listing must not depend on the presentation, got \
             {from_reversed:?} from the reversed set and {listed:?} from the declared one"
        );
    }
}
