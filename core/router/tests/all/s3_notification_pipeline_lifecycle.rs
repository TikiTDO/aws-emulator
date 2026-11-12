// SPDX-License-Identifier: CC0-1.0

//! The WHOLE LIFE of an event-driven object pipeline, end to end over the assembled router:
//! the role, the function and the bucket notification are put in place, an upload the
//! notification's filter admits is converted with no further API call, an upload it rejects
//! leaves nothing, and then the pipeline is TORN DOWN — the bucket emptied through its
//! paginated delete, the notification cleared, and the function, the role and the bucket
//! removed — leaving nothing of it behind.
//!
//! The rule under test: **a bucket whose notification configuration names a Lambda function
//! for an event class invokes that function for every object event whose key every rule of
//! the configuration's key-name `Filter` admits and for no other, and every resource of such
//! a pipeline can then be removed in the order a client removes them, each read-back
//! reporting the removal.** The sibling suite
//! `s3_notification_invokes_its_lambda_target` pins the first half of that for a handler that
//! leaves a marker; what only this suite reaches is the DERIVED OBJECT a converting handler
//! writes (read back and opened as the package it claims to be) and the teardown that
//! follows, which no crate-local suite can see because it spans S3, Lambda, IAM and
//! CloudFormation at once.
//!
//! The rows differ along what the rule generalises over: whether the uploaded key satisfies
//! the configuration's prefix rule, its suffix rule, both, or neither — and each row asserts
//! the only outcome its key admits, so a conversion that never happens and one the filter
//! excluded are told apart.
//!
//! Every call is SIGNED and the router VERIFIES it, which is the configuration the emulator
//! serves traffic in: the invocation the configuration makes, the handler's own SDK calls and
//! the teardown all travel through a router that authenticates, so a delivery only an
//! unauthenticated router would accept fails here.

use std::time::Duration;

use aws_credential_types::Credentials;
use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
use aws_sigv4::sign::v4::SigningParams;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element, outputs_of};

/// The Region every request here is signed for and addressed in.
const REGION: &str = "us-west-2";

/// The XML namespace a client's S3 payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The `StackStatus` a deployed stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The service principal Amazon S3 publishes notifications under, and the action it performs
/// on a function destination.
const S3_PRINCIPAL: &str = "s3.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The prefix and the suffix the bucket's notification configuration filters uploads by, and
/// the prefix the handler writes its own output under — which the filter does not name, so a
/// conversion never triggers another one.
const SOURCE_PREFIX: &str = "raw/";
const SOURCE_SUFFIX: &str = ".csv";
const OUTPUT_PREFIX: &str = "processed/";

/// How long this suite waits for work the clock owes — a stack's resources coming up, and the
/// asynchronous delivery of an object event — and how often it looks while waiting.
const WAIT: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// Keep this at or above the size `boot_serve.rs` uses: the same assembly runs here. The worker
/// stack is the one the emulator's own boot path gives its workers, because request handling —
/// and the delivery task below — runs on a worker.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;
const WORKER_STACK_SIZE: usize = 16 * 1024 * 1024;

/// How long a row that must see NOTHING keeps looking before concluding nothing came. The
/// converting rows run first, so the pipeline is known live and known to have drained an
/// event before an absence is read as an absence.
const QUIET_WINDOW: Duration = Duration::from_secs(8);

/// The CSV every row uploads, and the cells it holds.
fn source_rows() -> Vec<Vec<&'static str>> {
    vec![
        vec!["order", "total"],
        vec!["A-1", "42"],
        vec!["A-2", "7"],
    ]
}

/// The handler the pipeline runs: for each record it is handed it reads the CSV the event
/// named with the AWS SDK the runtime bundles, turns its rows into an Office Open XML
/// spreadsheet package with the standard library's own archive writer, and writes that
/// package back beside the source with the SDK.
///
/// The archive is written STORED (`zipfile.ZIP_STORED`) rather than deflated, so this suite
/// can read the worksheet part out of the stored package itself: a reader of the package needs
/// no other dependency to decompress it, which is what lets the assertion below be about the
/// cells the handler really wrote.
const CONVERTING_HANDLER: &str = r#"
import csv
import io
import os
import zipfile

