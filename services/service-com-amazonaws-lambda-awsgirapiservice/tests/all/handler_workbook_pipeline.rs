// SPDX-License-Identifier: CC0-1.0

//! What a `python3.x` handler can BUILD out of what the managed runtime presents, end to end
//! over the wire: Lambda, Amazon S3, IAM and the token service on ONE engine, a function whose
//! code does `import boto3` and turns a CSV object into a spreadsheet package with the
//! standard library's own archive writer, and that package read back where a client reads it.
//!
//! RULE: a managed `python3.x` runtime presents on the handler's import path both the AWS SDK
//! the runtime bundles and the COMPLETE standard library, so a handler module that imports
//! either one runs — the SDK's calls are made as the function's execution role and answered by
//! the service that owns the resource, and the standard library's file machinery (its CSV
//! reader, its in-memory byte streams, its ZIP writer and the text codecs each of those reaches
//! by name) does its work — and the invocation answers the handler's own value with no
//! `X-Amz-Function-Error`.
//!
//! Three tables, each over what the rule generalises over:
//!
//! * the CONVERSION table drives one synchronous invocation per source object, and its rows
//!   differ in how many rows and cells the source carries, whether a cell's text needs XML
//!   escaping, whether it is outside ASCII, whether the rows are ragged, and which compression
//!   method the archive is written with (stored, and deflated through the compression module);
//! * the ENCODING table is the same rule at the surface the archive reader reaches through: one
//!   row per text-codec family, each named by a handler that imports nothing of the codec
//!   machinery itself, plus a name no codec answers to;
//! * the EVENT table drives the same handler from an armed `s3:ObjectCreated:*` notification
//!   rather than from a client's call, one row per key the configuration's filter admits or
//!   rejects, so the work the runtime does is also asserted where nothing waits for it.
//!
//! Every row that produces a workbook reads it back through Amazon S3's own API and opens it as
//! the archive it claims to be, so a run that answered without writing one fails here.
//!
//! The suite RUNS handlers, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::io::Read as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::given_role;
use crate::event_source_mappings::{await_function_active, call, router_on};
use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The bucket each test reads its sources out of and writes its workbooks into. Every test
/// builds an emulator of its own, so the name is shared and the state is not.
const BUCKET: &str = "workbook-pipeline";

/// The prefix the handler writes a workbook under, which is the one the event table's
/// notification filter does NOT name — so a workbook never triggers another conversion.
const OUTPUT_PREFIX: &str = "processed/";

/// The `Timeout` every function here carries, in seconds. Sized the way the sibling suites
/// that RUN handlers size theirs: it has to cover this test binary's own scheduling, not the
/// handlers' work.
const FUNCTION_TIMEOUT_SECONDS: i32 = 25;

/// The content type the handler serves a spreadsheet package as, which is the one the Office
/// Open XML spreadsheet format publishes.
const SPREADSHEET_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// The part the cells land in, which a produced package is opened at.
const SHEET_PART: &str = "xl/worksheets/sheet1.xml";

/// Every part an Office Open XML spreadsheet package carries, sorted: the content-type part,
/// the package relationships, the workbook, its relationships and the one worksheet.
const PARTS: [&str; 5] = [
    "[Content_Types].xml",
    "_rels/.rels",
    "xl/_rels/workbook.xml.rels",
    "xl/workbook.xml",
    SHEET_PART,
];

/// The `compress_type` an archive reader reports for a STORED entry and for a DEFLATED one,
/// which are the ZIP format's own method numbers 0 and 8.
const STORED: u64 = 0;
const DEFLATED: u64 = 8;

/// How long a leg that waits for an object to APPEAR waits in all, and how often it looks.
const APPEARS_WITHIN: Duration = Duration::from_secs(20);

/// How long a leg that requires an object to STAY ABSENT watches for it. Shorter than the
/// window above on purpose: the positive leg runs first and proves the pipeline live and
/// drained, so an absence here is an absence and not lateness.
const STAYS_ABSENT_FOR: Duration = Duration::from_secs(5);

