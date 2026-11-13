// SPDX-License-Identifier: CC0-1.0

//! S3's BROWSER-BASED upload: the HTML-form `POST` of one object to a bucket.
//!
//! "The POST operation adds an object to a specified bucket by using HTML forms"
//! (Amazon S3 API Reference, *POST Object*). It is the one object write that is NOT a
//! modelled operation of this service — `smithy_query(query="list_operations")` names no
//! `PostObject`, because the form a browser submits is not a Smithy input shape — so no
//! routing-table entry claims `POST /<bucket>/` and the framework answered it with the
//! "nobody has written this yet" 501. Every browser and mobile direct-upload flow is that
//! request, so this module is what claims and serves it.
//!
//! ## What one request carries
//!
//! The payload is a `multipart/form-data` document whose parts are the form's fields, read
//! here with a maintained parser for that media type rather than by slicing bytes:
//!
//! * `key` — the object's key, which "`${filename}`" in it stands for the name of the file
//!   the browser attached;
//! * `file` — the object's bytes, the one part that carries a file name;
//! * `policy` plus `x-amz-credential` / `x-amz-algorithm` / `x-amz-date` /
//!   `x-amz-signature` — what AUTHORIZES the upload, see below;
//! * the fields that configure the stored object — `acl`, `Content-Type`,
//!   `Cache-Control`, `Content-Disposition`, `Content-Encoding`, `Expires`, `tagging`,
//!   `x-amz-tagging`, `x-amz-storage-class`, `x-amz-website-redirect-location`, the
//!   server-side-encryption fields and every `x-amz-meta-*` — each of which fills the same
//!   member the header of the same name fills on a `PutObject`, so ONE object write serves
//!   both calls ([`crate::ops::put_object::PutObject::write`]) and a form upload cannot
//!   drift from a `PUT` in what it stores;
//! * `success_action_status` or `success_action_redirect` / `redirect` — what the browser
//!   is ANSWERED with, see below.
//!
//! A field name is read case-insensitively ("field names are not case sensitive"), the way
//! the header it stands for is.
//!
//! ## What authorizes one request
//!
//! A form POST carries no `Authorization` header: its credential is in the form, which is
//! why this operation declares its own (empty) auth schemes — there is no SigV4 signature
//! for the framework to verify. Nothing is skipped by that:
//!
//! * the POLICY is enforced here — "the policy is a UTF-8 and base64-encoded document
//!   written in JSON" whose `expiration` and `conditions` say what the form may contain,
//!   and a form the policy does not allow is refused rather than stored;
//! * the policy's SIGNATURE is verified against the secret of the credential the form
//!   names, for a credential this process can resolve the secret of — the key pair the
//!   emulator publishes at start, which is what a client of this emulator signs with. A
//!   credential minted by another service is held in the emulator's credential store,
//!   whose secrets no service reads, and so is a signature computed under a scheme this
//!   derivation is not of; such a form is admitted on its policy alone — the same rule this
//!   service already resolves a signed request's principal by
//!   ([`crate::service::request_authorization::request_caller`]: a key id no store here
//!   holds speaks for the account), and it is logged;
//! * what the resolved caller may then WRITE is the bucket's own business, decided by the
//!   one authorization every object write of this service is decided by, inside the write
//!   this module delegates to.
//!
//! ## What one request is answered with
//!
//! "If the value of `success_action_redirect` is not valid … S3 returns the empty document
//! type specified in the `success_action_status` field", and "if the value is set to 201,
//! Amazon S3 returns an XML document" (*POST Object*'s own documentation):
//!
//! * a valid `success_action_redirect` is answered `303 See Other` with that URL, carrying
//!   the bucket, the key and the ETag it was uploaded under;
//! * otherwise `success_action_status` decides: `200` and `204` answer with an empty
//!   document, `201` answers with the `PostResponse` document naming the object's
//!   `Location`, `Bucket`, `Key` and `ETag`;
//! * a `success_action_status` the form does not state, or states as anything else,
//!   answers `204`.
//!
//! ## The event the upload publishes
//!
//! The write is an `s3:ObjectCreated:Post`, whose EventBridge detail reports the API call
//! it came of as "POST Object" — not "PutObject"
//! ([`crate::state::notifications::ObjectEventKind::CreatedByPost`]) — so a rule scoped to
//! one of the two reasons does not receive the other's events.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, OperationError, Service,
};
use ::std::sync::Arc;
use ::std::time::{Duration, SystemTime};

use crate::ops::types::{PutObjectInput, PutObjectOutput};
use crate::state::notifications::ObjectEventKind;

/// The key this service serves a browser-form upload under.
///
/// Not a routing key the model's protocol can resolve — no modelled operation is bound at
/// `POST /{Bucket}` — so nothing but [`super::EmulatedService::override_operation`] ever
/// looks it up, and it cannot collide with a modelled operation's own name.
pub const POST_OBJECT: &str = "PostObject";

/// The media type of the payload a browser form submits, which is what tells a form upload
/// from every other `POST` addressed to a bucket.
const FORM_DATA_MEDIA_TYPE: &str = "multipart/form-data";

/// Whether `content_type` — a request's own `Content-Type` — announces the form document a
/// browser upload carries.
///
/// The media type is read off the head of the value, because the boundary travels in its
/// parameters (`multipart/form-data; boundary=…`), and case-insensitively, as a media type
/// is compared.
pub(super) fn carries_form_payload(content_type: Option<&str>) -> bool {
    let Some(value) = content_type else {
        return false;
    };
    let media_type = value.split(';').next().unwrap_or(value).trim();
    media_type.eq_ignore_ascii_case(FORM_DATA_MEDIA_TYPE)
}

// ---------------------------------------------------------------------------
// The form's own fields
// ---------------------------------------------------------------------------

/// The field naming the object's key: "the name of the uploaded key" (*POST Object*).
const KEY_FIELD: &str = "key";
/// The field carrying the object's bytes, the one part a browser attaches a file to.
const FILE_FIELD: &str = "file";
/// The field carrying the base64-encoded policy document.
const POLICY_FIELD: &str = "policy";
/// The field carrying the signature of that document, in the SigV4 form.
const SIGNATURE_FIELD: &str = "x-amz-signature";
/// The field naming the credential that signed it, in the SigV4 form:
/// `<access-key-id>/<date>/<region>/<service>/aws4_request`.
const CREDENTIAL_FIELD: &str = "x-amz-credential";
/// The field naming the status an upload is answered with.
const SUCCESS_ACTION_STATUS_FIELD: &str = "success_action_status";
/// The field naming the URL the browser is redirected to.
const SUCCESS_ACTION_REDIRECT_FIELD: &str = "success_action_redirect";
/// The name that same redirect also goes by.
const REDIRECT_FIELD: &str = "redirect";
/// The prefix of every field that carries one piece of the object's user metadata.
const METADATA_FIELD_PREFIX: &str = "x-amz-meta-";
/// What `key` carries to be filled in with the name of the attached file: "if the user
/// provides a file, S3 replaces `${filename}` with the name of the uploaded file".
const FILENAME_PLACEHOLDER: &str = "${filename}";

/// One submitted form: its text fields in the order they arrived, and the attached file.
#[derive(Debug, Default)]
struct Form {
    /// Every field that is not the file, as `(name, value)`.
    fields: Vec<(String, String)>,
    /// The attached file: the name the browser stated for it, the media type it announced
    /// for the part, and its bytes.
    file: Option<(String, Option<String>, Vec<u8>)>,
}

