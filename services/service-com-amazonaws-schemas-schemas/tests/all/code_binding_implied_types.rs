// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH types a code binding's source declares.
//!
//! RULE under test: the archive `GetCodeBindingSource` serves carries one generated
//! source file per type the stored document IMPLIES — the schemas it names, and the
//! nested structures its members declare inline, which it never names — so a client that
//! unpacks it has a declaration for every type its handler can reach. An inline
//! declaration is read wherever it sits: directly under a member, under the `items` of a
//! list, and nested inside another inline declaration, however deep. An object that
//! declares no member of its own is free-form rather than a type, so it still gets no
//! file.
//!
//! The naming of a type the document leaves unnamed is this emulator's, stated once in
//! the generator (`src/ops/get_code_binding_source.rs`) and pinned here: the member's
//! own name, qualified by the structure that member belongs to where that name is
//! already some type's.
//!
//! What each declaration SAYS about its members — the JSON Schema type to language type
//! mapping and the package layout — is the sibling suite
//! [`crate::code_binding_source`]; the binding's own lifecycle is
//! [`crate::code_binding_lifecycle`]. Every case here drives the real wire path: a
//! signed `restJson1` request through `MultiServiceRouter::dispatch`, on the harness in
//! [`crate::registry_schema_lifecycle`].

use std::io::{Cursor, Read};
use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::CODE_BINDING_LANGUAGES;

use crate::registry_schema_lifecycle::{create_registry, schema_uri, Answer, Emulator};

/// The registry every case below creates its schemas in.
const REGISTRY: &str = "implied-types-registry";

/// How long a generation is given to produce source before a case gives up. Generous:
/// the service fixes the due instant at the put and nothing here races it, so this only
/// has to outlast a loaded host.
const GENERATION_DEADLINE: Duration = Duration::from_secs(60);

/// How often the source is asked for again while generation is still in progress.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// One document, and the types it implies.
struct DocumentCase {
    case: &'static str,
    schema: &'static str,
    schema_type: &'static str,
    document: &'static str,
    /// Every type the document implies, in the order the generator reads them: the
    /// schemas it names first, then each type hoisted out of a member's declaration.
    types: &'static [&'static str],
    /// The types among them the document never NAMES — each declared inline under a
    /// member — which are what this suite is about.
    implied: &'static [&'static str],
}

/// The table: one row per place an inline declaration can sit, so the rows differ along
/// WHERE the unnamed type is declared (under a member, under a list's items, inside
/// another inline declaration), how DEEP the nesting goes (one, two and three levels),
/// how MANY types one document implies (one, two, three), which document language
/// declares it (OpenAPI 3 against draft-04), whether the name it would take is already
/// some other type's, and whether the declaration carries members at all.
fn document_cases() -> Vec<DocumentCase> {
    vec![
        DocumentCase {
            case: "inline under a member",
            schema: "inline.member",
            schema_type: "OpenApi3",
            document: r##"{"openapi":"3.0.0","components":{"schemas":{
              "AWSEvent":{"type":"object","required":["detail"],"properties":{
                "detail-type":{"type":"string"},
                "detail":{"type":"object","properties":{
                  "id":{"type":"string"},"quantity":{"type":"integer"}}}}}}}}"##,
            types: &["AWSEvent", "Detail"],
            implied: &["Detail"],
        },
        DocumentCase {
            case: "inline under a list's items",
            schema: "inline.array",
            schema_type: "OpenApi3",
            document: r##"{"openapi":"3.0.0","components":{"schemas":{
              "Record":{"type":"object","properties":{
                "resources":{"type":"array","items":{"type":"object","properties":{
                  "ARN":{"type":"string"},"accountId":{"type":"string"}}}}}}}}}"##,
            types: &["Record", "Resources"],
            implied: &["Resources"],
        },
        DocumentCase {
            case: "inline inside inline, three deep",
            schema: "inline.nested",
            schema_type: "OpenApi3",
            document: r##"{"openapi":"3.0.0","components":{"schemas":{
              "AWSEvent":{"type":"object","properties":{
                "detail":{"type":"object","properties":{
                  "eventName":{"type":"string"},
                  "userIdentity":{"type":"object","properties":{
                    "principalId":{"type":"string"},
                    "sessionContext":{"type":"object","properties":{
                      "creationDate":{"type":"string","format":"date-time"}}}}}}}}}}}}"##,
            types: &["AWSEvent", "Detail", "UserIdentity", "SessionContext"],
            implied: &["Detail", "UserIdentity", "SessionContext"],
        },
        DocumentCase {
            case: "the member's name is already a named schema's",
            schema: "inline.shadowed",
            schema_type: "OpenApi3",
            document: r##"{"openapi":"3.0.0","components":{"schemas":{
              "AWSEvent":{"type":"object","properties":{
                "detail":{"type":"object","properties":{"id":{"type":"string"}}}}},
              "Detail":{"type":"string"}}}}"##,
            types: &["AWSEvent", "AWSEventDetail"],
            implied: &["AWSEventDetail"],
        },
        DocumentCase {
            case: "one member name, two structures",
            schema: "inline.twice",
            schema_type: "OpenApi3",
            document: r##"{"openapi":"3.0.0","components":{"schemas":{
              "Placed":{"type":"object","properties":{
                "payload":{"type":"object","properties":{"id":{"type":"string"}}}}},
              "Shipped":{"type":"object","properties":{
                "payload":{"type":"object","properties":{
                  "at":{"type":"string","format":"date-time"}}}}}}}}"##,
            types: &["Placed", "Shipped", "Payload", "ShippedPayload"],
            implied: &["Payload", "ShippedPayload"],
        },
        DocumentCase {
            case: "inline in a draft-04 document",
            schema: "inline-draft4",
            schema_type: "JSONSchemaDraft4",
            document: r##"{"$schema":"http://json-schema.org/draft-04/schema#",
              "type":"object","properties":{
                "carrier":{"$ref":"#/definitions/Carrier"},
                "shipment":{"type":"object","properties":{"postalCode":{"type":"string"}}}},
              "definitions":{
                "Carrier":{"type":"object","properties":{"name":{"type":"string"}}}}}"##,
            types: &["InlineDraft4", "Carrier", "Shipment"],
            implied: &["Shipment"],
        },
        DocumentCase {
            case: "an object with no member of its own is no type",
            schema: "inline.freeform",
            schema_type: "OpenApi3",
            document: r##"{"openapi":"3.0.0","components":{"schemas":{
              "Envelope":{"type":"object","properties":{
                "metadata":{"type":"object"},
                "requestParameters":{"type":"object","nullable":true},
                "detail":{"type":"object","properties":{"id":{"type":"string"}}}}}}}}"##,
            types: &["Envelope", "Detail"],
            implied: &["Detail"],
        },
    ]
}