/// How long one look waits before the next.
const LOOK_AGAIN: Duration = Duration::from_millis(250);

/// The handler every table runs: it reads the CSV object it is pointed at with the SDK, turns
/// its rows into a spreadsheet package with the standard library's archive writer, writes that
/// package back with the SDK, and reports what the package it wrote holds. It is driven either
/// by a client's own event or by an Amazon S3 event notification, which is the same work from
/// two callers.
const PIPELINE_SOURCE: &str = r#"
import csv
import io
import os
import zipfile

import boto3

CLIENT = boto3.client("s3")

SPREADSHEET = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"

OUTPUT_PREFIX = "processed/"

CONTENT_TYPES = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
    '<Default Extension="rels" ContentType='
    '"application/vnd.openxmlformats-package.relationships+xml"/>'
    '<Default Extension="xml" ContentType="application/xml"/>'
    '<Override PartName="/xl/workbook.xml" ContentType='
    '"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
    '<Override PartName="/xl/worksheets/sheet1.xml" ContentType='
    '"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
    "</Types>"
)

ROOT_RELATIONSHIPS = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/'
    'relationships/officeDocument" Target="xl/workbook.xml"/>'
    "</Relationships>"
)

WORKBOOK = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
    '<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>'
)

WORKBOOK_RELATIONSHIPS = (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/'
    'relationships/worksheet" Target="worksheets/sheet1.xml"/>'
    "</Relationships>"
)


def _escaped(value):
    """`value` as XML character data: the delimiters the format reserves."""
    return value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _column(index):
    """The letters a spreadsheet addresses the column at `index` by."""
    name = ""
    index += 1
    while index:
        index, remainder = divmod(index - 1, 26)
        name = chr(ord("A") + remainder) + name
    return name


def _sheet(rows):
    """The worksheet part holding `rows`, one inline-string cell per value."""
    body = []
    for number, row in enumerate(rows, start=1):
        cells = "".join(
            '<c r="%s%d" t="inlineStr"><is><t>%s</t></is></c>'
            % (_column(column), number, _escaped(value))
            for column, value in enumerate(row)
        )
        body.append('<row r="%d">%s</row>' % (number, cells))
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        "<sheetData>%s</sheetData></worksheet>" % "".join(body)
    )


def _output_key(key):
    """Where the workbook for the source `key` goes: its base name, under the output prefix."""
    return OUTPUT_PREFIX + os.path.basename(key).rsplit(".", 1)[0] + ".xlsx"


def _convert(bucket, key, output, compressed):
    """Read the CSV at `key`, write the workbook at `output`, and report what it holds."""
    read = CLIENT.get_object(Bucket=bucket, Key=key)
    source = read["Body"].read().decode("utf-8")
    rows = [row for row in csv.reader(io.StringIO(source)) if row]

    method = zipfile.ZIP_DEFLATED if compressed else zipfile.ZIP_STORED
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", method) as archive:
        archive.writestr("[Content_Types].xml", CONTENT_TYPES)
        archive.writestr("_rels/.rels", ROOT_RELATIONSHIPS)
        archive.writestr("xl/workbook.xml", WORKBOOK)
        archive.writestr("xl/_rels/workbook.xml.rels", WORKBOOK_RELATIONSHIPS)
        archive.writestr("xl/worksheets/sheet1.xml", _sheet(rows))
    workbook = buffer.getvalue()

    CLIENT.put_object(
        Bucket=bucket,
        Key=output,
        Body=workbook,
        ContentType=SPREADSHEET,
    )

    # Read back what was built, through the reader a consumer of the package uses.
    with zipfile.ZipFile(io.BytesIO(workbook)) as written:
        parts = sorted(written.namelist())
        methods = sorted(set(entry.compress_type for entry in written.infolist()))
    return {
        "output": output,
        "rows": len(rows),
        "cells": [len(row) for row in rows],
        "parts": parts,
        "methods": methods,
        "bytes": len(workbook),
    }