impl Form {
    /// The value of the field named `name`, read case-insensitively, or `None` when the
    /// form carries no such field. The FIRST occurrence answers, the way one field names
    /// one value.
    fn value(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The value of whichever of `name` and `alternative` the form carries, in that order.
    fn either(&self, name: &str, alternative: &str) -> Option<&str> {
        self.value(name).or_else(|| self.value(alternative))
    }

    /// Every `x-amz-meta-*` field, as the metadata map the stored object carries: the name
    /// with that prefix removed beside its value.
    fn metadata(&self) -> ::std::collections::HashMap<String, String> {
        self.fields
            .iter()
            .filter_map(|(field, value)| {
                let name = field.get(..METADATA_FIELD_PREFIX.len())?;
                name.eq_ignore_ascii_case(METADATA_FIELD_PREFIX)
                    .then(|| (field[METADATA_FIELD_PREFIX.len()..].to_string(), value.clone()))
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// What a refused upload is answered with
// ---------------------------------------------------------------------------

/// One refusal of a form upload: the wire `<Code>`, the HTTP status and the `<Message>`.
///
/// The codes and their sentences are S3's own published ones (Amazon S3 API Reference,
/// *Error responses*, `ErrorCodeList`); the variable parts of a message come from the
/// request and from the policy it carried.
#[derive(Debug)]
struct Refusal {
    code: &'static str,
    status: u16,
    message: String,
}

impl Refusal {
    /// The refusal `code` carries, with `message` as its sentence.
    fn new(code: &'static str, status: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            status,
            message: message.into(),
        }
    }
}

/// `MalformedPOSTRequest` (400): "The body of your POST request is not well-formed
/// multipart/form-data."
const MALFORMED_POST_CODE: &str = "MalformedPOSTRequest";
/// See [`MALFORMED_POST_CODE`].
const MALFORMED_POST_MESSAGE: &str =
    "The body of your POST request is not well-formed multipart/form-data.";

/// `IncorrectNumberOfFilesInPostRequest` (400): "POST requires exactly one file upload per
/// request."
const FILE_COUNT_CODE: &str = "IncorrectNumberOfFilesInPostRequest";
/// See [`FILE_COUNT_CODE`].
const FILE_COUNT_MESSAGE: &str = "POST requires exactly one file upload per request.";

/// `InvalidArgument` (400), the code a form stating no `key` is refused with, and the
/// sentence S3 states for it — which also tells the caller that the field ORDER matters,
/// because a `key` after the file is not read.
const INVALID_ARGUMENT_CODE: &str = "InvalidArgument";
/// See [`INVALID_ARGUMENT_CODE`].
const KEY_FIELD_MISSING_MESSAGE: &str =
    "Bucket POST must contain a field named 'key'. If it is specified, please check the \
     order of the fields.";

/// `InvalidPolicyDocument` (400): "The content of the form does not meet the conditions
/// specified in the policy document." — the code for a `policy` field that is not a policy
/// document at all.
const INVALID_POLICY_CODE: &str = "InvalidPolicyDocument";
/// See [`INVALID_POLICY_CODE`].
const INVALID_POLICY_MESSAGE: &str =
    "The content of the form does not meet the conditions specified in the policy document.";

/// `EntityTooSmall` (400): "Your proposed upload is smaller than the minimum allowed
/// object size." — the refusal of a file below the policy's `content-length-range`.
const ENTITY_TOO_SMALL_CODE: &str = "EntityTooSmall";
/// See [`ENTITY_TOO_SMALL_CODE`].
const ENTITY_TOO_SMALL_MESSAGE: &str =
    "Your proposed upload is smaller than the minimum allowed object size.";

/// `EntityTooLarge` (400): "Your proposed upload exceeds the maximum allowed size." — the
/// refusal of a file above that same range.
const ENTITY_TOO_LARGE_CODE: &str = "EntityTooLarge";
/// See [`ENTITY_TOO_LARGE_CODE`].
const ENTITY_TOO_LARGE_MESSAGE: &str = "Your proposed upload exceeds the maximum allowed size.";

/// `SignatureDoesNotMatch` (403) and the sentence S3 states for it.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";
/// See [`SIGNATURE_MISMATCH_CODE`].
const SIGNATURE_MISMATCH_MESSAGE: &str =
    "The request signature we calculated does not match the signature you provided. Check \
     your key and signing method.";

/// The prefix of every sentence S3 refuses a form its own policy does not allow with.
const INVALID_ACCORDING_TO_POLICY: &str = "Invalid according to Policy: ";

/// What that sentence states for a policy whose `expiration` has passed.
const POLICY_EXPIRED: &str = "Policy expired.";

/// What it states for a condition the submitted form does not satisfy, with the condition
/// itself as the policy wrote it.
const POLICY_CONDITION_FAILED: &str = "Policy Condition failed: ";

/// The status and code of both: an upload the policy does not allow is an authorization
/// refusal, which S3 answers with its own `AccessDenied` (403).
const POLICY_REFUSAL_CODE: &str = crate::state::ACCESS_DENIED_CODE;
/// See [`POLICY_REFUSAL_CODE`].
const POLICY_REFUSAL_STATUS: u16 = crate::state::ACCESS_DENIED_STATUS;

/// The status a client error of the form itself carries.
const BAD_REQUEST_STATUS: u16 = 400;

// ---------------------------------------------------------------------------
// Reading the form
// ---------------------------------------------------------------------------

/// How many fields of one submitted form this upload reads.
///
/// An EMULATOR ceiling, not an AWS quota, and the companion to the body the framework has
/// already buffered: the published form carries a bounded set of fields (the key, the
/// policy and its credential, the ones that configure the object, and the caller's own
/// `x-amz-meta-*`), so a document naming more than this is not a form a browser submitted
/// and is refused rather than held in memory one field at a time.
const MAX_FORM_FIELDS: usize = 256;

/// The form `body` carries, or the refusal the document itself earns.
///
/// `content_type` is the request's own, which is where the multipart BOUNDARY travels. The
/// body is already buffered by the time any operation runs, so the parser reads it as a
/// one-chunk stream; a document it cannot read, and one carrying more fields than
/// [`MAX_FORM_FIELDS`], are the published `MalformedPOSTRequest`.
async fn read_form(content_type: &str, body: &[u8]) -> Result<Form, Refusal> {
    let malformed = || Refusal::new(MALFORMED_POST_CODE, BAD_REQUEST_STATUS, MALFORMED_POST_MESSAGE);
    let boundary = ::multer::parse_boundary(content_type).map_err(|_| malformed())?;
    let payload = ::bytes::Bytes::copy_from_slice(body);
    let stream = ::futures_util::stream::once(async move {
        Ok::<::bytes::Bytes, ::std::io::Error>(payload)
    });
    let mut document = ::multer::Multipart::new(stream, boundary);
    let mut form = Form::default();
    while let Some(field) = document.next_field().await.map_err(|_| malformed())? {
        if form.fields.len() >= MAX_FORM_FIELDS {
            return Err(malformed());
        }
        // Read off the part's head before its bytes, which consume the field.
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        let part_media_type = field.content_type().map(ToString::to_string);
        let bytes = field.bytes().await.map_err(|_| malformed())?;
        if name.eq_ignore_ascii_case(FILE_FIELD) {
            if form.file.is_some() {
                return Err(Refusal::new(
                    FILE_COUNT_CODE,
                    BAD_REQUEST_STATUS,
                    FILE_COUNT_MESSAGE,
                ));
            }
            form.file = Some((
                file_name.unwrap_or_default(),
                part_media_type,
                bytes.to_vec(),
            ));
            continue;
        }
        // Every other field is text: a form field's value is the part's own bytes, read as
        // the UTF-8 the policy and the headers it stands for are stated in.
        let Ok(value) = String::from_utf8(bytes.to_vec()) else {
            return Err(malformed());
        };
        form.fields.push((name, value));
    }
    Ok(form)
}

// ---------------------------------------------------------------------------
// The policy
// ---------------------------------------------------------------------------

/// The member of a policy document naming when it stops being usable: "the expiration
/// element specifies the expiration date and time of the POST policy in ISO8601 GMT date
/// format" (*Creating a POST policy*).
const POLICY_EXPIRATION: &str = "expiration";

/// The member listing what the form may contain: "the conditions in a policy document
/// validate the contents of the uploaded object".
const POLICY_CONDITIONS: &str = "conditions";

/// The condition matcher that compares a field EXACTLY, and the one that compares its
/// start: "`eq` — exact match" and "`starts-with`", the two the policy grammar names
/// beside `content-length-range`.
const EQ_MATCHER: &str = "eq";
/// See [`EQ_MATCHER`].
const STARTS_WITH_MATCHER: &str = "starts-with";
/// The condition bounding the file's size: "`content-length-range` — the minimum and
/// maximum allowable size for the uploaded content".
const CONTENT_LENGTH_RANGE: &str = "content-length-range";

/// The condition name that stands for the bucket the form was POSTed to rather than for a
/// field of the form.
const BUCKET_CONDITION: &str = "bucket";

/// The refusal a `policy` field that is not a well-formed policy document earns: it is not
/// an expired policy and not a condition that failed, so it is the client error S3 publishes
/// for a document it could not read as a policy.
fn invalid_policy_refusal() -> Refusal {
    Refusal::new(
        INVALID_POLICY_CODE,
        BAD_REQUEST_STATUS,
        INVALID_POLICY_MESSAGE,
    )
}

/// The decoded policy document, or the refusal a `policy` field that is not one earns.
///
/// "The policy is a UTF-8 and base64-encoded document written in JSON" (*Creating a POST
/// policy*), so it is decoded in that order; anything that is not a JSON object after both
/// steps is no policy document.
fn read_policy(encoded: &str) -> Result<::serde_json::Map<String, ::serde_json::Value>, Refusal> {
    use ::base64::Engine;

    let bytes = ::base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| invalid_policy_refusal())?;
    let document: ::serde_json::Value =
        ::serde_json::from_slice(&bytes).map_err(|_| invalid_policy_refusal())?;
    match document {
        ::serde_json::Value::Object(members) => Ok(members),
        _ => Err(invalid_policy_refusal()),
    }
}

/// The value the submitted form carries for the name a condition names: `bucket` names the
/// bucket the form was POSTed to rather than a field, and every other name is a FIELD of
/// the form, read the same case-insensitive way the upload reads it.
fn submitted_value<'f>(form: &'f Form, bucket: &'f str, name: &str) -> Option<&'f str> {
    if name.eq_ignore_ascii_case(BUCKET_CONDITION) {
        return Some(bucket);
    }
    form.value(name)
}

/// The refusal a form its own policy does not allow earns, stating `sentence`.
fn policy_refusal(sentence: String) -> Refusal {
    Refusal::new(
        POLICY_REFUSAL_CODE,
        POLICY_REFUSAL_STATUS,
        format!("{INVALID_ACCORDING_TO_POLICY}{sentence}"),
    )
}

/// The refusal one failed condition earns, naming the condition as the policy wrote it.
fn condition_refusal(matcher: &str, field: &str, value: &str) -> Refusal {
    policy_refusal(format!(
        "{POLICY_CONDITION_FAILED}{}",
        stated_condition(matcher, field, value)
    ))
}

/// Why the submitted form is not what `policy` allows, or `None` when the policy allows
/// it.
///
/// The three things a policy decides, in the order S3 states them: it has to still be
/// usable (`expiration`), every condition it carries has to hold for what the form
/// submitted, and the file's size has to lie inside the `content-length-range` it names.
/// A document that states no `expiration`, or states one that names no instant, is no
/// policy document rather than an expired policy ([`invalid_policy_refusal`]).
fn policy_rejection(
    policy: &::serde_json::Map<String, ::serde_json::Value>,
    form: &Form,
    bucket: &str,
    file_bytes: usize,
    now: SystemTime,
) -> Option<Refusal> {
    let stated = policy
        .get(POLICY_EXPIRATION)
        .and_then(::serde_json::Value::as_str);
    match stated.and_then(|stated| expiration_passed(stated, now)) {
        None => return Some(invalid_policy_refusal()),
        Some(true) => return Some(policy_refusal(POLICY_EXPIRED.to_string())),
        Some(false) => {}
    }
    for condition in policy
        .get(POLICY_CONDITIONS)
        .and_then(::serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        match condition {
            // `{"acl": "private"}` — an exact match of one field, the form the policy
            // grammar states as "a map".
            ::serde_json::Value::Object(members) => {
                for (name, value) in members {
                    let Some(stated) = value.as_str() else {
                        return Some(condition_refusal(EQ_MATCHER, name, &value.to_string()));
                    };
                    if submitted_value(form, bucket, name) != Some(stated) {
                        return Some(condition_refusal(EQ_MATCHER, name, stated));
                    }
                }
            }
            // `["eq", "$key", "uploads/x"]`, `["starts-with", "$key", "uploads/"]` and
            // `["content-length-range", 0, 1048576]` — the list form.
            ::serde_json::Value::Array(elements) => {
                if let Some(refusal) =
                    listed_condition_rejection(elements, form, bucket, file_bytes)
                {
                    return Some(refusal);
                }
            }
            // Anything else is no condition the grammar states, so nothing can satisfy it.
            other => {
                return Some(policy_refusal(format!(
                    "{POLICY_CONDITION_FAILED}{other}"
                )));
            }
        }
    }
    None
}

/// Why the submitted form does not satisfy ONE condition stated in the list form, or
/// `None` when it does.
fn listed_condition_rejection(
    elements: &[::serde_json::Value],
    form: &Form,
    bucket: &str,
    file_bytes: usize,
) -> Option<Refusal> {
    let matcher = elements.first().and_then(::serde_json::Value::as_str)?;
    if matcher.eq_ignore_ascii_case(CONTENT_LENGTH_RANGE) {
        let bound = |at: usize| elements.get(at).and_then(::serde_json::Value::as_u64);
        // "The minimum and maximum allowable size for the uploaded content" is TWO sizes;
        // a range stating anything else is no policy document rather than a range nothing
        // fits inside.
        let (Some(minimum), Some(maximum)) = (bound(1), bound(2)) else {
            return Some(invalid_policy_refusal());
        };
        let size = file_bytes as u64;
        if size < minimum {
            return Some(Refusal::new(
                ENTITY_TOO_SMALL_CODE,
                BAD_REQUEST_STATUS,
                ENTITY_TOO_SMALL_MESSAGE,
            ));
        }
        if size > maximum {
            return Some(Refusal::new(
                ENTITY_TOO_LARGE_CODE,
                BAD_REQUEST_STATUS,
                ENTITY_TOO_LARGE_MESSAGE,
            ));
        }
        return None;
    }
    // The field a condition names is written with the `$` of a policy variable in front of
    // it; the value is the third element.
    let named = elements.get(1).and_then(::serde_json::Value::as_str)?;
    let field = named.strip_prefix('$').unwrap_or(named);
    let stated = elements
        .get(2)
        .and_then(::serde_json::Value::as_str)
        .unwrap_or_default();
    let holds = match submitted_value(form, bucket, field) {
        Some(value) if matcher.eq_ignore_ascii_case(EQ_MATCHER) => value == stated,
        Some(value) if matcher.eq_ignore_ascii_case(STARTS_WITH_MATCHER) => {
            value.starts_with(stated)
        }
        // A matcher outside the grammar states a condition nothing satisfies, and a field
        // the form did not submit cannot satisfy one either.
        _ => false,
    };
    if holds {
        return None;
    }
    Some(condition_refusal(matcher, field, stated))
}

/// One failed condition as the refusal states it: the matcher, the field it named with its
/// `$`, and the value it required — the form the policy itself is written in.
fn stated_condition(matcher: &str, field: &str, value: &str) -> String {
    format!("[\"{matcher}\", \"${field}\", \"{value}\"]")
}

/// Whether the instant `stated` names has already passed at `now`, or `None` when the text
/// names no instant at all.
///
/// "ISO8601 GMT date format" is an RFC 3339 instant, read through the one reading this tree
/// resolves a zone with (`service_common::rfc3339`) so a policy written with a numeric
/// offset is read the same way as one written with `Z`.
fn expiration_passed(stated: &str, now: SystemTime) -> Option<bool> {
    let (civil, offset) = ::service_common::rfc3339::split_utc_offset(stated)?;
    let instant = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    let seconds = instant.secs() - offset;
    let since_epoch = now.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    match u64::try_from(seconds) {
        Ok(seconds) => Some(Duration::from_secs(seconds) <= since_epoch),
        // An expiration before the epoch is in the past by definition.
        Err(_) => Some(true),
    }
}

// ---------------------------------------------------------------------------
// The policy's signature
// ---------------------------------------------------------------------------

/// The field naming the algorithm the form's signature was computed with, and the one
/// value of it this derivation is: "`x-amz-algorithm` — the signing algorithm used. For
/// SigV4, this value is `AWS4-HMAC-SHA256`" (*Browser-based uploads using POST*).
const ALGORITHM_FIELD: &str = "x-amz-algorithm";
/// See [`ALGORITHM_FIELD`].
const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";
/// The terminator of a SigV4 credential scope.
const SIGV4_TERMINATOR: &str = "aws4_request";
/// What the derived signing key's first stage mixes the secret with.
const SIGV4_SECRET_PREFIX: &str = "AWS4";

/// Whether the signature the form carries is the one the policy it carries signs to, for a
/// credential whose secret this process can resolve — `None` when it cannot resolve it, so
/// nothing is judged.
///
/// The derivation is SigV4's own for a form upload: "`HMAC-SHA256(derived signing key,
/// base64-encoded policy)`", with the signing key derived from the date, the Region and
/// the service of the credential the form named (Amazon S3 API Reference, *Authenticating
/// requests: browser-based uploads using POST*).
fn signature_matches(form: &Form, encoded_policy: &str) -> Option<bool> {
    let credential = form.value(CREDENTIAL_FIELD)?;
    let signature = form.value(SIGNATURE_FIELD)?;
    if !form
        .value(ALGORITHM_FIELD)
        .is_some_and(|stated| stated.eq_ignore_ascii_case(SIGV4_ALGORITHM))
    {
        // A form stating another algorithm states a signature this derivation is not of,
        // so nothing here can judge it.
        return None;
    }
    let mut scope = credential.split('/');
    let key_id = scope.next()?;
    let date = scope.next()?;
    let region = scope.next()?;
    let service = scope.next()?;
    if scope.next() != Some(SIGV4_TERMINATOR) {
        return None;
    }
    let published = ::emulator_server::request_authentication::SigV4Authenticator::minted_credentials()?;
    if published.access_key_id != key_id {
        return None;
    }
    let seed = format!("{SIGV4_SECRET_PREFIX}{}", published.secret_access_key);
    let mut signing_key = hmac(seed.as_bytes(), date.as_bytes())?;
    for stage in [region, service, SIGV4_TERMINATOR] {
        signing_key = hmac(&signing_key, stage.as_bytes())?;
    }
    let expected = ::hex::encode(hmac(&signing_key, encoded_policy.trim().as_bytes())?);
    Some(expected.eq_ignore_ascii_case(signature.trim()))
}

/// One HMAC-SHA256 stage of that derivation, or `None` for a key the construction does not
/// take — which HMAC has none of, so nothing on a request path depends on the branch.
fn hmac(key: &[u8], message: &[u8]) -> Option<Vec<u8>> {
    use ::hmac::Mac;

    let mut mac = <::hmac::Hmac<::sha2::Sha256> as ::hmac::Mac>::new_from_slice(key).ok()?;
    mac.update(message);
    Some(mac.finalize().into_bytes().to_vec())
}

// ---------------------------------------------------------------------------
// What the form asks the object to be stored as
// ---------------------------------------------------------------------------

/// The object write one submitted form performs: the same input a `PutObject` of the same
/// object would carry, so one write serves both calls.
///
/// Each field fills the member the request HEADER of the same name fills on a `PutObject`
/// (`smithy_query(query="operation",operation="PutObject")` names those bindings), which is
/// what keeps a form upload and a `PUT` from storing different objects for the same values.
/// The file's own announced media type is the object's content type when the form states
/// none, which is what a browser attaching a file sends.
fn object_write(form: &Form, bucket: &str, key: &str, body: Vec<u8>) -> PutObjectInput {
    let field = |name: &str| form.value(name).map(str::to_string);
    let metadata = form.metadata();
    let content_type = form.value("Content-Type").map(str::to_string).or_else(|| {
        form.file
            .as_ref()
            .and_then(|(_, media_type, _)| media_type.clone())
    });
    PutObjectInput {
        bucket: bucket.to_string(),
        key: key.to_string(),
        content_length: Some(i64::try_from(body.len()).unwrap_or(i64::MAX)),
        body,
        acl: field("acl"),
        cache_control: field("Cache-Control"),
        content_disposition: field("Content-Disposition"),
        content_encoding: field("Content-Encoding"),
        content_type,
        expires: field("Expires"),
        storage_class: field("x-amz-storage-class"),
        website_redirect_location: field("x-amz-website-redirect-location"),
        server_side_encryption: field("x-amz-server-side-encryption"),
        ssekms_key_id: field("x-amz-server-side-encryption-aws-kms-key-id"),
        ssekms_encryption_context: field("x-amz-server-side-encryption-context"),
        bucket_key_enabled: form
            .value("x-amz-server-side-encryption-bucket-key-enabled")
            .map(|value| value.eq_ignore_ascii_case("true")),
        // "The specified set of tags to add to the object. Must be in XML format"
        // (*POST Object*'s `tagging` field), where `PutObject` takes the same set as the
        // query-string `x-amz-tagging` header. Both forms are read into the one member
        // that write already stores a tag set from.
        tagging: field("x-amz-tagging").or_else(|| form.value("tagging").and_then(tag_set_query)),
        object_lock_mode: field("x-amz-object-lock-mode"),
        object_lock_legal_hold_status: field("x-amz-object-lock-legal-hold"),
        metadata: (!metadata.is_empty()).then_some(metadata),
        ..PutObjectInput::default()
    }
}

/// The `Tagging` document `tagging` carries, as the query-string tag set `PutObject` takes
/// — `None` for text that is not such a document.
///
/// "The tag-set must be encoded as URL Query parameters" (`PutObjectRequest$Tagging`) is
/// the one form this service stores a tag set from
/// ([`crate::state::tag_set_from_header`]), so the XML form a browser form states is
/// translated into it rather than parsed a second way. The document is read with the XML
/// reader this service already reads documents with, and each key and value is encoded the
/// way that query grammar spells one.
fn tag_set_query(document: &str) -> Option<String> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(document);
    let mut key: Option<String> = None;
    let mut pairs: Vec<String> = Vec::new();
    let mut pair = |key: &mut Option<String>, value: String| {
        if let Some(key) = key.take() {
            pairs.push(format!("{}={}", query_encoded(&key), query_encoded(&value)));
        }
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if element != TAG_KEY_ELEMENT && element != TAG_VALUE_ELEMENT {
                    continue;
                }
                // The element's WHOLE text, read through the reader's own reading of it.
                let end = start.to_end().into_owned();
                let Ok(content) = reader.read_text(end.name()) else {
                    return None;
                };
                // The content is the element's markup as written, so the character
                // references a value carries (`a&amp;b`) are resolved by the XML reader's
                // own unescaping of them rather than by anything hand-rolled here.
                let Ok(written) = content.xml10_content() else {
                    return None;
                };
                let Ok(text) = ::quick_xml::escape::unescape(&written) else {
                    return None;
                };
                match element.as_str() {
                    TAG_KEY_ELEMENT => key = Some(text.into_owned()),
                    _ => pair(&mut key, text.into_owned()),
                }
            }
            Ok(Event::Empty(empty)) => {
                // `<Value/>` states the empty value, which is a value a tag may carry.
                if String::from_utf8_lossy(empty.name().as_ref()) == TAG_VALUE_ELEMENT {
                    pair(&mut key, String::new());
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            // Every other event — the declaration, a comment, the containers the two
            // elements above sit in — says nothing about which tags the set holds.
            Ok(_) => {}
        }
    }
    (!pairs.is_empty()).then(|| pairs.join("&"))
}

/// The element one tag's key travels in inside a `Tagging` document, and the one its value
/// travels in (`smithy_query(query="shape",shape="Tag")` names both members).
const TAG_KEY_ELEMENT: &str = "Key";
/// See [`TAG_KEY_ELEMENT`].
const TAG_VALUE_ELEMENT: &str = "Value";

/// The characters a query-string value is NOT escaped over: RFC 3986's unreserved set.
const QUERY_UNRESERVED: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// `value` as the `application/x-www-form-urlencoded` grammar spells it.
fn query_encoded(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, QUERY_UNRESERVED).to_string()
}

/// The characters the key in a URL's PATH is not escaped over: the unreserved set plus the
/// `/` of a key prefix, which a URL carries as the path separator it is.
const PATH_UNRESERVED: &::percent_encoding::AsciiSet = &QUERY_UNRESERVED.remove(b'/');

// ---------------------------------------------------------------------------
// What a served upload is answered with
// ---------------------------------------------------------------------------

/// The statuses `success_action_status` may name: "if the value is not set or if it is set
/// to 200 or 204 … if the value is set to 201, Amazon S3 returns an XML document"
/// (*POST Object*). Anything else is not one of them and answers the default below.
const SUCCESS_ACTION_STATUSES: [u16; 3] = [200, 201, 204];

/// The status an upload is answered with when the form names none, or names one that is
/// not among [`SUCCESS_ACTION_STATUSES`]: "S3 returns a 204 by default".
const DEFAULT_SUCCESS_STATUS: u16 = 204;

/// The status that answers with the `PostResponse` document rather than an empty one.
const CREATED_STATUS: u16 = 201;

/// The status a redirect of the browser is answered with, and the header carrying where
/// to.
const REDIRECT_STATUS: u16 = 303;
/// See [`REDIRECT_STATUS`].
const LOCATION_HEADER: &str = "location";

/// The header the stored object's ETag travels in, as every object write reports it.
const ETAG_HEADER: &str = "etag";

/// The media type of the one document this upload answers with.
const XML_MEDIA_TYPE: &str = "application/xml";

/// The two headers every S3 response is correlated by.
const REQUEST_ID_HEADER: &str = "x-amz-request-id";
/// See [`REQUEST_ID_HEADER`].
const EXTENDED_REQUEST_ID_HEADER: &str = "x-amz-id-2";

/// The answer a served upload is given: the redirect the form asked for, or the status it
/// asked for with the document that status carries.
fn served(form: &Form, location: &str, bucket: &str, key: &str, etag: &str) -> RtResponse {
    if let Some(redirect) = form
        .either(SUCCESS_ACTION_REDIRECT_FIELD, REDIRECT_FIELD)
        .and_then(|url| redirect_to(url, bucket, key, etag))
    {
        let mut response = response_with(REDIRECT_STATUS, SdkBody::empty());
        insert_header(&mut response, LOCATION_HEADER, &redirect);
        return with_object_identity(response, etag);
    }
    let status = form
        .value(SUCCESS_ACTION_STATUS_FIELD)
        .and_then(|stated| stated.trim().parse::<u16>().ok())
        .filter(|status| SUCCESS_ACTION_STATUSES.contains(status))
        .unwrap_or(DEFAULT_SUCCESS_STATUS);
    if status != CREATED_STATUS {
        return with_object_identity(response_with(status, SdkBody::empty()), etag);
    }
    let document = post_response_document(location, bucket, key, etag);
    let mut response = response_with(status, SdkBody::from(document.clone()));
    insert_header(&mut response, "content-type", XML_MEDIA_TYPE);
    insert_header(&mut response, "content-length", &document.len().to_string());
    with_object_identity(response, etag)
}

/// The `PostResponse` document a `201` answers with: "the `Location`, `Bucket`, `Key` and
/// `ETag` of the object" (*POST Object*'s own response description), each value escaped the
/// way this service's XML documents escape one.
fn post_response_document(location: &str, bucket: &str, key: &str, etag: &str) -> String {
    let escaped = |value: &str| ::quick_xml::escape::escape(value).into_owned();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <PostResponse>\
         <Location>{}</Location>\
         <Bucket>{}</Bucket>\
         <Key>{}</Key>\
         <ETag>{}</ETag>\
         </PostResponse>",
        escaped(location),
        escaped(bucket),
        escaped(key),
        escaped(etag)
    )
}

