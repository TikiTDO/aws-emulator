// SPDX-License-Identifier: CC0-1.0

//! The CloudFront half of the SDK seam a RUNNING handler calls another service through: the
//! INVALIDATION of a distribution's cache, built the way CloudFront's own protocol binds it.
//!
//! A function that writes an object and then refreshes the distribution in front of it is
//! one of the shapes AWS documents a handler for: the write lands in the origin and the
//! handler calls `CreateInvalidation` so the edge stops serving what it holds. That call is
//! only reachable from a handler if this seam binds it.
//!
//! The other services a handler reaches bind their requests with an AWS JSON protocol,
//! where the request body IS the operation's input document, so one code path serves every
//! operation of every one of them ([`super::aws_api`]). CloudFront does not: its model is
//! `restXml`, so each operation binds its input to a METHOD, a PATH and an XML DOCUMENT of
//! its own, and answers XML. That binding is not written out here: the request is built,
//! signed, dispatched and parsed by the CloudFront CLIENT this crate holds
//! (`aws_sdk_cloudfront`), over the same in-process callback transport every other call of
//! this seam uses, so the wire details are the client's and the answer is the emulated
//! CloudFront service's own.
//!
//! What a guest can and cannot do here:
//!
//! * The destination is ALWAYS this emulator: the client is given the callback's own
//!   transport and CloudFront's own endpoint, so no host name is resolved and no socket is
//!   opened.
//! * The call is signed as the identity the invocation was given — the session of the
//!   function's execution role ([`super::execution_role_session`]) — so CloudFront judges it
//!   against the policies in force on that role exactly as it judges any other signed
//!   request.
//! * Only the operations in [`BOUND_OPERATIONS`] are bound, and only the input members each
//!   entry names. An operation or a member this seam does not bind is reported as a call that
//!   was NOT MADE, naming it, rather than sent with a value dropped — a dropped member would
//!   make a call that did something other than what the handler asked for.
//! * The answer's own status and document are what CloudFront answered: a success is that
//!   operation's output members, and a refusal is CloudFront's own error code, message and
//!   status, which the SDK the guest calls raises as `ClientError`
//!   ([`super::aws_sdk_guest`]).

use ::std::sync::{Arc, Mutex, PoisonError};

use ::aws_sdk_cloudfront::error::ProvideErrorMetadata;
use ::aws_sdk_cloudfront::Client;
use ::serde_json::{Map, Value};
use ::smithy_server_runtime::EmbeddedEmulator;

use super::aws_api::{Answer, CallerIdentity, Refusal, MAX_RESPONSE_BYTES};

/// The provider name the signing identity carries: it names this runtime rather than any
/// AWS credential source, and never travels on the wire.
const PROVIDER_NAME: &str = "aws-lambda-execution-role";

/// The operations of CloudFront this seam binds, in the order a refusal lists them.
///
/// One entry per operation [`call`] builds a request for: the invalidation surface, which is
/// what a handler refreshing the cache in front of an origin it just wrote calls. A handler
/// naming any other operation of the service is told the call was not made and why, because
/// a request this module cannot bind is one it would otherwise be guessing at.
pub const BOUND_OPERATIONS: &[&str] = &[
    "CreateInvalidation",
    "GetInvalidation",
    "ListInvalidations",
];

/// The Region a CloudFront call is signed for, whatever Region the client the guest built
/// names: CloudFront is a global service reached at one endpoint, and its credential scope
/// names the Region that endpoint is served from
/// (`service_common::management_events::PARTITION_LEADER_REGION`).
const SIGNING_REGION: &str = ::service_common::management_events::PARTITION_LEADER_REGION;

/// The endpoint every CloudFront call is made at, which is the host the emulated CloudFront
/// service claims its control plane on.
const ENDPOINT: &str = "https://cloudfront.amazonaws.com";

/// Make ONE CloudFront call: `operation`, with `body` as the operation's input document (the
/// keyword arguments the handler passed its client method).
pub async fn call(
    callback: &EmbeddedEmulator,
    identity: Option<&CallerIdentity>,
    operation: &str,
    body: &str,
) -> Result<Answer, Refusal> {
    let input = Input::read(body)?;
    let observed = Observed::default();
    let client = client(callback, identity, observed.clone());
    match operation {
        "CreateInvalidation" => create_invalidation(&client, &input, &observed).await,
        "GetInvalidation" => get_invalidation(&client, &input, &observed).await,
        "ListInvalidations" => list_invalidations(&client, &input, &observed).await,
        other => Err(Refusal::NotMade(format!(
            "the AWS SDK this runtime supplies binds no {other} call on CloudFront; the \
             operations it binds are {}",
            BOUND_OPERATIONS.join(", ")
        ))),
    }
}