def lambda_handler(event, context):
    if "Records" in event:
        converted = [
            _convert(
                record["s3"]["bucket"]["name"],
                record["s3"]["object"]["key"],
                _output_key(record["s3"]["object"]["key"]),
                True,
            )
            for record in event["Records"]
        ]
    else:
        converted = [
            _convert(event["bucket"], event["key"], event["output"], event["compressed"])
        ]
    return {"converted": converted}
"#;

/// Lambda, IAM, the token service and Amazon S3 over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's own SDK calls are dispatched back into
/// this same router, so the bucket it reads and writes is the bucket this suite reads, and the
/// role it runs as is the role IAM holds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One Amazon S3 request, path style, as the account's own credential: its protocol binds
/// everything to a method and a path, and the answer's BYTES are what a binary object is.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Vec<u8>) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    (parts.status.as_u16(), parts.headers, body.to_vec())
}

/// One Amazon S3 request that must be served, answering the headers and bytes it came with.
async fn s3_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: &[u8],
) -> (http::HeaderMap, Vec<u8>) {
    let (status, headers, answered) = s3(router, method, path, body).await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: expected S3 {method} {path} to be served, got {status} with {}",
        String::from_utf8_lossy(&answered)
    );
    (headers, answered)
}

/// The identity policy an execution role carries: the two object actions the handler makes, on
/// this suite's own bucket.
fn object_access() -> Value {
    json!([{
        "Effect": "Allow",
        "Action": ["s3:GetObject", "s3:PutObject"],
        "Resource": format!("arn:aws:s3:::{BUCKET}/*"),
    }])
}

/// Create the function `name`, running as `role_arn`, out of [`PIPELINE_SOURCE`].
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str, role_arn: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(
                    PIPELINE_SOURCE,
                )),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(router, case, name).await;
}

/// The letters a spreadsheet addresses the column at `index` by, which is what the cell
/// references in a produced worksheet are built from.
fn column(index: usize) -> String {
    let mut name = String::new();
    let mut left = index + 1;
    while left > 0 {
        let remainder = (left - 1) % 26;
        name.insert(0, char::from(b'A' + u8::try_from(remainder).unwrap_or(0)));
        left = (left - 1) / 26;
    }
    name
}

/// `value` as XML character data, the way the format reserves its three delimiters.
fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The CSV text an object holds for `cells`.
fn csv_text(cells: &[Vec<String>]) -> String {
    let mut text = String::new();
    for row in cells {
        text.push_str(&row.join(","));
        text.push('\n');
    }
    text
}

/// The workbook stored at `key`, opened as the archive it claims to be: the parts it holds and
/// its worksheet's text.
fn opened(case: &str, stored: Vec<u8>) -> (Vec<String>, String) {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(stored))
        .unwrap_or_else(|error| panic!("case {case:?}: expected an archive, got {error}"));
    let mut names: Vec<String> = archive
        .file_names()
        .map(|name| {
            name.unwrap_or_else(|error| {
                panic!("case {case:?}: expected the entry names, got {error}")
            })
            .to_string()
        })
        .collect();
    names.sort();
    let mut sheet = String::new();
    archive
        .by_name(SHEET_PART)
        .unwrap_or_else(|error| panic!("case {case:?}: expected the worksheet part, got {error}"))
        .read_to_string(&mut sheet)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected the worksheet to be text, got {error}")
        });
    (names, sheet)
}

/// Assert that the package stored at `key` holds every part and every cell of `cells`.
async fn assert_workbook_holds(
    router: &MultiServiceRouter,
    case: &str,
    key: &str,
    cells: &[Vec<String>],
) {
    let (headers, stored) = s3_ok(router, case, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some(SPREADSHEET_TYPE),
        "case {case:?}: expected the stored object to carry the content type the write named, \
         got {headers:?}"
    );
    let (names, sheet) = opened(case, stored);
    assert_eq!(
        names,
        PARTS
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>(),
        "case {case:?}: expected the stored package to hold every part, got {names:?}"
    );
    for (number, row) in cells.iter().enumerate() {
        for (position, value) in row.iter().enumerate() {
            let expected = format!(
                "<c r=\"{}{}\" t=\"inlineStr\"><is><t>{}</t></is></c>",
                column(position),
                number + 1,
                escaped(value)
            );
            assert!(
                sheet.contains(&expected),
                "case {case:?}: expected the worksheet to hold {expected:?}, got {sheet}"
            );
        }
    }
}

