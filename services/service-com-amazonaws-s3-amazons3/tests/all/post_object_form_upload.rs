// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for S3's BROWSER-BASED upload: the HTML-form `POST` of an object to a
//! bucket, which "adds an object to a specified bucket by using HTML forms" (Amazon S3 API
//! Reference, *POST Object*).
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour a browser, a mobile client or a `curl -F` sees: the status, the document
//! the answer carries, the headers, and the object the upload really stored — read back
//! through `GetObject`, so a served answer cannot be a claim about a write nothing made.
//!
//! The rules under test:
//!
//! * the form's `file` part is stored at the key its `key` field names, with `${filename}`
//!   in that key standing for the name of the attached file, and the fields beside it
//!   configuring the object exactly as the request headers of the same name configure a
//!   `PutObject`;
//! * the answer is the one the form asked for — "if the value is set to 201, Amazon S3
//!   returns an XML document" and otherwise the empty document of the
//!   `success_action_status` it named, which is `204` when it named none or named one S3
//!   does not publish — and a valid `success_action_redirect` is answered `303` with the
//!   bucket, the key and the ETag added to that URL;
//! * the `policy` the form carries is ENFORCED: an expired policy, a condition the
//!   submitted form does not satisfy and a file outside the policy's
//!   `content-length-range` each refuse the upload with S3's own published code, and
//!   nothing is stored;
//! * the event the upload publishes is an `s3:ObjectCreated:Post` whose EventBridge
//!   `reason` is "POST Object", so a destination or a rule scoped to the `PUT` of an object
//!   does not receive it.
//!
//! The rows differ along what each rule generalises over: the shape of the key, which
//! fields the form carries, the size of the file, which status it asked for, which
//! condition the policy states and which half of it the form breaks, and which event value
//! a notification configuration named.

use serde_json::{json, Value};

use super::object_event_notifications::{queue_arn, Answer, Destination, Emulator};

/// The boundary every form in this suite is submitted with. A boundary is a client's own
/// choice of a delimiter that does not occur in the payload; one serves every row.
const BOUNDARY: &str = "----------emulator-form-boundary";

/// The bytes the attached file carries unless a row names its own.
const FILE_BYTES: &[u8] = b"browser,form,upload\n1,2,3\n";

/// The name the browser states for the attached file.
const FILE_NAME: &str = "quarterly.csv";

/// An expiration no run of this suite reaches, for a policy that is meant to be usable.
const UNEXPIRED: &str = "2999-01-01T00:00:00Z";

/// One part of a submitted form: the field name and its value.
type FormField<'a> = (&'a str, &'a str);

/// The `multipart/form-data` document a browser submits for `fields`, with `file` attached
/// as the `file` part under `file_name`.
///
/// Built by hand because this is the WIRE format under test: a client's form is a sequence
/// of parts, each introduced by the boundary and carrying its own
/// `Content-Disposition: form-data` head, and the file part is the one that also states a
/// `filename`.
///
/// `pub(crate)`: the announced-URL suite ([`super::announced_response_urls`]) submits a form
/// of its own to read the `Location` the answer names, and drives it through this one
/// document builder rather than keeping a second copy of the wire format.
pub(crate) fn form_document(
    fields: &[FormField<'_>],
    file: Option<(&str, &[u8], Option<&str>)>,
) -> Vec<u8> {
    let mut document = Vec::new();
    for (name, value) in fields {
        document.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        document.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        document.extend_from_slice(value.as_bytes());
        document.extend_from_slice(b"\r\n");
    }
    if let Some((file_name, bytes, media_type)) = file {
        document.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        document.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n"
            )
            .as_bytes(),
        );
        if let Some(media_type) = media_type {
            document.extend_from_slice(format!("Content-Type: {media_type}\r\n").as_bytes());
        }
        document.extend_from_slice(b"\r\n");
        document.extend_from_slice(bytes);
        document.extend_from_slice(b"\r\n");
    }
    document.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    document
}