/// A CloudFront client over the in-process callback into this same emulator, signing as the
/// identity this invocation was given.
///
/// The transport is in-process, so everything a network client needs and this one cannot
/// have — retries, timeouts, identity caching and stalled-stream protection, each of which
/// needs a sleep implementation the callback transport is not given — is switched off.
fn client(
    callback: &EmbeddedEmulator,
    identity: Option<&CallerIdentity>,
    observed: Observed,
) -> Client {
    let config = ::aws_sdk_cloudfront::Config::builder()
        .behavior_version(::aws_sdk_cloudfront::config::BehaviorVersion::latest())
        .region(::aws_sdk_cloudfront::config::Region::new(
            SIGNING_REGION.to_string(),
        ))
        .endpoint_url(ENDPOINT)
        .http_client(callback.http_client())
        .interceptor(observed)
        .retry_config(::aws_sdk_cloudfront::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudfront::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudfront::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudfront::config::StalledStreamProtectionConfig::disabled(),
        );
    let config = match identity {
        Some(identity) => config.credentials_provider(
            ::aws_sdk_cloudfront::config::Credentials::new(
                identity.access_key_id.clone(),
                identity.secret_access_key.clone(),
                Some(identity.session_token.clone()),
                None,
                PROVIDER_NAME,
            ),
        ),
        None => config.credentials_provider(callback.aws_credentials_provider()),
    };
    Client::from_conf(config.build())
}

/// `CreateInvalidation`: the batch of paths whose objects the edge is to stop serving.
async fn create_invalidation(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["DistributionId", "InvalidationBatch"])?;
    let batch = input.object("InvalidationBatch")?.ok_or_else(|| {
        Refusal::NotMade(
            "the CloudFront call carries no InvalidationBatch, which names the paths to \
             invalidate"
                .to_string(),
        )
    })?;
    let paths = batch.object("Paths")?.ok_or_else(|| {
        Refusal::NotMade(
            "the call's InvalidationBatch carries no Paths, which names the objects to \
             invalidate"
                .to_string(),
        )
    })?;
    let items = paths.texts("Items")?;
    // `Paths$Quantity` is "the number of invalidation paths specified for the objects that
    // you want to invalidate", so it is the number of items the call carries — the value an
    // SDK computes for the caller rather than one the handler states.
    let quantity = i32::try_from(items.len()).unwrap_or(i32::MAX);
    let batch_sent = ::aws_sdk_cloudfront::types::InvalidationBatch::builder()
        .paths(
            ::aws_sdk_cloudfront::types::Paths::builder()
                .quantity(quantity)
                .set_items(Some(items))
                .build()
                .map_err(|error| {
                    Refusal::NotMade(format!(
                        "the call's Paths is not one CloudFront accepts: {error}"
                    ))
                })?,
        )
        .set_caller_reference(batch.text("CallerReference")?)
        .build()
        .map_err(|error| {
            Refusal::NotMade(format!(
                "the call's InvalidationBatch is not one CloudFront accepts: {error}"
            ))
        })?;
    let sent = client
        .create_invalidation()
        .distribution_id(input.required_text("DistributionId")?)
        .invalidation_batch(batch_sent)
        .send()
        .await;
    let created = match sent {
        Ok(created) => created,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Location", created.location());
    if let Some(invalidation) = created.invalidation() {
        document.insert("Invalidation".to_string(), invalidation_document(invalidation));
    }
    observed.answer(document)
}

/// `GetInvalidation`: what one submitted invalidation reports now.
async fn get_invalidation(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["DistributionId", "Id"])?;
    let sent = client
        .get_invalidation()
        .distribution_id(input.required_text("DistributionId")?)
        .id(input.required_text("Id")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    if let Some(invalidation) = read.invalidation() {
        document.insert("Invalidation".to_string(), invalidation_document(invalidation));
    }
    observed.answer(document)
}