import boto3

CLIENT = boto3.client("s3")

OUTPUT_PREFIX = "processed/"

SPREADSHEET = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"

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
    return value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _column(index):
    name = ""
    index += 1
    while index:
        index, remainder = divmod(index - 1, 26)
        name = chr(ord("A") + remainder) + name
    return name


def _sheet(rows):
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
    return OUTPUT_PREFIX + os.path.basename(key).rsplit(".", 1)[0] + ".xlsx"


def handler(event, context):
    written = []
    for record in event["Records"]:
        bucket = record["s3"]["bucket"]["name"]
        key = record["s3"]["object"]["key"]
        source = CLIENT.get_object(Bucket=bucket, Key=key)["Body"].read().decode("utf-8")
        rows = [row for row in csv.reader(io.StringIO(source)) if row]
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w", zipfile.ZIP_STORED) as archive:
            archive.writestr("[Content_Types].xml", CONTENT_TYPES)
            archive.writestr("_rels/.rels", ROOT_RELATIONSHIPS)
            archive.writestr("xl/workbook.xml", WORKBOOK)
            archive.writestr("xl/_rels/workbook.xml.rels", WORKBOOK_RELATIONSHIPS)
            archive.writestr("xl/worksheets/sheet1.xml", _sheet(rows))
        output = _output_key(key)
        CLIENT.put_object(
            Bucket=bucket,
            Key=output,
            Body=buffer.getvalue(),
            ContentType=SPREADSHEET,
        )
        written.append(output)
    return {"written": written}
"#;

/// The emulator as it is ASSEMBLED, on one engine, with request authentication ON — the
/// configuration the emulator serves traffic in: the handler's SDK calls and the delivery the
/// bucket's notification configuration makes are dispatched back into this same router, so
/// whether a router that VERIFIES signatures accepts them is part of what this suite asks.
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(true);
    router
}

/// The credential every call below signs with: the pair this process publishes once request
/// authentication is on.
fn published() -> &'static (String, String) {
    static PUBLISHED: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    PUBLISHED.get_or_init(|| {
        let minted = ::emulator_server::SigV4Authenticator::minted_credentials()
            .expect("turning request authentication on publishes this process's keys");
        (
            minted.access_key_id.clone(),
            minted.secret_access_key.clone(),
        )
    })
}

/// `request`, signed for `signing_name` in [`REGION`] the way an SDK signs one.
fn signed(mut request: http::Request<Vec<u8>>, signing_name: &str) -> http::Request<Bytes> {
    let (key_id, secret) = published();
    let identity = Credentials::new(key_id, secret, None, None, "test").into();
    let params = SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name(signing_name)
        .time(std::time::SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .expect("the signing parameters are complete")
        .into();
    let (method, uri) = (request.method().to_string(), request.uri().to_string());
    let headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().expect("a text header").to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        &method,
        &uri,
        headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(request.body()),
    )
    .expect("the request is signable");
    let (instructions, _) = sign(signable, &params)
        .expect("the request signs")
        .into_parts();
    instructions.apply_to_request_http1x(&mut request);
    let (parts, payload) = request.into_parts();
    http::Request::from_parts(parts, Bytes::from(payload))
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.into_bytes())
        .expect("a well-formed request");
    let response = router.dispatch(signed(request, service)).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One response of a REST surface: the status and the body.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    /// The body as text, for a refusal or an XML document.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Dispatch one request against S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/octet-stream")
        .body(body.to_vec())
        .expect("a well-formed request");
    let response = router.dispatch(signed(request, "s3")).await;
    Answer {
        status: response.status().as_u16(),
        body: response.body().to_vec(),
    }
}