/// One row of the conversion table: the cells the source object carries and how the archive is
/// written.
struct Row {
    case: &'static str,
    /// The source object's cells, row by row. No value carries a comma, a quotation mark or a
    /// line break, so the CSV this is rendered to needs no quoting and the row is about the
    /// cells rather than about the CSV dialect.
    cells: Vec<Vec<String>>,
    /// Whether the handler writes the archive DEFLATED (through the compression module) rather
    /// than stored.
    compressed: bool,
}

/// The rows: how many rows and cells there are, what the text needs of the XML escaping rule,
/// whether it is outside ASCII, whether the rows are ragged, and which compression method the
/// archive carries.
fn rows() -> Vec<Row> {
    let text = |row: &[&str]| row.iter().map(|cell| (*cell).to_string()).collect();
    vec![
        Row {
            case: "a header row and one data row, deflated",
            cells: vec![text(&["id", "total"]), text(&["1", "42"])],
            compressed: true,
        },
        Row {
            case: "a header row alone, stored",
            cells: vec![text(&["id", "total", "currency", "booked"])],
            compressed: false,
        },
        Row {
            case: "cells carrying the characters XML reserves",
            cells: vec![
                text(&["name", "note"]),
                text(&["a&b", "x<y"]),
                text(&["p>q", "r&<>s"]),
            ],
            compressed: true,
        },
        Row {
            case: "cells outside ASCII, stored",
            cells: vec![
                text(&["ville", "pays"]),
                text(&["Montréal", "Canada"]),
                text(&["東京", "日本"]),
            ],
            compressed: false,
        },
        Row {
            case: "ragged rows, each with its own cell count",
            cells: vec![
                text(&["a", "b", "c"]),
                text(&["1"]),
                text(&["2", "3"]),
                text(&["4", "5", "6"]),
            ],
            compressed: true,
        },
        Row {
            case: "many rows, deflated",
            cells: (0..40)
                .map(|index| {
                    vec![
                        format!("row-{index}"),
                        format!("{}", index * 7),
                        format!("{}", index % 3),
                    ]
                })
                .collect(),
            compressed: true,
        },
    ]
}