/// `ListInvalidations`: one page of the invalidations submitted for a distribution.
async fn list_invalidations(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["DistributionId", "Marker", "MaxItems"])?;
    let sent = client
        .list_invalidations()
        .distribution_id(input.required_text("DistributionId")?)
        .set_marker(input.text("Marker")?)
        .set_max_items(input.number("MaxItems")?.map(|items| {
            i32::try_from(items).unwrap_or(i32::MAX)
        }))
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    if let Some(list) = listed.invalidation_list() {
        let mut page = Map::new();
        insert_text(&mut page, "Marker", Some(list.marker()));
        insert_text(&mut page, "NextMarker", list.next_marker());
        insert_number(&mut page, "MaxItems", Some(i64::from(list.max_items())));
        insert_number(&mut page, "Quantity", Some(i64::from(list.quantity())));
        page.insert(
            "IsTruncated".to_string(),
            Value::Bool(list.is_truncated()),
        );
        let items: Vec<Value> = list
            .items()
            .iter()
            .map(|summary| {
                let mut entry = Map::new();
                entry.insert("Id".to_string(), Value::String(summary.id().to_string()));
                entry.insert(
                    "Status".to_string(),
                    Value::String(summary.status().to_string()),
                );
                insert_timestamp(&mut entry, "CreateTime", Some(summary.create_time()));
                Value::Object(entry)
            })
            .collect();
        if !items.is_empty() {
            page.insert("Items".to_string(), Value::Array(items));
        }
        document.insert("InvalidationList".to_string(), Value::Object(page));
    }
    observed.answer(document)
}

/// One `Invalidation` as the answer's document carries it: what the service reported about
/// the batch it holds.
fn invalidation_document(invalidation: &::aws_sdk_cloudfront::types::Invalidation) -> Value {
    let mut document = Map::new();
    document.insert(
        "Id".to_string(),
        Value::String(invalidation.id().to_string()),
    );
    document.insert(
        "Status".to_string(),
        Value::String(invalidation.status().to_string()),
    );
    insert_timestamp(&mut document, "CreateTime", Some(invalidation.create_time()));
    if let Some(batch) = invalidation.invalidation_batch() {
        let mut batch_document = Map::new();
        batch_document.insert(
            "CallerReference".to_string(),
            Value::String(batch.caller_reference().to_string()),
        );
        if let Some(paths) = batch.paths() {
            let mut paths_document = Map::new();
            paths_document.insert(
                "Quantity".to_string(),
                Value::Number(i64::from(paths.quantity()).into()),
            );
            let items: Vec<Value> = paths
                .items()
                .iter()
                .map(|path| Value::String(path.clone()))
                .collect();
            if !items.is_empty() {
                paths_document.insert("Items".to_string(), Value::Array(items));
            }
            batch_document.insert("Paths".to_string(), Value::Object(paths_document));
        }
        document.insert(
            "InvalidationBatch".to_string(),
            Value::Object(batch_document),
        );
    }
    Value::Object(document)
}

/// The input document one call carries: the keyword arguments the handler passed, read as
/// the operation's own input members.
struct Input {
    members: Map<String, Value>,
}

impl Input {
    /// The document `body` carries, or a call not made when it is not an object at all.
    fn read(body: &str) -> Result<Self, Refusal> {
        let parsed: Value = ::serde_json::from_str(body).map_err(|error| {
            Refusal::NotMade(format!("the call's parameters are not a document: {error}"))
        })?;
        match parsed {
            Value::Object(members) => Ok(Self { members }),
            _ => Err(Refusal::NotMade(
                "the call's parameters must be the operation's own input members".to_string(),
            )),
        }
    }

    /// Refuse to send a call carrying a member this binding does not carry, naming it: a
    /// member silently dropped would make a request other than the one asked for.
    fn only(&self, bound: &[&str]) -> Result<(), Refusal> {
        let mut unbound: Vec<&str> = self
            .members
            .keys()
            .map(String::as_str)
            .filter(|name| !bound.contains(name))
            .collect();
        unbound.sort_unstable();
        if unbound.is_empty() {
            return Ok(());
        }
        Err(Refusal::NotMade(format!(
            "the AWS SDK this runtime supplies does not carry the {} parameter(s) of this \
             CloudFront call; the ones it carries are {}",
            unbound.join(", "),
            bound.join(", ")
        )))
    }