/// Dispatch one Lambda `restJson1` request.
async fn lambda(router: &MultiServiceRouter, method: &str, path: &str, body: Option<&Value>) -> Answer {
    let host = format!("lambda.{REGION}.amazonaws.com");
    let payload = body.map(|body| body.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        .body(payload.into_bytes())
        .expect("a well-formed request");
    let response = router.dispatch(signed(request, "lambda")).await;
    Answer {
        status: response.status().as_u16(),
        body: response.body().to_vec(),
    }
}

/// `PUT /<bucket>` — `CreateBucket`, in the Region every call here is signed for: a
/// region-specific endpoint takes the `LocationConstraint` for its own Region ("The
/// unspecified location constraint is incompatible for the region specific endpoint this
/// request was sent to" is what a request without one is answered).
async fn create_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"{S3_XMLNS}\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let answer = s3(
        router,
        "PUT",
        &format!("/{bucket}"),
        configuration.as_bytes(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
}

/// `PUT /<bucket>/<key>` — `PutObject`.
async fn put_object(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str, body: &[u8]) {
    let answer = s3(router, "PUT", &format!("/{bucket}/{key}"), body).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: PutObject({bucket:?}, {key:?}) must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
}

/// The keys `bucket` holds under `prefix`, read through `ListObjectsV2`.
async fn listed(router: &MultiServiceRouter, case: &str, bucket: &str, prefix: &str) -> Vec<String> {
    let answer = s3(
        router,
        "GET",
        &format!("/{bucket}?list-type=2&prefix={prefix}"),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListObjectsV2({bucket:?}) must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
    elements(&answer.text(), "Key")
}

/// Wait until the object at `key` is held, or — for a key that must see nothing — until the
/// quiet window is up. Answers whether it is held.
async fn awaited_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    expected: bool,
) -> bool {
    let wait = if expected { WAIT } else { QUIET_WINDOW };
    let looks = wait.as_millis() / LOOK_INTERVAL.as_millis();
    for _ in 0..looks {
        let head = s3(router, "HEAD", &format!("/{bucket}/{key}"), &[]).await;
        if (200..300).contains(&head.status) {
            return true;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    false
}

/// `PUT /<bucket>?notification` with `body`, which must be accepted.
async fn put_notification(router: &MultiServiceRouter, case: &str, bucket: &str, body: &str) {
    let answer = s3(
        router,
        "PUT",
        &format!("/{bucket}?notification"),
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: PutBucketNotificationConfiguration({bucket:?}) must succeed, got {} \
         with {}",
        answer.status,
        answer.text()
    );
}

/// The configuration the pipeline is armed with: one Lambda destination for every
/// `ObjectCreated` event, filtered to [`SOURCE_PREFIX`] and [`SOURCE_SUFFIX`].
///
/// The elements are the ones S3's own model binds the Lambda list to
/// (`smithy_query(query="shape",shape="NotificationConfiguration")`:
/// `LambdaFunctionConfigurations` is bound as `CloudFunctionConfiguration`, whose ARN member is
/// bound as `CloudFunction`).
fn armed_configuration(function_arn: &str) -> String {
    format!(
        "<NotificationConfiguration xmlns=\"{S3_XMLNS}\"><CloudFunctionConfiguration>\
         <Id>convert</Id><CloudFunction>{function_arn}</CloudFunction>\
         <Event>s3:ObjectCreated:*</Event><Filter><S3Key>\
         <FilterRule><Name>prefix</Name><Value>{SOURCE_PREFIX}</Value></FilterRule>\
         <FilterRule><Name>suffix</Name><Value>{SOURCE_SUFFIX}</Value></FilterRule>\
         </S3Key></Filter></CloudFunctionConfiguration></NotificationConfiguration>"
    )
}

/// The trust policy a Lambda execution role needs, and the grant the handler's own calls need.
fn role_resource(bucket: &str) -> Value {
    json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "AssumeRolePolicyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "lambda.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            },
            "Policies": [{
                "PolicyName": "convert-the-upload",
                "PolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": ["s3:GetObject", "s3:PutObject", "logs:*"],
                        "Resource": [
                            format!("arn:aws:s3:::{bucket}/*"),
                            "*",
                        ],
                    }],
                },
            }],
        },
    })
}