/// Where the browser is redirected to, or `None` when the form's URL is not one: "if the
/// value of `success_action_redirect` is not valid, Amazon S3 ignores the field"
/// (*POST Object*), and S3 "adds the bucket, key, and ETag values as query string
/// parameters" to the URL it does redirect to.
fn redirect_to(url: &str, bucket: &str, key: &str, etag: &str) -> Option<String> {
    let url = url.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    Some(format!(
        "{url}{separator}bucket={}&key={}&etag={}",
        query_encoded(bucket),
        query_encoded(key),
        query_encoded(etag)
    ))
}

/// One response of this upload, with the correlation pair every S3 response carries: the
/// pair the write recorded ([`crate::state::request_ids`]), so the response and the event
/// the upload published name the same request.
fn response_with(status: u16, body: SdkBody) -> RtResponse {
    let status = StatusCode::try_from(status).unwrap_or_else(|_| {
        StatusCode::try_from(DEFAULT_SUCCESS_STATUS).expect("204 is a status")
    });
    let mut response = RtResponse::new(status, body);
    let correlation = crate::state::request_ids::answering()
        .unwrap_or_else(crate::state::request_ids::RequestCorrelation::mint);
    insert_header(&mut response, REQUEST_ID_HEADER, &correlation.request_id);
    insert_header(
        &mut response,
        EXTENDED_REQUEST_ID_HEADER,
        &correlation.extended_request_id,
    );
    response
}