    /// A string member, or `None` where the call carries none.
    fn text(&self, name: &str) -> Result<Option<String>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(other) => Err(Refusal::NotMade(format!(
                "the call's {name} must be text, got {other}"
            ))),
        }
    }

    /// A string member the call has to carry.
    fn required_text(&self, name: &str) -> Result<String, Refusal> {
        self.text(name)?.ok_or_else(|| {
            Refusal::NotMade(format!(
                "the call carries no {name}, which this CloudFront operation requires"
            ))
        })
    }

    /// A numeric member, or `None` where the call carries none.
    fn number(&self, name: &str) -> Result<Option<i64>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => number.as_i64().map(Some).ok_or_else(|| {
                Refusal::NotMade(format!("the call's {name} must be a whole number"))
            }),
            Some(other) => Err(Refusal::NotMade(format!(
                "the call's {name} must be a number, got {other}"
            ))),
        }
    }

    /// A nested structure member, or `None` where the call carries none.
    fn object(&self, name: &str) -> Result<Option<Self>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Object(members)) => Ok(Some(Self {
                members: members.clone(),
            })),
            Some(other) => Err(Refusal::NotMade(format!(
                "the call's {name} must be a structure, got {other}"
            ))),
        }
    }

    /// A list-of-strings member, empty where the call carries none.
    fn texts(&self, name: &str) -> Result<Vec<String>, Refusal> {
        match self.members.get(name) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(values)) => values
                .iter()
                .map(|value| match value {
                    Value::String(text) => Ok(text.clone()),
                    other => Err(Refusal::NotMade(format!(
                        "every entry of the call's {name} must be text, got {other}"
                    ))),
                })
                .collect(),
            Some(other) => Err(Refusal::NotMade(format!(
                "the call's {name} must be a list, got {other}"
            ))),
        }
    }
}

/// What the ANSWER itself carried, recorded as it arrived rather than assumed: the status
/// CloudFront answered and the id it gave the request.
///
/// An SDK client reports an operation's output members and not the HTTP answer they were
/// read out of, so the one place that answer passes through is read here and nowhere else.
#[derive(Clone, Default)]
struct Observed(Arc<Mutex<Option<Carried>>>);

/// The facts of one HTTP answer this seam reports to the guest.
#[derive(Clone)]
struct Carried {
    /// The status CloudFront answered.
    status: u16,
    /// The id it gave the request it answered.
    request_id: Option<String>,
}

/// The header CloudFront carries the id of the request it answers in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The name this interceptor is registered under on the client's own configuration.
const OBSERVER_NAME: &str = "lambda-runtime-cloudfront-answer";

impl ::std::fmt::Debug for Observed {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("Observed").finish_non_exhaustive()
    }
}

impl ::aws_smithy_runtime_api::client::interceptors::Intercept for Observed {
    fn name(&self) -> &'static str {
        OBSERVER_NAME
    }

    fn read_before_deserialization(
        &self,
        context: &::aws_smithy_runtime_api::client::interceptors::context::BeforeDeserializationInterceptorContextRef<'_>,
        _components: &::aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
        _config: &mut ::aws_smithy_types::config_bag::ConfigBag,
    ) -> Result<(), ::aws_smithy_runtime_api::box_error::BoxError> {
        let response = context.response();
        let carried = Carried {
            status: response.status().as_u16(),
            request_id: response
                .headers()
                .get(REQUEST_ID_HEADER)
                .map(str::to_string),
        };
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(carried);
        Ok(())
    }
}

impl Observed {
    /// What the answer carried, or a call not made where no answer reached this seam at all.
    fn carried(&self) -> Result<Carried, Refusal> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                Refusal::NotMade("the call reached no CloudFront answer at all".to_string())
            })
    }

    /// One served answer: the output members `document` holds, under the status and request
    /// id CloudFront itself answered.
    ///
    /// Bounded by [`MAX_RESPONSE_BYTES`], the ceiling every answer of this seam is read back
    /// under: a listing whose page a caller made large is reported as an answer too large to
    /// read rather than handed over whole.
    fn answer(&self, document: Map<String, Value>) -> Result<Answer, Refusal> {
        let carried = self.carried()?;
        let body = Value::Object(document).to_string();
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(Refusal::ResponseTooLarge);
        }
        Ok(Answer {
            status: carried.status,
            body,
            error_type: None,
            request_id: carried.request_id,
            // Every value of a CloudFront answer is text or a number; none of its
            // invalidation members carries bytes.
            streams: Vec::new(),
            blobs: Vec::new(),
        })
    }
}