/// Every row of the conversion table, on ONE emulator and ONE function: the handler reads the
/// CSV with the SDK the runtime bundles, builds the package with the standard library and
/// writes it back — and the package is read back through Amazon S3's own API and opened as an
/// archive here.
#[tokio::test]
async fn a_handler_turns_a_csv_object_into_a_workbook_with_the_sdk_and_the_standard_library() {
    let _stream = one_invocation_stream_at_a_time().await;
    let router = emulator();
    let role = given_role(
        &router,
        "setup",
        "workbook-pipeline-role",
        Some(object_access()),
    )
    .await;
    let function = "workbook-pipeline";
    given_function(&router, "setup", function, &role).await;
    s3_ok(&router, "setup", "PUT", &format!("/{BUCKET}"), &[]).await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let source_key = format!("raw/source-{index}.csv");
        let output_key = format!("{OUTPUT_PREFIX}source-{index}.xlsx");
        s3_ok(
            &router,
            case,
            "PUT",
            &format!("/{BUCKET}/{source_key}"),
            csv_text(&row.cells).as_bytes(),
        )
        .await;

        let answered = call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{function}/invocations"),
            &json!({
                "bucket": BUCKET,
                "key": source_key,
                "output": output_key,
                "compressed": row.compressed,
            }),
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("x-amz-function-error"),
            None,
            "case {case:?}: the handler must run to completion, got the function error {:?} with \
             {}",
            answered.header("x-amz-function-error"),
            answered.text()
        );
        let report = &answered.json()["converted"][0];

        // What the handler reported of the package it wrote.
        assert_eq!(
            report["rows"],
            json!(row.cells.len()),
            "case {case:?}: expected one worksheet row per source row, got {report}"
        );
        let counts: Vec<usize> = row.cells.iter().map(Vec::len).collect();
        assert_eq!(
            report["cells"],
            json!(counts),
            "case {case:?}: expected the source's own cell counts, got {report}"
        );
        assert_eq!(
            report["parts"],
            json!(PARTS),
            "case {case:?}: expected the package's own parts, got {report}"
        );
        let expected_method = if row.compressed { DEFLATED } else { STORED };
        assert_eq!(
            report["methods"],
            json!([expected_method]),
            "case {case:?}: expected every entry written with the method the row names, got \
             {report}"
        );
        let built = report["bytes"].as_u64().unwrap_or_default();
        assert!(
            built > 0,
            "case {case:?}: expected the handler to build a non-empty package, got {report}"
        );

        // What the BUCKET holds: the work the handler reported is the work that happened, read
        // back where a client reads it and opened as the archive it claims to be.
        assert_workbook_holds(&router, case, &output_key, &row.cells).await;
        let (_headers, stored) = s3_ok(
            &router,
            case,
            "GET",
            &format!("/{BUCKET}/{output_key}"),
            &[],
        )
        .await;
        assert_eq!(
            stored.len() as u64,
            built,
            "case {case:?}: expected the bucket to hold exactly the bytes the handler built, got \
             {} bytes",
            stored.len()
        );
    }
}

/// The handler of the encoding table: it encodes and decodes the text each case names with the
/// codec that case names, and reports what it got — or what was raised. The module imports
/// NOTHING: the codec is reached by name off `str` and `bytes`, which is how the standard
/// library's own modules reach one.
const ENCODING_SOURCE: &str = r#"
def lambda_handler(event, context):
    report = {}
    for case in event["cases"]:
        name = case["encoding"]
        try:
            encoded = case["text"].encode(name)
            report[name] = {
                "bytes": list(encoded),
                "round_trip": encoded.decode(name),
            }
        except BaseException as error:
            report[name] = {"raised": type(error).__name__, "message": str(error)}
    return report
"#;

/// One row of the encoding table: a codec, the text it carries and the bytes that text is in
/// that codec.
struct Encoding {
    /// The codec's name, as a program names it.
    ///
    /// Source: the "Standard Encodings" table of the Python standard library's `codecs`
    /// documentation, which is the catalog the `encodings` package implements. None of these
    /// names is one the issue's own report cited.
    name: &'static str,
    /// The text encoded, chosen so a row's bytes could come from no other codec here.
    text: &'static str,
    /// The bytes that text is, in that codec, or `None` for a name no codec answers to.
    bytes: Option<&'static [u8]>,
}

/// The rows: the variable-width codec, three single-byte families (the Latin, the Windows and
/// the Macintosh ones) and a non-Latin one, two fixed-width codecs of different widths and
/// different byte orders, the ASCII one — and a name no codec answers to, which is a lookup
/// failure rather than a silent substitution.
fn encodings() -> Vec<Encoding> {
    vec![
        Encoding {
            name: "utf-8",
            text: "é",
            bytes: Some(&[0xC3, 0xA9]),
        },
        Encoding {
            name: "ascii",
            text: "A",
            bytes: Some(&[0x41]),
        },
        Encoding {
            name: "latin-1",
            text: "é",
            bytes: Some(&[0xE9]),
        },
        // The codec the ZIP format names for an entry whose name is not flagged UTF-8, which
        // is why an archive reader needs it.
        Encoding {
            name: "cp437",
            text: "é",
            bytes: Some(&[0x82]),
        },
        Encoding {
            name: "cp1252",
            text: "€",
            bytes: Some(&[0x80]),
        },
        Encoding {
            name: "mac_roman",
            text: "é",
            bytes: Some(&[0x8E]),
        },
        Encoding {
            name: "iso8859-7",
            text: "α",
            bytes: Some(&[0xE1]),
        },
        Encoding {
            name: "utf-16-le",
            text: "A",
            bytes: Some(&[0x41, 0x00]),
        },
        Encoding {
            name: "utf-32-be",
            text: "A",
            bytes: Some(&[0x00, 0x00, 0x00, 0x41]),
        },
        Encoding {
            name: "nosuchcodec",
            text: "A",
            bytes: None,
        },
    ]
}