/// `response` carrying the stored object's ETag, which every object write reports in that
/// header.
fn with_object_identity(mut response: RtResponse, etag: &str) -> RtResponse {
    if !etag.is_empty() {
        insert_header(&mut response, ETAG_HEADER, etag);
    }
    response
}

/// Set one response header, dropping a value the wire cannot carry rather than turning a
/// served upload into a transport failure.
fn insert_header(response: &mut RtResponse, name: &'static str, value: &str) {
    if let Err(refused) = response.headers_mut().try_insert(name, value.to_string()) {
        ::tracing::warn!(
            header = name,
            error = %refused,
            "this header cannot be sent on the wire and is left off the upload's response"
        );
    }
}

/// The error document a refused upload is answered with, rendered through the SAME protocol
/// every other answer of this service is rendered through — so a refusal here carries the
/// envelope, the correlation elements and the headers S3's own error document carries.
///
/// A refusal naming NO code is the one S3 answers with no error document at all
/// ([`crate::service::object_write::ERRORLESS_REFUSAL_CODE`] — a key carrying a NUL byte),
/// so it is sent as the bare status it is rather than as an envelope with an empty `<Code>`.
fn refused(svc: &dyn Service, refusal: &Refusal) -> RtResponse {
    if refusal.code.is_empty() {
        return response_with(refusal.status, SdkBody::empty());
    }
    let protocol = svc
        .override_protocol()
        .unwrap_or_else(|| svc.definition().protocol());
    let document = crate::scaffolded::UnmodeledMessage {
        message: refusal.message.clone(),
    };
    protocol
        .serialize_error(
            &document,
            &crate::scaffolded::UNMODELED_SCHEMA,
            refusal.code,
            refusal.status,
        )
        .unwrap_or_else(|error| {
            ::tracing::warn!(
                code = refusal.code,
                %error,
                "this refusal of a form upload could not be rendered in this service's \
                 envelope"
            );
            response_with(refusal.status, SdkBody::empty())
        })
}