/// The stack declaring the role, the converting function and the grant S3 needs.
fn pipeline_stack(bucket: &str) -> Value {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ConverterRole": role_resource(bucket),
            "Converter": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["ConverterRole", "Arn"] },
                    "Timeout": 30,
                    "Code": { "ZipFile": CONVERTING_HANDLER },
                },
            },
            "InvokePermission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": { "Fn::GetAtt": ["Converter", "Arn"] },
                    "Action": INVOKE_ACTION,
                    "Principal": S3_PRINCIPAL,
                    "SourceArn": format!("arn:aws:s3:::{bucket}"),
                },
            },
        },
        "Outputs": {
            "FunctionArn": { "Value": { "Fn::GetAtt": ["Converter", "Arn"] } },
            "FunctionName": { "Value": { "Ref": "Converter" } },
            "RoleName": { "Value": { "Ref": "ConverterRole" } },
        },
    })
}

/// The `StackStatus` `stack` reports now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "DescribeStacks",
        &[("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStacks({stack:?}) must succeed, got {status} with {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Create `stack` from `template`, wait for `CREATE_COMPLETE` and answer its outputs.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> std::collections::BTreeMap<String, String> {
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack({stack:?}) must be accepted, got {status} with {body}"
    );
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut reported = stack_status(router, stack).await;
    for _ in 0..looks {
        if reported.0 == CREATE_COMPLETE {
            break;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
        reported = stack_status(router, stack).await;
    }
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "case {case:?}: the stack must reach {CREATE_COMPLETE}, got {:?} with {}",
        reported.0, reported.1
    );
    outputs_of(&reported.1)
}

/// The CSV text the rows hold.
fn csv_text(rows: &[Vec<&str>]) -> Vec<u8> {
    let mut text = String::new();
    for row in rows {
        text.push_str(&row.join(","));
        text.push('\n');
    }
    text.into_bytes()
}

/// One row: the key uploaded, where its conversion would land, and whether the configuration's
/// filter admits it.
struct Row {
    case: &'static str,
    key: &'static str,
    output: &'static str,
    converted: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a key under the filtered prefix carrying the filtered suffix",
        key: "raw/orders.csv",
        output: "processed/orders.xlsx",
        converted: true,
    },
    Row {
        case: "a nested key under the filtered prefix",
        key: "raw/2026/01/ledger.csv",
        output: "processed/ledger.xlsx",
        converted: true,
    },
    Row {
        case: "a key under the prefix carrying another suffix",
        key: "raw/notes.txt",
        output: "processed/notes.xlsx",
        converted: false,
    },
    Row {
        case: "a key outside the filtered prefix",
        key: "staged/report.csv",
        output: "processed/report.xlsx",
        converted: false,
    },
];