/// The media type a form submission announces, with the boundary its parts are delimited
/// by.
///
/// `pub(crate)` for the same reason as [`form_document`].
pub(crate) fn form_media_type() -> String {
    format!("multipart/form-data; boundary={BOUNDARY}")
}

/// `POST /<bucket>/` — the browser-form upload, with `fields` and the attached file.
async fn post_form(
    emulator: &Emulator,
    bucket: &str,
    fields: &[FormField<'_>],
    file: Option<(&str, &[u8], Option<&str>)>,
) -> Answer {
    let media_type = form_media_type();
    emulator
        .send(
            "s3",
            "POST",
            &format!("/{bucket}/"),
            &[("content-type", media_type.as_str())],
            form_document(fields, file),
        )
        .await
}

/// `GET /<bucket>/<key>` — what the upload really stored, which is what makes a served
/// answer more than a claim.
async fn get_object(emulator: &Emulator, bucket: &str, key: &str) -> Answer {
    emulator
        .send(
            "s3",
            "GET",
            &format!("/{bucket}/{}", key.replace(' ', "%20")),
            &[],
            Vec::new(),
        )
        .await
}

/// The text of the first `<name>` element of a document, when it carries one, with the
/// XML escaping the serializer applies undone — a refusal's sentence names the policy
/// condition it failed, which is written with quotes.
fn element(body: &str, name: &str) -> Option<String> {
    body.split_once(&format!("<{name}>"))
        .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
        .map(|(text, _)| {
            text.replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&")
        })
}

/// The base64-encoded policy document a form carries, with `conditions` as its conditions
/// and `expiration` as its expiration.
fn encoded_policy(expiration: &str, conditions: Value) -> String {
    use base64::Engine;

    let document = json!({ "expiration": expiration, "conditions": conditions });
    base64::engine::general_purpose::STANDARD.encode(document.to_string())
}

/// One row of the served-upload table.
struct Served {
    /// The case every assertion of the row reports.
    case: &'static str,
    /// The fields the form carries beside the policy, `key` among them.
    fields: Vec<(&'static str, String)>,
    /// The conditions the policy states, or `None` for a form carrying no policy at all.
    conditions: Option<Value>,
    /// The bytes the attached file carries.
    bytes: &'static [u8],
    /// The media type the file part announces, when any.
    part_media_type: Option<&'static str>,
    /// The status the upload must be answered with.
    status: u16,
    /// The key the object must be stored at.
    key: &'static str,
    /// The `Content-Type` the stored object must report.
    content_type: Option<&'static str>,
}

/// RULE: a form upload stores the attached file at the key the form names and answers the
/// status the form asked for; the stored object carries what the form's fields configured.
///
/// The rows differ along what the rule generalises over: the shape of the key (plain,
/// prefixed, carrying a space, carrying the `${filename}` placeholder), which
/// `success_action_status` was asked for — each of the three S3 publishes, one it does
/// not, and none at all — whether the form carries a policy, the size of the file
/// (including an empty one), and where the object's content type comes from (the form's own
/// field or the file part's).
#[tokio::test]
async fn a_form_upload_stores_the_file_and_answers_the_status_the_form_asked_for() {
    let emulator = Emulator::new();
    let bucket = "form-uploads";
    emulator.create_bucket(bucket).await;

    let rows: Vec<Served> = vec![
        Served {
            case: "no status asked for, answered with the default",
            fields: vec![("key", "plain.csv".to_string())],
            conditions: Some(json!([{ "bucket": "form-uploads" }])),
            bytes: FILE_BYTES,
            part_media_type: Some("text/csv"),
            status: 204,
            key: "plain.csv",
            content_type: Some("text/csv"),
        },
        Served {
            case: "the created status, which carries the PostResponse document",
            fields: vec![
                ("key", "uploads/report.csv".to_string()),
                ("success_action_status", "201".to_string()),
                ("Content-Type", "text/csv".to_string()),
                ("Cache-Control", "max-age=60".to_string()),
                ("x-amz-meta-owner", "finance".to_string()),
            ],
            conditions: Some(json!([
                { "bucket": "form-uploads" },
                ["starts-with", "$key", "uploads/"],
                ["starts-with", "$Content-Type", "text/"],
                ["starts-with", "$Cache-Control", ""],
                ["starts-with", "$x-amz-meta-owner", ""],
                ["content-length-range", 0, 1048576],
            ])),
            bytes: FILE_BYTES,
            // The form states its own content type, which is the one the object is stored
            // with even though the attached part announces another.
            part_media_type: Some("application/octet-stream"),
            status: 201,
            key: "uploads/report.csv",
            content_type: Some("text/csv"),
        },
        Served {
            case: "the empty 200",
            fields: vec![
                ("key", "two-hundred.bin".to_string()),
                ("success_action_status", "200".to_string()),
            ],
            conditions: Some(json!([])),
            bytes: b"\x00\x01\x02",
            part_media_type: Some("application/octet-stream"),
            status: 200,
            key: "two-hundred.bin",
            content_type: Some("application/octet-stream"),
        },
        Served {
            case: "the empty 204, asked for explicitly",
            fields: vec![
                ("key", "two-oh-four.txt".to_string()),
                ("success_action_status", "204".to_string()),
            ],
            conditions: Some(json!([])),
            bytes: b"",
            part_media_type: Some("text/plain"),
            status: 204,
            key: "two-oh-four.txt",
            content_type: Some("text/plain"),
        },
        Served {
            case: "a status S3 does not publish, answered with the default",
            fields: vec![
                ("key", "ignored-status.txt".to_string()),
                ("success_action_status", "302".to_string()),
            ],
            conditions: Some(json!([])),
            bytes: FILE_BYTES,
            part_media_type: Some("text/csv"),
            status: 204,
            key: "ignored-status.txt",
            content_type: Some("text/csv"),
        },
        Served {
            case: "the filename placeholder, which the attached file's name fills",
            fields: vec![
                ("key", "uploads/${filename}".to_string()),
                ("success_action_status", "201".to_string()),
            ],
            conditions: Some(json!([["starts-with", "$key", "uploads/"]])),
            bytes: FILE_BYTES,
            part_media_type: Some("text/csv"),
            status: 201,
            key: "uploads/quarterly.csv",
            content_type: Some("text/csv"),
        },
        Served {
            case: "a key carrying a space",
            fields: vec![
                ("key", "quarter three.csv".to_string()),
                ("success_action_status", "201".to_string()),
            ],
            conditions: Some(json!([{ "key": "quarter three.csv" }])),
            bytes: FILE_BYTES,
            part_media_type: Some("text/csv"),
            status: 201,
            key: "quarter three.csv",
            content_type: Some("text/csv"),
        },
        Served {
            case: "a form carrying no policy at all",
            fields: vec![("key", "no-policy.csv".to_string())],
            conditions: None,
            bytes: FILE_BYTES,
            part_media_type: Some("text/csv"),
            status: 204,
            key: "no-policy.csv",
            content_type: Some("text/csv"),
        },
    ];

    for row in rows {
        let case = row.case;
        let policy = row.conditions.map(|conditions| encoded_policy(UNEXPIRED, conditions));
        let mut fields: Vec<(&str, &str)> = row
            .fields
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        if let Some(policy) = policy.as_deref() {
            fields.push(("policy", policy));
            fields.push(("x-amz-algorithm", "AWS4-HMAC-SHA256"));
            fields.push(("x-amz-credential", "emulator-test-key/20260101/us-east-1/s3/aws4_request"));
            fields.push(("x-amz-date", "20260101T000000Z"));
            fields.push(("x-amz-signature", "not-verified-by-this-emulator"));
        }
        let answer = post_form(
            &emulator,
            bucket,
            &fields,
            Some((FILE_NAME, row.bytes, row.part_media_type)),
        )
        .await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with {}",
            row.status,
            answer.status,
            answer.text()
        );
        let etag = answer.header("etag").unwrap_or_default();
        assert!(
            etag.starts_with('"') && etag.ends_with('"') && etag.len() > 2,
            "case {case:?}: expected the stored object's quoted ETag on the answer, got \
             {etag:?}"
        );
        if row.status == 201 {
            let body = answer.text();
            assert_eq!(
                element(&body, "Bucket").as_deref(),
                Some(bucket),
                "case {case:?}: expected the PostResponse to name the bucket, got {body}"
            );
            assert_eq!(
                element(&body, "Key").as_deref(),
                Some(row.key),
                "case {case:?}: expected the PostResponse to name the key, got {body}"
            );
            assert_eq!(
                element(&body, "ETag").map(|etag| etag.replace("&quot;", "\"")),
                Some(etag.clone()),
                "case {case:?}: expected the PostResponse to carry the answered ETag, got \
                 {body}"
            );
            let location = element(&body, "Location").unwrap_or_default();
            assert!(
                location.contains(bucket),
                "case {case:?}: expected the PostResponse Location to address the bucket, \
                 got {location:?}"
            );
        } else {
            assert!(
                answer.text().is_empty(),
                "case {case:?}: expected an empty document, got {}",
                answer.text()
            );
        }
        // What the upload really stored, read back through the object plane.
        let stored = get_object(&emulator, bucket, row.key).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: expected the uploaded object to be readable at {:?}, got {} \
             with {}",
            row.key,
            stored.status,
            stored.text()
        );
        assert_eq!(
            stored.bytes(),
            row.bytes,
            "case {case:?}: expected the attached file's bytes at {:?}, got {:?}",
            row.key,
            stored.text()
        );
        assert_eq!(
            stored.header("content-type").as_deref(),
            row.content_type,
            "case {case:?}: expected the stored content type {:?}, got {:?}",
            row.content_type,
            stored.header("content-type")
        );
        assert_eq!(
            stored.header("etag").as_deref(),
            Some(etag.as_str()),
            "case {case:?}: expected the stored object to carry the ETag the upload \
             answered, got {:?}",
            stored.header("etag")
        );
    }
}