/// The refusal a write this service already decided answers with, rendered through that
/// same protocol: the error channel of the object write carries its own code, status and
/// members, so a form upload refused by the write is answered exactly as the `PUT` of the
/// same object would be.
fn refused_write(svc: &dyn Service, error: &crate::ops::put_object::PutObjectError) -> RtResponse {
    let protocol = svc
        .override_protocol()
        .unwrap_or_else(|| svc.definition().protocol());
    protocol
        .serialize_error(
            error.as_serializable(),
            error.variant_schema(),
            error.shape_name(),
            error.http_status(),
        )
        .unwrap_or_else(|failure| {
            ::tracing::warn!(
                code = error.shape_name(),
                %failure,
                "this refusal of the object write a form upload performs could not be \
                 rendered in this service's envelope"
            );
            response_with(error.http_status(), SdkBody::empty())
        })
}

// ---------------------------------------------------------------------------
// The operation a form upload is served as
// ---------------------------------------------------------------------------

/// The operation this service serves a browser-form upload with.
///
/// It is an OPERATION rather than a reworded routing refusal, because serving the upload
/// needs the two things only an operation is given: the shared engine the object is stored
/// in, and the request's own callback into this emulator, over which the event the write
/// publishes reaches its destinations.
pub struct PostObject<Eng> {
    /// The SAME shared engine every operation of this service holds: an object uploaded
    /// over a form is the object `GetObject` then serves.
    engine: Arc<Eng>,
}