/// Assert that the object at `key` is the package the handler built: an archive holding every
/// part of an Office Open XML spreadsheet, whose worksheet carries the source's own cells.
async fn assert_package_holds(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    key: &str,
    rows: &[Vec<&str>],
) {
    let answer = s3(router, "GET", &format!("/{bucket}/{key}"), &[]).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetObject({bucket:?}, {key:?}) must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
    let stored = answer.body.clone();
    assert_eq!(
        stored.get(..4),
        Some(b"PK\x03\x04".as_slice()),
        "case {case:?}: expected {key:?} to be an archive, got {} byte(s) beginning {:?}",
        stored.len(),
        stored.get(..4)
    );
    let text = String::from_utf8_lossy(&stored).into_owned();
    for part in [
        "[Content_Types].xml",
        "_rels/.rels",
        "xl/workbook.xml",
        "xl/_rels/workbook.xml.rels",
        "xl/worksheets/sheet1.xml",
    ] {
        assert!(
            text.contains(part),
            "case {case:?}: expected the package at {key:?} to hold the part {part:?}, got {} \
             byte(s)",
            stored.len()
        );
    }
    // The worksheet part is read out of the STORED archive itself (the handler writes
    // `ZIP_STORED`, so a reader needs nothing to decompress it), and each cell is asserted at
    // the reference the format addresses it by — so a value that appears anywhere else in the
    // package does not satisfy this.
    let sheet = text
        .split("<sheetData>")
        .nth(1)
        .and_then(|rest| rest.split("</sheetData>").next())
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the package at {key:?} to hold a worksheet, got {} \
                 byte(s)",
                stored.len()
            )
        })
        .to_string();
    for (index, row) in rows.iter().enumerate() {
        for (column, cell) in row.iter().enumerate() {
            let reference = format!("{}{}", column_name(column), index + 1);
            let written = format!("<c r=\"{reference}\" t=\"inlineStr\"><is><t>{cell}</t></is></c>");
            assert!(
                sheet.contains(&written),
                "case {case:?}: expected the worksheet in {key:?} to hold the source cell \
                 {cell:?} at {reference:?}, got {sheet}"
            );
        }
    }
}

/// The letters a spreadsheet addresses the column at `index` by, which is how the cell
/// references above are spelled.
fn column_name(index: usize) -> String {
    let mut name = String::new();
    let mut left = index + 1;
    while left > 0 {
        let remainder = (left - 1) % 26;
        name.insert(0, char::from(b'A' + u8::try_from(remainder).unwrap_or(0)));
        left = (left - 1) / 26;
    }
    name
}

/// `POST /<bucket>?delete` — `DeleteObjects`, the multi-object delete a client empties a
/// bucket with, naming each entry's version the way a client that listed versions names it.
async fn delete_versions(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    versions: &[(String, String)],
) {
    let objects: String = versions
        .iter()
        .map(|(key, version)| {
            format!("<Object><Key>{key}</Key><VersionId>{version}</VersionId></Object>")
        })
        .collect();
    let body = format!("<Delete xmlns=\"{S3_XMLNS}\">{objects}</Delete>");
    let answer = s3(
        router,
        "POST",
        &format!("/{bucket}?delete"),
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: DeleteObjects({bucket:?}, {versions:?}) must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
    let reported = elements(&answer.text(), "Key");
    assert_eq!(
        reported.len(),
        versions.len(),
        "case {case:?}: expected every one of {versions:?} to be reported deleted, got {}",
        answer.text()
    );
}

/// How many entries one page of the teardown's listing asks for. Small on purpose: the empty
/// a client performs before deleting a bucket is PAGINATED, so the teardown has to follow the
/// listing's own continuation rather than assume one page holds everything. The bucket is never
/// versioning-enabled here, so each entry the version listing reports is the one version of its
/// key, named by the null version id.
const VERSIONS_PER_PAGE: usize = 2;

/// One page of `ListObjectVersions`: the `(key, version id)` of every version it reported, and
/// the markers a next page would continue from.
struct VersionPage {
    versions: Vec<(String, String)>,
    next: Option<(String, String)>,
}

/// `GET /<bucket>?versions` — one page of the version listing, continued from `marker`.
async fn version_page(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    marker: Option<&(String, String)>,
) -> VersionPage {
    let mut path = format!("/{bucket}?versions&max-keys={VERSIONS_PER_PAGE}");
    if let Some((key, version)) = marker {
        path.push_str(&format!("&key-marker={key}&version-id-marker={version}"));
    }
    let answer = s3(router, "GET", &path, &[]).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListObjectVersions({bucket:?}) must succeed, got {} with {}",
        answer.status,
        answer.text()
    );
    let body = answer.text();
    // Each `<Version>` block carries the key and the version id the delete below names it by;
    // reading them per block keeps a key paired with its own id.
    let versions: Vec<(String, String)> = body
        .split("<Version>")
        .skip(1)
        .filter_map(|block| block.split("</Version>").next())
        .map(|block| (element(block, "Key"), element(block, "VersionId")))
        .collect();
    let truncated = elements(&body, "IsTruncated")
        .first()
        .is_some_and(|value| value == "true");
    let next = match (
        elements(&body, "NextKeyMarker").first().cloned(),
        elements(&body, "NextVersionIdMarker").first().cloned(),
    ) {
        (Some(key), Some(version)) if truncated => Some((key, version)),
        _ => None,
    };
    VersionPage { versions, next }
}