/// CloudFront's own REFUSAL, as the answer the guest reads back: the code it named, the
/// message it worded and the status it answered, so the SDK the guest calls raises the
/// modelled error class for that code with that message.
///
/// A call that reached no answer — nothing to report a status or a code from — is reported
/// as a call that was not made, carrying why.
fn refused<E: ProvideErrorMetadata>(
    error: &::aws_sdk_cloudfront::error::SdkError<E, ::aws_smithy_runtime_api::http::Response>,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    let Some(code) = error.code() else {
        return Err(Refusal::NotMade(format!(
            "the CloudFront call could not be made: {error}"
        )));
    };
    let carried = observed.carried()?;
    let mut document = Map::new();
    // `__type` and `message` are where the SDK the guest calls reads a refusal's code and
    // sentence from, so the answer carries CloudFront's own in both.
    document.insert("__type".to_string(), Value::String(code.to_string()));
    document.insert(
        "message".to_string(),
        Value::String(error.message().unwrap_or_default().to_string()),
    );
    Ok(Answer {
        status: carried.status,
        body: Value::Object(document).to_string(),
        error_type: Some(code.to_string()),
        request_id: carried.request_id,
        streams: Vec::new(),
        blobs: Vec::new(),
    })
}

/// Put a string member on the answer's document, where the answer carried one.
fn insert_text(document: &mut Map<String, Value>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::String(value.to_string()));
    }
}

/// Put a numeric member on it, the same way.
fn insert_number(document: &mut Map<String, Value>, name: &str, value: Option<i64>) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::Number(value.into()));
    }
}

/// Put a timestamp member on it, as the epoch seconds an AWS JSON protocol carries a
/// timestamp as — which is the form this seam hands every other service's timestamps over
/// in.
fn insert_timestamp(
    document: &mut Map<String, Value>,
    name: &str,
    value: Option<&::aws_smithy_types::DateTime>,
) {
    if let Some(value) = value {
        document.insert(name.to_string(), Value::Number(value.secs().into()));
    }
}

#[cfg(test)]
mod tests {
    use super::{Input, BOUND_OPERATIONS};

    #[test]
    fn every_bound_operation_is_named_once() {
        let mut names = BOUND_OPERATIONS.to_vec();
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            total,
            "case \"the bound operations\": expected each operation to be named once, got \
             {BOUND_OPERATIONS:?}"
        );
    }

    #[test]
    fn an_input_is_read_as_the_operations_own_members() {
        // Rows differ along the shape of the value a member carries: text, a number, a
        // nested structure, a list of text, and a value of the wrong shape for each.
        let input = Input::read(
            r#"{"DistributionId":"E1EXAMPLE","MaxItems":5,
                "InvalidationBatch":{"CallerReference":"ref-1",
                "Paths":{"Items":["/a","/b"]}},"Wrong":{"a":1}}"#,
        )
        .expect("the document is an object");
        assert_eq!(
            input.required_text("DistributionId").ok(),
            Some("E1EXAMPLE".to_string()),
            "case \"a text member\": expected the distribution id"
        );
        assert_eq!(
            input.number("MaxItems").ok().flatten(),
            Some(5),
            "case \"a numeric member\": expected 5"
        );
        let batch = input
            .object("InvalidationBatch")
            .ok()
            .flatten()
            .expect("case \"a nested structure\": expected the batch");
        assert_eq!(
            batch.text("CallerReference").ok().flatten(),
            Some("ref-1".to_string()),
            "case \"a nested text member\": expected the caller reference"
        );
        let paths = batch
            .object("Paths")
            .ok()
            .flatten()
            .expect("case \"a doubly nested structure\": expected the paths");
        assert_eq!(
            paths.texts("Items").ok(),
            Some(vec!["/a".to_string(), "/b".to_string()]),
            "case \"a list of text\": expected both paths"
        );
        assert!(
            paths.texts("Absent").is_ok_and(|items| items.is_empty()),
            "case \"an absent list\": expected no entries"
        );
        assert!(
            input.text("MaxItems").is_err(),
            "case \"a number read as text\": expected a call that was not made"
        );
        assert!(
            input.number("DistributionId").is_err(),
            "case \"text read as a number\": expected a call that was not made"
        );
        assert!(
            input.object("DistributionId").is_err(),
            "case \"text read as a structure\": expected a call that was not made"
        );
        assert!(
            input.only(&["DistributionId", "MaxItems", "InvalidationBatch"]).is_err(),
            "case \"an unbound member\": expected a call that was not made"
        );
    }
}