/// `GetCodeBindingSource`'s URI.
fn source_uri(schema: &str, language: &str) -> String {
    format!(
        "{}/language/{language}/source",
        schema_uri(REGISTRY, schema)
    )
}

/// `PutCodeBinding`'s URI, which is what brings a binding into existence.
fn binding_uri(schema: &str, language: &str) -> String {
    format!("{}/language/{language}", schema_uri(REGISTRY, schema))
}

/// The directory a package of `language` puts its sources under, and the extension its
/// files carry — the layout the generator states (`src/ops/get_code_binding_source.rs`,
/// `Target::source_root` / `Target::extension`).
fn layout(language: &str) -> (&'static str, &'static str) {
    match language {
        "Java8" => ("src/main/java/", "java"),
        "TypeScript3" => ("src/", "ts"),
        "Python36" => ("src/", "py"),
        "Go1" => ("", "go"),
        other => panic!("no layout known for language {other:?}"),
    }
}

/// How `language` declares the type `type_name` — the one line a generated file for it
/// must carry (`src/ops/get_code_binding_source.rs`, the four `*_source` renderers).
fn declaration_of(language: &str, type_name: &str) -> String {
    match language {
        "Java8" => format!("public class {type_name} {{"),
        "TypeScript3" => format!("export interface {type_name} {{"),
        "Python36" => format!("class {type_name}:"),
        "Go1" => format!("type {type_name} struct {{"),
        other => panic!("no declaration known for language {other:?}"),
    }
}

/// Whether `language` names a generated file after the type it declares, which is what
/// lets a case assert the archive's paths without restating the generator's casing.
fn file_is_named_after_the_type(language: &str) -> bool {
    matches!(language, "Java8" | "TypeScript3")
}

/// The package a schema's generated files live in, below the language's source root:
/// the one root every binding shares and the schema name's own alphanumeric runs,
/// lower-cased (`src/ops/get_code_binding_source.rs`, `package_segments`).
fn package_path(schema: &str) -> String {
    let mut segments = vec!["schema".to_string()];
    segments.extend(
        schema
            .split(|character: char| !character.is_alphanumeric())
            .filter(|part| !part.is_empty())
            .map(str::to_lowercase),
    );
    segments.join("/")
}