/// Empty `bucket` the way a client about to delete one empties it: page through its version
/// listing and remove every version each page reports, until a page reports none. Answers
/// whether the bucket really is empty afterwards.
async fn emptied_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) -> bool {
    // One pass per page, bounded by the pages the uploads above can possibly produce, so a
    // listing that keeps reporting what the delete removed ends the loop instead of spinning.
    for _ in 0..(ROWS.len() * 4) {
        let mut marker = None;
        let mut removed = 0;
        loop {
            let page = version_page(router, case, bucket, marker.as_ref()).await;
            if page.versions.is_empty() {
                break;
            }
            removed += page.versions.len();
            delete_versions(router, case, bucket, &page.versions).await;
            marker = page.next;
            if marker.is_none() {
                break;
            }
        }
        if removed == 0 {
            return listed(router, case, bucket, "").await.is_empty();
        }
    }
    false
}

/// Whatever `bucket` comes to hold while the quiet window runs: the keys of the first look that
/// finds any, or nothing when it stays empty throughout.
async fn held_throughout_quiet_window(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
) -> Vec<String> {
    let looks = QUIET_WINDOW.as_millis() / LOOK_INTERVAL.as_millis();
    for _ in 0..looks {
        let keys = listed(router, case, bucket, "").await;
        if !keys.is_empty() {
            return keys;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    Vec::new()
}

/// RULE: the pipeline converts exactly the uploads its filter admits, and every resource of it
/// can then be removed, each read-back reporting the removal.
///
/// Driven on a MULTI-THREAD runtime with the worker stack the emulator's own boot path gives
/// its workers (`core/router/src/main.rs`), because that is the runtime the emulator serves on:
/// the delivery a bucket's notification configuration makes is a task of its own, so which
/// thread it runs on is part of the configuration under test. Assembling the router costs
/// stack, so the suite is driven on an explicitly sized thread, as `boot_serve.rs` is.
#[test]
fn an_event_driven_object_pipeline_converts_its_uploads_and_tears_down() {
    std::thread::Builder::new()
        .name("s3-notification-pipeline-lifecycle".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(WORKER_STACK_SIZE)
                .build()
                .expect("build a multi-thread runtime");
            runtime.block_on(drive());
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread runs to completion");
}

/// The whole lifecycle, from the wiring to the teardown.
async fn drive() {
    let case = "the pipeline's wiring";
    let router = build_router();
    let bucket = "pipeline-lifecycle-source";
    create_bucket(&router, case, bucket).await;

    let outputs = deployed(
        &router,
        case,
        "pipeline-lifecycle-stack",
        &pipeline_stack(bucket).to_string(),
    )
    .await;
    let function_arn = outputs
        .get("FunctionArn")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output the function's ARN"));
    let function = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output the function's name"));
    let role = outputs
        .get("RoleName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output the role's name"));

    put_notification(&router, case, bucket, &armed_configuration(&function_arn)).await;

    let rows = source_rows();
    let source = csv_text(&rows);
    for row in ROWS {
        let case = row.case;
        put_object(&router, case, bucket, row.key, &source).await;
        let held = awaited_object(&router, bucket, row.output, row.converted).await;
        // Every row's conversion would land under the one prefix, so the listing under it is
        // also read: a row the filter rejects may leave nothing there at all.
        let under_output = listed(&router, case, bucket, OUTPUT_PREFIX).await;
        if !row.converted {
            assert!(
                !held,
                "case {case:?}: the filter rejects {:?}, so nothing may be written at {:?}, \
                 and an object appeared there; {OUTPUT_PREFIX} holds {under_output:?}",
                row.key, row.output
            );
            assert!(
                !under_output.contains(&row.output.to_string()),
                "case {case:?}: the filter rejects {:?}, so {OUTPUT_PREFIX} may not list \
                 {:?}, got {under_output:?}",
                row.key,
                row.output
            );
            continue;
        }
        assert!(
            held,
            "case {case:?}: expected the notification to convert {:?} into {:?} with no \
             further call, and nothing appeared within {WAIT:?}; {OUTPUT_PREFIX} holds \
             {under_output:?}",
            row.key, row.output
        );
        assert_package_holds(&router, case, bucket, row.output, &rows).await;
    }

    // ---- teardown: the bucket is emptied the way a client empties one it is about to
    // delete — every version each page of the listing reports, removed by the multi-object
    // delete — and then the notification is cleared and the function, the role and the
    // bucket are removed.
    let case = "the pipeline's teardown";
    let emptied = emptied_bucket(&router, case, bucket).await;
    assert!(
        emptied,
        "case {case:?}: expected the paginated delete to empty {bucket:?}, got {:?}",
        listed(&router, case, bucket, "").await
    );
    // STAYS empty, rather than being empty once: the configuration is still armed while the
    // bucket is emptied, so a key that comes back is a delete this pipeline answered with
    // another write.
    let returned = held_throughout_quiet_window(&router, case, bucket).await;
    assert!(
        returned.is_empty(),
        "case {case:?}: expected the emptied bucket to stay empty, and {returned:?} appeared \
         in it again within {QUIET_WINDOW:?}"
    );
    for row in ROWS {
        // Only the keys this pipeline really wrote: an output its filter rejected never
        // existed, so its absence here would assert nothing.
        let written: &[&str] = if row.converted {
            &[row.key, row.output]
        } else {
            &[row.key]
        };
        for key in written {
            let head = s3(&router, "HEAD", &format!("/{bucket}/{key}"), &[]).await;
            assert_eq!(
                head.status,
                404,
                "case {case:?}: expected {key:?} to be gone from the emptied bucket, got {} \
                 with {}",
                head.status,
                head.text()
            );
        }
    }
    let versions = version_page(&router, case, bucket, None).await;
    assert!(
        versions.versions.is_empty(),
        "case {case:?}: expected the emptied bucket to report no version at all, got {:?}",
        versions.versions
    );

    put_notification(
        &router,
        case,
        bucket,
        &format!("<NotificationConfiguration xmlns=\"{S3_XMLNS}\"></NotificationConfiguration>"),
    )
    .await;
    let read_back = s3(&router, "GET", &format!("/{bucket}?notification"), &[]).await;
    assert_eq!(
        read_back.status,
        200,
        "case {case:?}: GetBucketNotificationConfiguration({bucket:?}) must succeed, got {} \
         with {}",
        read_back.status,
        read_back.text()
    );
    assert!(
        !read_back.text().contains("CloudFunction"),
        "case {case:?}: expected the cleared configuration to name no destination, got {}",
        read_back.text()
    );

    let removed = lambda(
        &router,
        "DELETE",
        &format!("/2015-03-31/functions/{function}"),
        None,
    )
    .await;
    assert_eq!(
        removed.status,
        204,
        "case {case:?}: DeleteFunction({function:?}) must succeed, got {} with {}",
        removed.status,
        removed.text()
    );
    for path in [
        format!("/2015-03-31/functions/{function}/configuration"),
        format!("/2015-03-31/functions/{function}"),
        format!("/2015-03-31/functions/{function}/policy"),
    ] {
        let gone = lambda(&router, "GET", &path, None).await;
        assert_eq!(
            gone.status,
            404,
            "case {case:?}: expected {path:?} to report the deleted function absent, got {} \
             with {}",
            gone.status,
            gone.text()
        );
    }
    let listing = lambda(&router, "GET", "/2015-03-31/functions", None).await;
    assert_eq!(
        listing.status,
        200,
        "case {case:?}: ListFunctions must succeed, got {} with {}",
        listing.status,
        listing.text()
    );
    assert!(
        !listing.text().contains(&function),
        "case {case:?}: expected ListFunctions not to name the deleted {function:?}, got {}",
        listing.text()
    );

    let (status, body) = query(
        &router,
        "iam",
        "2010-05-08",
        "ListRolePolicies",
        &[("RoleName", &role)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListRolePolicies({role:?}) must succeed, got {status} with {body}"
    );
    for policy in elements(&body, "member") {
        let (status, body) = query(
            &router,
            "iam",
            "2010-05-08",
            "DeleteRolePolicy",
            &[("RoleName", &role), ("PolicyName", &policy)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteRolePolicy({role:?}, {policy:?}) must succeed, got {status} \
             with {body}"
        );
    }
    let (status, body) = query(
        &router,
        "iam",
        "2010-05-08",
        "DeleteRole",
        &[("RoleName", &role)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteRole({role:?}) must succeed, got {status} with {body}"
    );
    let (status, body) = query(
        &router,
        "iam",
        "2010-05-08",
        "GetRole",
        &[("RoleName", &role)],
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: the deleted role must not resolve, got {status} with {body}"
    );
    assert_eq!(
        element(&body, "Code"),
        "NoSuchEntity",
        "case {case:?}: expected the deleted role to be reported absent, got {body}"
    );
    let (status, body) = query(&router, "iam", "2010-05-08", "ListRoles", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListRoles must succeed, got {status} with {body}"
    );
    assert!(
        !body.contains(&role),
        "case {case:?}: expected ListRoles not to name the deleted {role:?}, got {body}"
    );

    let dropped = s3(&router, "DELETE", &format!("/{bucket}"), &[]).await;
    assert_eq!(
        dropped.status,
        204,
        "case {case:?}: DeleteBucket({bucket:?}) must succeed, got {} with {}",
        dropped.status,
        dropped.text()
    );
    let absent = s3(&router, "HEAD", &format!("/{bucket}"), &[]).await;
    assert_eq!(
        absent.status,
        404,
        "case {case:?}: the deleted bucket must not resolve, got {} with {}",
        absent.status,
        absent.text()
    );
    let buckets = s3(&router, "GET", "/", &[]).await;
    assert_eq!(
        buckets.status,
        200,
        "case {case:?}: ListBuckets must succeed, got {} with {}",
        buckets.status,
        buckets.text()
    );
    assert!(
        !buckets.text().contains(bucket),
        "case {case:?}: expected ListBuckets not to name the deleted {bucket:?}, got {}",
        buckets.text()
    );
    // What is left of the pipeline on the deleted bucket: neither its objects nor the
    // configuration that drove them resolve any longer.
    for path in [
        format!("/{bucket}?notification"),
        format!("/{bucket}?list-type=2"),
    ] {
        let gone = s3(&router, "GET", &path, &[]).await;
        assert_eq!(
            gone.status,
            404,
            "case {case:?}: expected {path:?} to report the deleted bucket absent, got {} with \
             {}",
            gone.status,
            gone.text()
        );
        assert_eq!(
            element(&gone.text(), "Code"),
            "NoSuchBucket",
            "case {case:?}: expected {path:?} to name the bucket absent, got {}",
            gone.text()
        );
    }
}