/// Every text codec the standard library ships is reachable BY NAME from a handler that
/// imports nothing of the codec machinery itself — which is how the standard library's own
/// modules reach one — and a name no codec answers to is a lookup failure.
#[tokio::test]
async fn every_encoding_the_standard_library_ships_is_reachable_by_name_from_a_handler() {
    let _stream = one_invocation_stream_at_a_time().await;
    let case = "the codecs a handler names";
    let router = emulator();
    let role = given_role(&router, case, "encoding-surface-role", None).await;
    let function = "encoding-surface";
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": function,
            "Role": role,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(
                    ENCODING_SOURCE,
                )),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(&router, case, function).await;

    let rows = encodings();
    let cases: Vec<Value> = rows
        .iter()
        .map(|row| json!({ "encoding": row.name, "text": row.text }))
        .collect();
    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        &json!({ "cases": cases }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.header("x-amz-function-error"),
        None,
        "case {case:?}: the handler must run to completion, got the function error {:?} with {}",
        answered.header("x-amz-function-error"),
        answered.text()
    );
    let report = answered.json();
    for row in &rows {
        let name = row.name;
        let answer = &report[name];
        match row.bytes {
            Some(expected) => {
                assert_eq!(
                    answer["bytes"],
                    json!(expected),
                    "case {name:?}: expected the bytes the codec produces, got {answer}"
                );
                assert_eq!(
                    answer["round_trip"],
                    json!(row.text),
                    "case {name:?}: expected the text back out of its own bytes, got {answer}"
                );
            }
            None => {
                assert_eq!(
                    answer["raised"],
                    json!("LookupError"),
                    "case {name:?}: expected a name no codec answers to to be a lookup failure, \
                     got {answer}"
                );
                let message = answer["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(name),
                    "case {name:?}: expected the failure to name the codec asked for, got {answer}"
                );
            }
        }
    }
}

/// One row of the event table: the key uploaded, and whether the notification's filter admits
/// it — which decides whether a workbook appears for it.
struct Upload {
    case: &'static str,
    /// The key the object is written under.
    key: &'static str,
    /// Where the handler would write that object's workbook.
    output: &'static str,
    /// Whether the configuration's filter admits the key.
    converted: bool,
}

/// The rows: the key the filter admits, a key whose suffix differs only in case (a
/// `FilterRule` value is matched literally, so it is not admitted), a key under the prefix with
/// another suffix, and a key outside the prefix.
const UPLOADS: [Upload; 4] = [
    Upload {
        case: "a key under the filtered prefix with the filtered suffix",
        key: "raw/orders.csv",
        output: "processed/orders.xlsx",
        converted: true,
    },
    Upload {
        case: "a key whose suffix differs from the filter's only in case",
        key: "raw/upper.CSV",
        output: "processed/upper.xlsx",
        converted: false,
    },
    Upload {
        case: "a key under the prefix carrying another suffix",
        key: "raw/notes.txt",
        output: "processed/notes.xlsx",
        converted: false,
    },
    Upload {
        case: "a key outside the filtered prefix",
        key: "staged/ledger.csv",
        output: "processed/ledger.xlsx",
        converted: false,
    },
];