/// An emulator with the registry and every schema of [`document_cases`] created.
async fn emulator_with_schemas() -> Emulator {
    let emulator = Emulator::new();
    create_registry(&emulator, REGISTRY).await;
    for case in document_cases() {
        let created = emulator
            .call(
                "POST",
                &schema_uri(REGISTRY, case.schema),
                Some(json!({ "Content": case.document, "Type": case.schema_type })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: expected 201 from CreateSchema, got {} {:?}",
            case.case, created.status, created.body
        );
    }
    emulator
}

/// Start a binding and read its source once generation has produced it, the way a
/// caller does: ask again until the source is served or a generous deadline passes.
async fn generated_source(emulator: &Emulator, schema: &str, language: &str, case: &str) -> Answer {
    let put = emulator
        .call("POST", &binding_uri(schema, language), None)
        .await;
    assert_eq!(
        put.status, 202,
        "case {case:?}: expected 202 from PutCodeBinding, got {} {:?}",
        put.status, put.body
    );
    let read = source_uri(schema, language);
    let deadline = Instant::now() + GENERATION_DEADLINE;
    loop {
        let answer = emulator.call_payload("GET", &read).await;
        if answer.status == 200 {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the generated source must be served once generation \
             completes, last answer {} {:?}",
            answer.status,
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The archive one answer carries, as `(path, text)` per entry in the archive's own
/// order.
fn archive_entries(case: &str, answer: &Answer) -> Vec<(String, String)> {
    let mut archive =
        ::zip::ZipArchive::new(Cursor::new(answer.raw.to_vec())).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the body must be a ZIP archive ({error}), got {} bytes",
                answer.raw.len()
            )
        });
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .unwrap_or_else(|error| panic!("case {case:?}: entry {index} must read ({error})"));
        let path = entry
            .name()
            .unwrap_or_else(|error| panic!("case {case:?}: entry {index} must be named ({error})"))
            .to_string();
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .unwrap_or_else(|error| panic!("case {case:?}: {path} must be UTF-8 text ({error})"));
        entries.push((path, text));
    }
    entries
}

/// RULE: the archive holds one source file per type the document implies — its named
/// schemas and the structures its members declare inline — in every language the
/// service generates for.
#[tokio::test]
async fn the_archive_declares_every_type_the_document_implies() {
    let emulator = emulator_with_schemas().await;

    for document in document_cases() {
        for language in CODE_BINDING_LANGUAGES {
            let label = format!("{}/{language}", document.case);
            let answer = generated_source(&emulator, document.schema, language, &label).await;
            let entries = archive_entries(&label, &answer);
            let paths: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();

            assert_eq!(
                paths.len(),
                document.types.len(),
                "case {label:?}: one file per implied type {:?}, got {paths:?}",
                document.types
            );

            let (root, extension) = layout(language);
            let package = package_path(document.schema);
            if file_is_named_after_the_type(language) {
                let expected: Vec<String> = document
                    .types
                    .iter()
                    .map(|type_name| format!("{root}{package}/{type_name}.{extension}"))
                    .collect();
                assert_eq!(
                    paths, expected,
                    "case {label:?}: expected the package's files, got {paths:?}"
                );
            } else {
                for path in &paths {
                    assert!(
                        path.starts_with(&format!("{root}{package}/"))
                            && path.ends_with(&format!(".{extension}")),
                        "case {label:?}: wrong layout for {path:?}"
                    );
                }
            }

            let text: String = entries
                .iter()
                .map(|(_, body)| body.as_str())
                .collect::<Vec<&str>>()
                .join("\n");
            for type_name in document.types {
                let declaration = declaration_of(language, type_name);
                assert!(
                    text.contains(&declaration),
                    "case {label:?}: the archive must declare {declaration:?}, got {text}"
                );
            }
        }
    }
}

/// RULE: a type the document declares inline is a type of the generated package like
/// any other — declared in its own file AND named by the member that declares it, so
/// the declaration a client compiles against is reachable rather than orphaned.
#[tokio::test]
async fn an_implied_type_is_declared_and_named_by_its_member() {
    let emulator = emulator_with_schemas().await;

    for document in document_cases() {
        for language in CODE_BINDING_LANGUAGES {
            let label = format!("{}/{language}", document.case);
            let answer = generated_source(&emulator, document.schema, language, &label).await;
            let entries = archive_entries(&label, &answer);
            let text: String = entries
                .iter()
                .map(|(_, body)| body.as_str())
                .collect::<Vec<&str>>()
                .join("\n");

            for type_name in document.implied {
                let mentions = text.matches(type_name).count();
                assert!(
                    mentions >= 2,
                    "case {label:?}: {type_name:?} must be declared and named by the \
                     member that declares it, mentioned {mentions} time(s) in {text}"
                );
            }
        }
    }
}

/// RULE: the free-form members stay free-form — an object that declares no member of
/// its own carries contents the generator cannot name, so it gets the language's
/// unconstrained type and no file of its own.
#[tokio::test]
async fn an_object_with_no_members_gets_no_file() {
    let emulator = emulator_with_schemas().await;

    // The one document that declares both kinds of object side by side, in every
    // language, with that language's unconstrained type.
    for (language, unconstrained) in [
        ("Java8", "Object metadata"),
        ("TypeScript3", "metadata?: any;"),
        ("Python36", "metadata: Optional[Any]"),
        ("Go1", "Metadata interface{}"),
    ] {
        let label = format!("free-form/{language}");
        let answer = generated_source(&emulator, "inline.freeform", language, &label).await;
        let entries = archive_entries(&label, &answer);
        let paths: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths.len(),
            2,
            "case {label:?}: only the envelope and its inline detail are types, got {paths:?}"
        );
        let text: String = entries
            .iter()
            .map(|(_, body)| body.as_str())
            .collect::<Vec<&str>>()
            .join("\n");
        assert!(
            text.contains(unconstrained),
            "case {label:?}: a member with no declared members must take the \
             unconstrained type {unconstrained:?}, got {text}"
        );
    }
}