/// One row of the refused-upload table.
struct Refused {
    /// The case every assertion of the row reports.
    case: &'static str,
    /// The fields the form carries, `key` among them unless the row is about its absence.
    fields: Vec<(&'static str, String)>,
    /// Whether the form attaches a file at all.
    attaches_file: bool,
    /// The bytes the attached file carries.
    bytes: &'static [u8],
    /// The status, the `<Code>` and the `<Message>` the refusal must carry.
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// The sentence S3 refuses a form that states no `key` with.
const KEY_FIELD_MISSING: &str = "Bucket POST must contain a field named 'key'. If it is \
                                 specified, please check the order of the fields.";

/// RULE: a form the policy it carries does not allow, and a form that is not a complete
/// POST at all, store nothing and are refused with S3's own published code and sentence.
///
/// The rows differ along what the rule generalises over: which half of the policy refuses
/// the form (its expiration, an `eq` condition, a `starts-with` condition, the map form,
/// the bucket it names, each end of the `content-length-range`), and which part of the form
/// itself is missing or unreadable.
#[tokio::test]
async fn a_form_the_policy_does_not_allow_stores_nothing() {
    let emulator = Emulator::new();
    let bucket = "refused-uploads";
    emulator.create_bucket(bucket).await;

    let key = "uploads/refused.csv";
    let rows: Vec<Refused> = vec![
        Refused {
            case: "a policy whose expiration has passed",
            fields: vec![
                ("key", key.to_string()),
                ("policy", encoded_policy("2000-01-01T00:00:00Z", json!([]))),
            ],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 403,
            code: "AccessDenied",
            message: "Invalid according to Policy: Policy expired.",
        },
        Refused {
            case: "an eq condition the submitted key does not satisfy",
            fields: vec![
                ("key", key.to_string()),
                (
                    "policy",
                    encoded_policy(UNEXPIRED, json!([["eq", "$key", "uploads/allowed.csv"]])),
                ),
            ],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 403,
            code: "AccessDenied",
            message: "Invalid according to Policy: Policy Condition failed: [\"eq\", \
                      \"$key\", \"uploads/allowed.csv\"]",
        },
        Refused {
            case: "a starts-with condition the submitted key does not satisfy",
            fields: vec![
                ("key", key.to_string()),
                (
                    "policy",
                    encoded_policy(UNEXPIRED, json!([["starts-with", "$key", "private/"]])),
                ),
            ],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 403,
            code: "AccessDenied",
            message: "Invalid according to Policy: Policy Condition failed: \
                      [\"starts-with\", \"$key\", \"private/\"]",
        },
        Refused {
            case: "a map condition on a field the form submitted differently",
            fields: vec![
                ("key", key.to_string()),
                ("acl", "public-read".to_string()),
                ("policy", encoded_policy(UNEXPIRED, json!([{ "acl": "private" }]))),
            ],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 403,
            code: "AccessDenied",
            message: "Invalid according to Policy: Policy Condition failed: [\"eq\", \
                      \"$acl\", \"private\"]",
        },
        Refused {
            case: "a policy written for another bucket",
            fields: vec![
                ("key", key.to_string()),
                (
                    "policy",
                    encoded_policy(UNEXPIRED, json!([{ "bucket": "another-bucket" }])),
                ),
            ],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 403,
            code: "AccessDenied",
            message: "Invalid according to Policy: Policy Condition failed: [\"eq\", \
                      \"$bucket\", \"another-bucket\"]",
        },
        Refused {
            case: "a file below the policy's content-length-range",
            fields: vec![
                ("key", key.to_string()),
                (
                    "policy",
                    encoded_policy(UNEXPIRED, json!([["content-length-range", 1024, 2048]])),
                ),
            ],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 400,
            code: "EntityTooSmall",
            message: "Your proposed upload is smaller than the minimum allowed object size.",
        },
        Refused {
            case: "a file above the policy's content-length-range",
            fields: vec![
                ("key", key.to_string()),
                (
                    "policy",
                    encoded_policy(UNEXPIRED, json!([["content-length-range", 0, 4]])),
                ),
            ],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 400,
            code: "EntityTooLarge",
            message: "Your proposed upload exceeds the maximum allowed size.",
        },
        Refused {
            case: "a policy field that is not a policy document",
            fields: vec![
                ("key", key.to_string()),
                ("policy", "not-a-policy".to_string()),
            ],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 400,
            code: "InvalidPolicyDocument",
            message: "The content of the form does not meet the conditions specified in the \
                      policy document.",
        },
        Refused {
            case: "a form attaching no file at all",
            fields: vec![("key", key.to_string())],
            attaches_file: false,
            bytes: FILE_BYTES,
            status: 400,
            code: "IncorrectNumberOfFilesInPostRequest",
            message: "POST requires exactly one file upload per request.",
        },
        Refused {
            case: "a form stating no key",
            fields: vec![("acl", "private".to_string())],
            attaches_file: true,
            bytes: FILE_BYTES,
            status: 400,
            code: "InvalidArgument",
            message: KEY_FIELD_MISSING,
        },
    ];

    for row in rows {
        let case = row.case;
        let fields: Vec<(&str, &str)> = row
            .fields
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let file = row
            .attaches_file
            .then_some((FILE_NAME, row.bytes, Some("text/csv")));
        let answer = post_form(&emulator, bucket, &fields, file).await;
        let body = answer.text();
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with {body}",
            row.status, answer.status
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some(row.code),
            "case {case:?}: expected the code {:?}, got {body}",
            row.code
        );
        assert_eq!(
            element(&body, "Message").as_deref(),
            Some(row.message),
            "case {case:?}: expected the published sentence, got {body}"
        );
        // Nothing is stored by a refused upload, which is what makes the refusal real.
        let stored = get_object(&emulator, bucket, key).await;
        assert_eq!(
            stored.status, 404,
            "case {case:?}: a refused upload must store nothing, got HTTP {} reading \
             {key:?} back",
            stored.status
        );
    }
}

/// One row of the key-ceiling table: the case, the `key` field the form states, the name
/// the browser states for its file, and the status plus `<Code>` the upload must answer
/// (`None` for a key S3 stores).
type CeilingRow = (&'static str, String, String, u16, Option<&'static str>);

/// RULE: the key a form upload stores is bounded by S3's published key ceiling — "the name
/// for a key is a sequence of Unicode characters whose UTF-8 encoding is at most 1,024 bytes
/// long" (Amazon S3 User Guide, *Creating object key names*) — and the `${filename}`
/// expansion is judged by the same ceiling, so neither the stated key, nor the attached
/// file's name, nor what the expansion makes of them can exceed it.
///
/// The rows differ along the published bound itself, one byte past it on each of the two
/// inputs, and an expansion that crosses it from two inputs that do not.
#[tokio::test]
async fn the_key_ceiling_bounds_both_the_stated_key_and_the_filename_expansion() {
    let emulator = Emulator::new();
    let bucket = "key-ceiling-uploads";
    emulator.create_bucket(bucket).await;

    let rows: [CeilingRow; 4] = [
        (
            "the longest key S3 stores",
            "k".repeat(1024),
            FILE_NAME.to_string(),
            204,
            None,
        ),
        (
            "a stated key one byte past the ceiling",
            "k".repeat(1025),
            FILE_NAME.to_string(),
            400,
            Some("KeyTooLongError"),
        ),
        (
            "a file name one byte past the ceiling",
            "${filename}".to_string(),
            "f".repeat(1025),
            400,
            Some("KeyTooLongError"),
        ),
        (
            "an expansion that crosses the ceiling from two inputs that do not",
            "${filename}${filename}".to_string(),
            "f".repeat(600),
            400,
            Some("KeyTooLongError"),
        ),
    ];

    for (case, stated_key, file_name, status, code) in rows {
        let answer = post_form(
            &emulator,
            bucket,
            &[("key", stated_key.as_str())],
            Some((file_name.as_str(), FILE_BYTES, Some("text/csv"))),
        )
        .await;
        let body = answer.text();
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {body}",
            answer.status
        );
        match code {
            Some(code) => assert_eq!(
                element(&body, "Code").as_deref(),
                Some(code),
                "case {case:?}: expected the code {code:?}, got {body}"
            ),
            // The key S3 stores is readable back at the key the form named.
            None => {
                let stored = get_object(&emulator, bucket, &stated_key).await;
                assert_eq!(
                    stored.status, 200,
                    "case {case:?}: expected the stored object to be readable, got HTTP {}",
                    stored.status
                );
            }
        }
    }
}