impl<Eng> PostObject<Eng> {
    /// Build the upload's operation from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> PostObject<Eng> {
    /// The bucket this request addresses, in either address form: the first label of the
    /// host when that names a bucket this emulator holds, else the first segment of the
    /// path — the one reading this service resolves every address with
    /// ([`super::addressed_in_host`]).
    fn addressed_bucket(&self, req: &RtRequest) -> Option<String> {
        let path = super::wire::request_path(req.uri());
        let holds = |name: &str| {
            crate::state::service_state(&*self.engine)
                .is_some_and(|state| state.lock().bucket(name).is_some())
        };
        let in_host = super::request_host_of(req)
            .and_then(|host| super::addressed_in_host(host, path, &holds));
        match in_host {
            Some(bucket) => Some(bucket.to_string()),
            None => super::first_path_segment(path).map(str::to_string),
        }
    }

    /// Serve one form upload: read the form, judge the policy it carries, perform the
    /// object write and answer it the way the form asked to be answered.
    async fn serve(
        &self,
        svc: &dyn Service,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> RtResponse {
        // The correlation slot this thread holds is cleared before the write records its
        // own pair, exactly as the framework's own walk clears it as it deserializes a
        // request — so this response is named by its own request's pair or by a fresh one.
        crate::state::request_ids::forget_any_unanswered();
        let content_type = req.headers().get("content-type").unwrap_or_default();
        let form = match read_form(content_type, req.body().bytes().unwrap_or_default()).await {
            Ok(form) => form,
            Err(refusal) => return refused(svc, &refusal),
        };
        let Some(bucket) = self.addressed_bucket(req) else {
            // Routing reached this operation from an address naming a bucket, so there is
            // none to serve without one; the published refusal of a form naming no key is
            // the nearest thing a caller can act on.
            return refused(
                svc,
                &Refusal::new(
                    INVALID_ARGUMENT_CODE,
                    BAD_REQUEST_STATUS,
                    KEY_FIELD_MISSING_MESSAGE,
                ),
            );
        };
        match self.upload(svc, &form, &bucket, ctx).await {
            Ok(response) => response,
            Err(refusal) => refused(svc, &refusal),
        }
    }

    /// The upload itself, or the refusal the FORM earns before anything is stored. A
    /// refusal the WRITE makes is already a rendered response, which is why that half is
    /// answered rather than returned.
    async fn upload(
        &self,
        svc: &dyn Service,
        form: &Form,
        bucket: &str,
        ctx: &OperationContext<'_>,
    ) -> Result<RtResponse, Refusal> {
        let Some((file_name, _, file_bytes)) = form.file.as_ref() else {
            return Err(Refusal::new(
                FILE_COUNT_CODE,
                BAD_REQUEST_STATUS,
                FILE_COUNT_MESSAGE,
            ));
        };
        let Some(stated_key) = form.value(KEY_FIELD) else {
            return Err(Refusal::new(
                INVALID_ARGUMENT_CODE,
                BAD_REQUEST_STATUS,
                KEY_FIELD_MISSING_MESSAGE,
            ));
        };
        // The key the form states and the name the browser stated for its file are both
        // arbitrary request text, and filling the placeholder is the PRODUCT of the two, so
        // each is judged against S3's own published key ceiling before the expansion
        // allocates anything: "the name for a key is a sequence of Unicode characters whose
        // UTF-8 encoding is at most 1,024 bytes long"
        // ([`crate::service::object_write::key_refusal`] holds that rule, and is the same
        // one the write itself judges the finished key by).
        for named in [stated_key, file_name.as_str()] {
            if let Some((code, message)) = crate::service::object_write::key_refusal(named) {
                return Err(Refusal::new(
                    code,
                    crate::service::object_write::KEY_REFUSAL_STATUS,
                    message,
                ));
            }
        }
        let key = stated_key.replace(FILENAME_PLACEHOLDER, file_name);
        if let Some(encoded) = form.value(POLICY_FIELD) {
            let policy = read_policy(encoded)?;
            if let Some(refusal) = policy_rejection(
                &policy,
                form,
                bucket,
                file_bytes.len(),
                SystemTime::now(),
            ) {
                return Err(refusal);
            }
            match signature_matches(form, encoded) {
                Some(true) => {}
                Some(false) => {
                    return Err(Refusal::new(
                        SIGNATURE_MISMATCH_CODE,
                        crate::state::ACCESS_DENIED_STATUS,
                        SIGNATURE_MISMATCH_MESSAGE,
                    ))
                }
                None => ::tracing::debug!(
                    bucket = %bucket,
                    "this form upload names a credential whose secret this process does \
                     not hold, so its policy signature is not verified here"
                ),
            }
        } else {
            // A form carrying no policy at all presents nothing that authorizes the write:
            // what it may store is decided solely by what the bucket grants, which is the
            // authorization the object write below makes. The authentication fields are
            // the policy's signature, so without a policy there is nothing for them to
            // sign and they decide nothing either.
            ::tracing::debug!(
                bucket = %bucket,
                "this form upload carries no policy; what it may store is left to the \
                 bucket's own authorization"
            );
        }
        let input = object_write(form, bucket, &key, file_bytes.clone());
        let write = crate::ops::put_object::PutObject::new(Arc::clone(&self.engine));
        // The object write EVERY call of this service's object plane goes through, told
        // which call it is serving: the event this one publishes is an
        // `s3:ObjectCreated:Post` whose reason is the POST of the form.
        match write
            .write(input, ctx, ObjectEventKind::CreatedByPost)
            .await
        {
            Ok(output) => Ok(self.answered(form, bucket, &key, &output, ctx)),
            Err(error) => Ok(refused_write(svc, &error)),
        }
    }

    /// The answer the stored object earns, with the `Location` it is reachable at: the
    /// path-style address of the key on the endpoint serving this request
    /// ([`crate::service::announced_url::announced_bucket_url`]).
    fn answered(
        &self,
        form: &Form,
        bucket: &str,
        key: &str,
        output: &PutObjectOutput,
        ctx: &OperationContext<'_>,
    ) -> RtResponse {
        let encoded_key = ::percent_encoding::utf8_percent_encode(key, PATH_UNRESERVED);
        let location = crate::service::announced_url::announced_bucket_url(
            ctx,
            bucket,
            &encoded_key.to_string(),
        );
        served(
            form,
            &location,
            bucket,
            key,
            output.e_tag.as_deref().unwrap_or_default(),
        )
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for PostObject<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(svc, req, ctx).await })
    }

    /// Served with NO SigV4 scheme, because a browser form carries no `Authorization`
    /// header: "the POST operation adds an object to a specified bucket by using HTML
    /// forms", whose credential is the `policy` and its `x-amz-signature` field. There is
    /// no header signature for the framework to verify, and nothing is waved through —
    /// [`PostObject::upload`] enforces the policy and verifies its signature itself.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A form carrying exactly `fields` and a file of `bytes`.
    fn form(fields: &[(&str, &str)], bytes: &[u8]) -> Form {
        Form {
            fields: fields
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            file: Some((
                "attached.txt".to_string(),
                Some("text/plain".to_string()),
                bytes.to_vec(),
            )),
        }
    }

    /// A policy document carrying `conditions`, and `expiration` when the row states one.
    fn policy(
        expiration: Option<&str>,
        conditions: ::serde_json::Value,
    ) -> ::serde_json::Map<String, ::serde_json::Value> {
        let mut document = ::serde_json::Map::new();
        if let Some(expiration) = expiration {
            document.insert(
                POLICY_EXPIRATION.to_string(),
                ::serde_json::Value::String(expiration.to_string()),
            );
        }
        document.insert(POLICY_CONDITIONS.to_string(), conditions);
        document
    }

    /// An expiration far enough ahead that no run of this test reaches it.
    const NOT_EXPIRED: Option<&str> = Some("2999-01-01T00:00:00Z");

    /// One row of the policy table: the case, the conditions the policy states, the
    /// expiration it states (when any), the fields the form submitted, the size of the
    /// file, and the `<Code>` the refusal must carry (`None` for a form the policy allows).
    type PolicyRow = (
        &'static str,
        ::serde_json::Value,
        Option<&'static str>,
        Vec<(&'static str, &'static str)>,
        usize,
        Option<&'static str>,
    );

    /// RULE: a policy allows a form only while it is unexpired and every condition it
    /// carries holds for what the form submitted; a document that is not a well-formed
    /// policy is refused as one. The rows differ along what the rule generalises over: the
    /// matcher (`eq`, `starts-with`, the map form, `content-length-range`), which field is
    /// named, whether the submitted value satisfies it, the size of the file, and the
    /// shapes of expiration and of range the grammar does and does not admit.
    #[test]
    fn a_policy_allows_only_the_forms_its_conditions_state() {
        let rows: Vec<PolicyRow> = vec![
            (
                "the map form satisfied",
                ::serde_json::json!([{ "acl": "private" }, { "key": "uploads/report.csv" }]),
                NOT_EXPIRED,
                vec![("acl", "private"), ("key", "uploads/report.csv")],
                9,
                None,
            ),
            (
                "the map form unsatisfied",
                ::serde_json::json!([{ "acl": "private" }]),
                NOT_EXPIRED,
                vec![("acl", "public-read")],
                9,
                Some("AccessDenied"),
            ),
            (
                "an eq condition satisfied",
                ::serde_json::json!([["eq", "$key", "uploads/report.csv"]]),
                NOT_EXPIRED,
                vec![("key", "uploads/report.csv")],
                9,
                None,
            ),
            (
                "an eq condition the submitted value only starts with",
                ::serde_json::json!([["eq", "$key", "uploads/"]]),
                NOT_EXPIRED,
                vec![("key", "uploads/report.csv")],
                9,
                Some("AccessDenied"),
            ),
            (
                "a starts-with condition satisfied",
                ::serde_json::json!([["starts-with", "$key", "uploads/"]]),
                NOT_EXPIRED,
                vec![("key", "uploads/report.csv")],
                9,
                None,
            ),
            (
                "a starts-with condition on a field the form did not submit",
                ::serde_json::json!([["starts-with", "$x-amz-meta-owner", ""]]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                Some("AccessDenied"),
            ),
            (
                "the bucket condition, which names the addressed bucket",
                ::serde_json::json!([{ "bucket": "uploads-bucket" }]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                None,
            ),
            (
                "a bucket condition naming another bucket",
                ::serde_json::json!([{ "bucket": "another-bucket" }]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                Some("AccessDenied"),
            ),
            (
                "a file inside the content-length-range",
                ::serde_json::json!([["content-length-range", 1, 16]]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                None,
            ),
            (
                "a file below the range's minimum",
                ::serde_json::json!([["content-length-range", 16, 32]]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                Some("EntityTooSmall"),
            ),
            (
                "a file above the range's maximum",
                ::serde_json::json!([["content-length-range", 0, 4]]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                Some("EntityTooLarge"),
            ),
            (
                "a file exactly on the range's maximum",
                ::serde_json::json!([["content-length-range", 0, 9]]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                None,
            ),
            (
                "a content-length-range stating one bound",
                ::serde_json::json!([["content-length-range", 16]]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                Some("InvalidPolicyDocument"),
            ),
            (
                "a content-length-range whose bounds are not sizes",
                ::serde_json::json!([["content-length-range", "0", "16"]]),
                NOT_EXPIRED,
                vec![("key", "k")],
                9,
                Some("InvalidPolicyDocument"),
            ),
            (
                "an expiration that has passed",
                ::serde_json::json!([]),
                Some("2000-01-01T00:00:00Z"),
                vec![("key", "k")],
                9,
                Some("AccessDenied"),
            ),
            (
                "an expiration stated with a numeric zone offset",
                ::serde_json::json!([]),
                Some("2999-01-01T00:00:00+02:00"),
                vec![("key", "k")],
                9,
                None,
            ),
            (
                "an expiration that names no instant",
                ::serde_json::json!([]),
                Some("whenever"),
                vec![("key", "k")],
                9,
                Some("InvalidPolicyDocument"),
            ),
            (
                "a document stating no expiration at all",
                ::serde_json::json!([]),
                None,
                vec![("key", "k")],
                9,
                Some("InvalidPolicyDocument"),
            ),
        ];

        for (case, conditions, expiration, fields, size, expected) in rows {
            let document = policy(expiration, conditions);
            let submitted = form(&fields, &vec![b'x'; size]);
            let rejection = policy_rejection(
                &document,
                &submitted,
                "uploads-bucket",
                size,
                SystemTime::now(),
            );
            let code = rejection.as_ref().map(|refusal| refusal.code);
            assert_eq!(
                code, expected,
                "case {case:?}: expected {expected:?}, got {rejection:?}"
            );
        }
    }

    /// One row of the tagging table: the case, the document read, and the tag set it names.
    type TaggingRow = (&'static str, &'static str, Option<&'static str>);

    /// RULE: a `Tagging` document the form states is read into the query-string tag set the
    /// object write stores. The rows differ along what the rule generalises over: the tag
    /// count, an empty value, characters the query grammar escapes, and text that is no
    /// document at all.
    #[test]
    fn a_tagging_document_is_read_as_the_query_string_tag_set() {
        let rows: [TaggingRow; 5] = [
            (
                "one tag",
                "<Tagging><TagSet><Tag><Key>team</Key><Value>ops</Value></Tag></TagSet></Tagging>",
                Some("team=ops"),
            ),
            (
                "two tags",
                "<Tagging><TagSet><Tag><Key>team</Key><Value>ops</Value></Tag>\
                 <Tag><Key>tier</Key><Value>gold</Value></Tag></TagSet></Tagging>",
                Some("team=ops&tier=gold"),
            ),
            (
                "a tag whose value is empty",
                "<Tagging><TagSet><Tag><Key>team</Key><Value/></Tag></TagSet></Tagging>",
                Some("team="),
            ),
            (
                "a value the query grammar escapes",
                "<Tagging><TagSet><Tag><Key>owner</Key><Value>a b&amp;c</Value></Tag>\
                 </TagSet></Tagging>",
                Some("owner=a%20b%26c"),
            ),
            ("text that is no document", "not xml at all", None),
        ];

        for (case, document, expected) in rows {
            let read = tag_set_query(document);
            assert_eq!(
                read.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// One row of the status table: the case, the status the form named (when any), the
    /// status it must be answered with, and whether that answer carries a document.
    type StatusRow = (&'static str, Option<&'static str>, u16, bool);

    /// RULE: the status a served upload answers with is the one the form named, when that
    /// is one of the three the service publishes, and `204` otherwise; a `201` is the one
    /// that carries the `PostResponse` document. The rows differ along the stated value,
    /// including each published status and one past them.
    #[test]
    fn the_answered_status_is_the_one_the_form_named() {
        let rows: [StatusRow; 6] = [
            ("no status named", None, 204, false),
            ("the empty 200", Some("200"), 200, false),
            ("the created 201", Some("201"), 201, true),
            ("the empty 204", Some("204"), 204, false),
            ("a status outside the published three", Some("302"), 204, false),
            ("a value that is no status at all", Some("yes"), 204, false),
        ];

        for (case, stated, expected, carries_document) in rows {
            let fields: Vec<(&str, &str)> = stated
                .map(|value| vec![(SUCCESS_ACTION_STATUS_FIELD, value)])
                .unwrap_or_default();
            let answer = served(
                &form(&fields, b"payload"),
                "https://s3.us-east-1.amazonaws.com/uploads-bucket/report.csv",
                "uploads-bucket",
                "report.csv",
                "\"d41d8cd98f00b204e9800998ecf8427e\"",
            );
            assert_eq!(
                answer.status().as_u16(),
                expected,
                "case {case:?}: expected HTTP {expected}, got {}",
                answer.status().as_u16()
            );
            let body = answer.body().bytes().unwrap_or_default();
            assert_eq!(
                !body.is_empty(),
                carries_document,
                "case {case:?}: expected a document {carries_document:?}, got {:?}",
                String::from_utf8_lossy(body)
            );
        }
    }

    /// One row of the redirect table: the case, the URL the form named, and where the
    /// browser must be sent (`None` for a value that is no URL).
    type RedirectRow = (&'static str, &'static str, Option<&'static str>);

    /// RULE: a valid redirect URL is answered `303` with the bucket, the key and the ETag
    /// added to it; anything else is not a URL and the status field answers instead. The
    /// rows differ along the scheme, whether the URL already carries a query, and the two
    /// shapes of invalid value.
    #[test]
    fn a_valid_redirect_url_is_answered_with_a_redirect() {
        let rows: [RedirectRow; 5] = [
            (
                "an https URL",
                "https://example.test/done",
                Some("https://example.test/done?bucket=uploads-bucket&key=report.csv&etag=%22abc%22"),
            ),
            (
                "an http URL",
                "http://example.test/done",
                Some("http://example.test/done?bucket=uploads-bucket&key=report.csv&etag=%22abc%22"),
            ),
            (
                "a URL already carrying a query",
                "https://example.test/done?from=form",
                Some(
                    "https://example.test/done?from=form&bucket=uploads-bucket&key=report.csv\
                     &etag=%22abc%22",
                ),
            ),
            ("a relative path", "/done", None),
            ("text that is no URL", "done", None),
        ];

        for (case, url, expected) in rows {
            let redirect = redirect_to(url, "uploads-bucket", "report.csv", "\"abc\"");
            assert_eq!(
                redirect.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {redirect:?}"
            );
        }
    }

    /// One row of the signature table: the case, the credential the form names, the
    /// algorithm it states (when any), the signature it carries, and the verdict.
    type SignatureRow = (
        &'static str,
        String,
        Option<&'static str>,
        String,
        Option<bool>,
    );

    /// RULE: the signature a form carries is the HMAC-SHA256 of the base64 policy under the
    /// signing key SigV4 derives from the credential the form named — judged for a
    /// credential whose secret this process holds, and not judged at all for one it does
    /// not.
    ///
    /// The rows differ along what the rule generalises over: whether the signature is the
    /// derived one, whether the credential names the key this process published, whether
    /// the form states the algorithm the derivation is of, and whether its credential is a
    /// SigV4 scope at all.
    #[test]
    fn the_policy_signature_is_verified_against_the_credential_the_form_named() {
        let published =
            ::emulator_server::request_authentication::SigV4Authenticator::for_this_process();
        // A policy document, encoded the way a form carries it; what it CONTAINS does not
        // enter the signature, only its bytes do.
        let encoded = "eyJjb25kaXRpb25zIjpbXX0=";
        let scope = format!(
            "{}/20260101/eu-west-1/s3/aws4_request",
            published.access_key_id
        );
        // The derivation, stage by stage, as the published form of it states it.
        let mut signing_key = hmac(
            format!("AWS4{}", published.secret_access_key).as_bytes(),
            b"20260101",
        )
        .expect("the derivation takes this key");
        for stage in [
            b"eu-west-1".as_slice(),
            b"s3".as_slice(),
            b"aws4_request".as_slice(),
        ] {
            signing_key = hmac(&signing_key, stage).expect("the derivation takes this key");
        }
        let signature = ::hex::encode(
            hmac(&signing_key, encoded.as_bytes()).expect("the derivation takes this key"),
        );
        let tampered = format!("{}0", &signature[1..]);

        let rows: Vec<SignatureRow> = vec![
            (
                "the derived signature",
                scope.clone(),
                Some(SIGV4_ALGORITHM),
                signature.clone(),
                Some(true),
            ),
            (
                "a tampered signature",
                scope.clone(),
                Some(SIGV4_ALGORITHM),
                tampered,
                Some(false),
            ),
            (
                "a credential this process published no secret for",
                "AKIAEXAMPLE/20260101/eu-west-1/s3/aws4_request".to_string(),
                Some(SIGV4_ALGORITHM),
                signature.clone(),
                None,
            ),
            (
                "a form stating no algorithm",
                scope.clone(),
                None,
                signature.clone(),
                None,
            ),
            (
                "a form stating another algorithm",
                scope,
                Some("AWS4-HMAC-SHA1"),
                signature.clone(),
                None,
            ),
            (
                "a credential that is no SigV4 scope",
                published.access_key_id.clone(),
                Some(SIGV4_ALGORITHM),
                signature,
                None,
            ),
        ];

        for (case, credential, algorithm, signature, expected) in rows {
            let mut fields = vec![
                (CREDENTIAL_FIELD, credential.as_str()),
                (SIGNATURE_FIELD, signature.as_str()),
            ];
            if let Some(algorithm) = algorithm {
                fields.push((ALGORITHM_FIELD, algorithm));
            }
            let verdict = signature_matches(&form(&fields, b"payload"), encoded);
            assert_eq!(
                verdict, expected,
                "case {case:?}: expected {expected:?}, got {verdict:?}"
            );
        }
    }
}