/// Whether the object at `key` is held, looked for until `window` is up.
async fn held_within(router: &MultiServiceRouter, key: &str, window: Duration) -> bool {
    let until = std::time::Instant::now() + window;
    loop {
        let (status, _headers, _body) = s3(router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
        if (200..300).contains(&status) {
            return true;
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Whether the object at `key` stays absent for the whole of `window`.
async fn absent_throughout(router: &MultiServiceRouter, key: &str, window: Duration) -> bool {
    let until = std::time::Instant::now() + window;
    loop {
        let (status, _headers, _body) = s3(router, "GET", &format!("/{BUCKET}/{key}"), &[]).await;
        if (200..300).contains(&status) {
            return false;
        }
        if std::time::Instant::now() >= until {
            return true;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The same handler, driven by an armed `s3:ObjectCreated:*` notification instead of by a
/// client's call: an upload the filter admits is converted with no further API call, and one it
/// rejects produces nothing. The positive row runs FIRST, so the pipeline is known live and
/// known to have drained an event before an absence is read as an absence.
#[tokio::test]
async fn an_armed_notification_runs_the_same_handler_on_the_keys_its_filter_admits() {
    let _stream = one_invocation_stream_at_a_time().await;
    let case = "an armed object-created notification";
    let router = emulator();
    let role = given_role(&router, case, "workbook-event-role", Some(object_access())).await;
    let function = "workbook-event";
    given_function(&router, case, function, &role).await;

    // Amazon S3 invokes the function as the service principal, which the function's own
    // resource policy is what admits.
    let permitted = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/policy"),
        &json!({
            "StatementId": "s3-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
            "SourceArn": format!("arn:aws:s3:::{BUCKET}"),
        }),
    )
    .await;
    assert_eq!(
        permitted.status,
        201,
        "case {case:?}: expected AddPermission to answer 201, got {} {}",
        permitted.status,
        permitted.text()
    );

    s3_ok(&router, case, "PUT", &format!("/{BUCKET}"), &[]).await;
    // The configuration, in the elements the model's own `xmlName`s give it: the Lambda list is
    // `CloudFunctionConfiguration` and its ARN member is `CloudFunction`.
    let configuration = format!(
        "<NotificationConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <CloudFunctionConfiguration><Id>convert</Id>\
         <CloudFunction>arn:aws:lambda:{REGION}:{account}:function:{function}</CloudFunction>\
         <Event>s3:ObjectCreated:*</Event>\
         <Filter><S3Key>\
         <FilterRule><Name>prefix</Name><Value>raw/</Value></FilterRule>\
         <FilterRule><Name>suffix</Name><Value>.csv</Value></FilterRule>\
         </S3Key></Filter></CloudFunctionConfiguration></NotificationConfiguration>",
        account = engine::emulator_account_id()
    );
    s3_ok(
        &router,
        case,
        "PUT",
        &format!("/{BUCKET}?notification"),
        configuration.as_bytes(),
    )
    .await;

    let cells: Vec<Vec<String>> = vec![
        vec!["id".to_string(), "total".to_string()],
        vec!["1".to_string(), "42".to_string()],
    ];
    for upload in &UPLOADS {
        let case = upload.case;
        s3_ok(
            &router,
            case,
            "PUT",
            &format!("/{BUCKET}/{}", upload.key),
            csv_text(&cells).as_bytes(),
        )
        .await;
        if upload.converted {
            assert!(
                held_within(&router, upload.output, APPEARS_WITHIN).await,
                "case {case:?}: expected the notification to convert {} into {} with no further \
                 call, and nothing appeared within {:?}",
                upload.key,
                upload.output,
                APPEARS_WITHIN
            );
            // The workbook the run wrote holds the source's own cells.
            assert_workbook_holds(&router, case, upload.output, &cells).await;
            continue;
        }
        assert!(
            absent_throughout(&router, upload.output, STAYS_ABSENT_FOR).await,
            "case {case:?}: expected the filter to reject {} so that nothing is written at {}, \
             and an object appeared within {:?}",
            upload.key,
            upload.output,
            STAYS_ABSENT_FOR
        );
    }
}