/// One row of the redirect table: the case, the URL the form names, and the prefix the
/// answer's `Location` must carry (`None` for a value that is no URL).
type RedirectRow = (&'static str, &'static str, Option<&'static str>);

/// RULE: a valid `success_action_redirect` is answered `303 See Other` with the bucket, the
/// key and the ETag added to that URL, and the object is stored all the same.
#[tokio::test]
async fn a_redirecting_form_is_answered_with_a_see_other_naming_what_was_stored() {
    let emulator = Emulator::new();
    let bucket = "redirecting-uploads";
    emulator.create_bucket(bucket).await;

    let rows: [RedirectRow; 3] = [
        (
            "an absolute URL",
            "https://console.example.test/uploaded",
            Some("https://console.example.test/uploaded?"),
        ),
        (
            "an absolute URL already carrying a query",
            "https://console.example.test/uploaded?from=form",
            Some("https://console.example.test/uploaded?from=form&"),
        ),
        // "If the value of success_action_redirect is not valid, Amazon S3 ignores the
        // field", so the status field answers instead.
        ("a value that is no URL", "/uploaded", None),
    ];

    for (index, (case, redirect, expected)) in rows.into_iter().enumerate() {
        let key = format!("redirect-{index}.csv");
        let answer = post_form(
            &emulator,
            bucket,
            &[
                ("key", key.as_str()),
                ("success_action_redirect", redirect),
            ],
            Some((FILE_NAME, FILE_BYTES, Some("text/csv"))),
        )
        .await;
        match expected {
            Some(prefix) => {
                assert_eq!(
                    answer.status, 303,
                    "case {case:?}: expected HTTP 303, got {} with {}",
                    answer.status,
                    answer.text()
                );
                let location = answer.header("location").unwrap_or_default();
                assert!(
                    location.starts_with(prefix),
                    "case {case:?}: expected the redirect to start with {prefix:?}, got \
                     {location:?}"
                );
                for parameter in ["bucket=", "key=", "etag="] {
                    assert!(
                        location.contains(parameter),
                        "case {case:?}: expected the redirect to carry {parameter:?}, got \
                         {location:?}"
                    );
                }
            }
            None => assert_eq!(
                answer.status, 204,
                "case {case:?}: expected the default HTTP 204, got {} with {}",
                answer.status,
                answer.text()
            ),
        }
        let stored = get_object(&emulator, bucket, &key).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the object must be stored whatever the answer was, got HTTP {}",
            stored.status
        );
    }
}

/// One row of the notification table: the case, the `Event` value the configuration names,
/// and whether a form upload notifies it.
type EventRow = (&'static str, &'static str, bool);

/// RULE: the event a form upload publishes is the `s3:ObjectCreated:Post` of the key it
/// stored — so a notification configuration naming that event value, or the wildcard over
/// the type, is notified and one naming the `PUT` of an object is not.
///
/// The rows differ along the event value the configuration names: the one the upload is,
/// the one it is not, and the wildcard that stands for "every event of this type".
#[tokio::test]
async fn a_form_upload_notifies_the_destinations_scoped_to_the_post_of_an_object() {
    let rows: [EventRow; 3] = [
        ("the POST of an object", "s3:ObjectCreated:Post", true),
        ("the PUT of an object", "s3:ObjectCreated:Put", false),
        ("every creation of an object", "s3:ObjectCreated:*", true),
    ];

    for (index, (case, event, notified)) in rows.into_iter().enumerate() {
        // One emulator per row, so one row's configuration cannot notify another's queue.
        let emulator = Emulator::new();
        let bucket = format!("post-events-{index}");
        emulator.create_bucket(&bucket).await;
        let queue = format!("post-events-queue-{index}");
        let url = emulator.create_queue(&queue).await;
        let arn = queue_arn(&queue);
        emulator
            .put_notification(&bucket, &Destination::queue(&arn), &[event], &[], false)
            .await;

        let key = "uploads/notified.csv";
        let answer = post_form(
            &emulator,
            &bucket,
            &[("key", key), ("success_action_status", "201")],
            Some((FILE_NAME, FILE_BYTES, Some("text/csv"))),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: the upload must succeed, got {} with {}",
            answer.status,
            answer.text()
        );

        if !notified {
            let delivered = emulator.nothing_delivered(&url).await;
            assert!(
                delivered.is_empty(),
                "case {case:?}: a configuration naming {event:?} must not be notified of a \
                 form upload, got {delivered:?}"
            );
            continue;
        }
        let record = emulator.delivered(&url).await.unwrap_or_else(|| {
            panic!("case {case:?}: a configuration naming {event:?} must be notified")
        });
        assert_eq!(
            record["Records"][0]["eventName"].as_str(),
            Some("ObjectCreated:Post"),
            "case {case:?}: expected the record to name the POST of an object, got {record}"
        );
        assert_eq!(
            record["Records"][0]["s3"]["object"]["key"].as_str(),
            Some("uploads/notified.csv"),
            "case {case:?}: expected the record to name the stored key, got {record}"
        );
        assert_eq!(
            record["Records"][0]["s3"]["object"]["size"].as_i64(),
            Some(FILE_BYTES.len() as i64),
            "case {case:?}: expected the record to report the stored size, got {record}"
        );
    }
}

/// RULE: the EventBridge event a form upload publishes is an "Object Created" whose
/// `reason` is "POST Object", so a rule scoped to the reason of a `PutObject` does not
/// receive it and one scoped to the POST's own reason does.
#[tokio::test]
async fn the_event_bridge_reason_of_a_form_upload_is_the_post_of_an_object() {
    let emulator = Emulator::new();
    let bucket = "post-bus-uploads";
    emulator.create_bucket(bucket).await;

    // Two rules over the same bus, each scoped to one of the two reasons an "Object
    // Created" event carries, so one upload decides both at once.
    let post_queue = emulator.create_queue("post-reason-queue").await;
    let put_queue = emulator.create_queue("put-reason-queue").await;
    for (rule, reason, queue) in [
        ("post-reason", "POST Object", "post-reason-queue"),
        ("put-reason", "PutObject", "put-reason-queue"),
    ] {
        emulator
            .events(
                "PutRule",
                &json!({
                    "Name": rule,
                    "EventPattern": json!({
                        "source": ["aws.s3"],
                        "detail-type": ["Object Created"],
                        "detail": { "reason": [reason] },
                    })
                    .to_string(),
                }),
            )
            .await;
        emulator
            .events(
                "PutTargets",
                &json!({
                    "Rule": rule,
                    "Targets": [{ "Id": "the-queue", "Arn": queue_arn(queue) }],
                }),
            )
            .await;
    }
    emulator
        .put_notification(
            bucket,
            &Destination::queue(&queue_arn("post-reason-queue")),
            // A native destination the upload does NOT notify, so the only thing that
            // reaches the queue below is what the bus delivered: this configuration is
            // here for the `EventBridgeConfiguration` beside it, which is what "enables
            // delivery of events to Amazon EventBridge".
            &["s3:ObjectRemoved:Delete"],
            &[],
            true,
        )
        .await;

    let answer = post_form(
        &emulator,
        bucket,
        &[("key", "bus/report.csv"), ("success_action_status", "201")],
        Some((FILE_NAME, FILE_BYTES, Some("text/csv"))),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "the upload must succeed, got {} with {}",
        answer.status,
        answer.text()
    );

    // The POST-scoped rule receives the envelope; the bucket's own native destination is
    // configured for an event this upload is not, so nothing else reaches this queue.
    let envelope = emulator
        .delivered(&post_queue)
        .await
        .expect("a rule scoped to the POST of an object must receive the envelope");
    assert_eq!(
        envelope["detail"]["reason"].as_str(),
        Some("POST Object"),
        "expected the envelope's reason to name the POST of an object, got {envelope}"
    );
    assert_eq!(
        envelope["detail"]["object"]["key"].as_str(),
        Some("bus/report.csv"),
        "expected the envelope to name the stored key, got {envelope}"
    );
    // "`time` is an ISO-8601 instant with whole-second precision and a trailing `Z`", which
    // is the form every consumer of an envelope parses.
    let time = envelope["time"].as_str().unwrap_or_default();
    assert!(
        time.len() == 20 && time.ends_with('Z') && !time.contains('.'),
        "expected the envelope's time at whole-second precision, got {time:?}"
    );

    let withheld = emulator.nothing_delivered(&put_queue).await;
    assert!(
        withheld.is_empty(),
        "a rule scoped to the reason of a PutObject must not receive a form upload's \
         envelope, got {withheld:?}"
    );
}
