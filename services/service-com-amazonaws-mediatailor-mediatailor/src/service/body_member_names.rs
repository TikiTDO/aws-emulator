// SPDX-License-Identifier: CC0-1.0

//! The JSON keys this service READS a request's body members under.
//!
//! # The rule
//!
//! A body member the model RENAMES on the wire (`@jsonName`) is read under EITHER spelling:
//! the renamed key the AWS SDKs write, or the member's own Smithy member name. A member
//! absent under both spellings is absent — and then, and only then, the request is refused
//! for it.
//!
//! The rename is a CLIENT-side spelling: a model states it so a generated client writes one
//! key, and the service reading the body accepts the member name the model declares as well.
//! That is the recorded behaviour the rule comes from, not a leniency chosen here: a tagging
//! write whose body spelled its map member under the member name the model declares was
//! answered as a SUCCESS, and the read that followed it reported the tags that write had
//! stored. Every MediaTailor request body carrying a renamed member is therefore served
//! whichever spelling the caller sent, so the member reaches the handler and the operation
//! does its work instead of being refused as though the caller had sent nothing.
//!
//! # Scope
//!
//! The keys re-keyed are the body object's own, which is where the codec binds a member by
//! its wire name; every `@jsonName` this service's model declares is on such a member, so
//! the rule holds wherever the model states a rename. A nested shape that one day declares
//! one would need the walk to descend, which the schema a member carries does not reach
//! today.
//!
//! # Why a seam and not a line in each handler
//!
//! The spelling decides whether the member ever BECOMES a value: the framework's codec binds
//! each body key to a member by its wire name, and a key it binds to nothing is skipped — so
//! a `@required` member spelled the other way is reported absent before any handler runs. No
//! handler can see that; the only place the decision is still open is the request, which is
//! why the rule lives in a wrapper around each modelled operation ([`MemberNamedBody`]) that
//! re-keys the body it is handed and then runs the operation unchanged. Deserialization,
//! constraint validation, the handler and every response stay the framework's and the
//! service's own.
//!
//! The wrapper reads which members are renamed off the operation's own input schema, so it
//! states no member name and no operation name of its own: an operation whose input renames
//! nothing has nothing to re-key, which is read from the model rather than from a list here.

use ::std::sync::Arc;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::aws_smithy_schema::Schema;
use ::aws_smithy_types::body::SdkBody;
use ::serde_json::Value;
use ::service_common::query_error_envelope::RequestIdReportingOperation;
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Service,
};

/// Every member of `input_schema` the model gives a wire name that DIFFERS from its Smithy
/// member name, as `(member name, wire name)`.
///
/// Both names come from the schema the model built, so this is the model's own answer to
/// "which keys name the same member".
fn renamed_members(input_schema: &Schema) -> Vec<(&str, &str)> {
    input_schema
        .members()
        .iter()
        .copied()
        .filter_map(|member| {
            let wire = member.json_name()?.value();
            let modelled = member.member_name()?;
            (wire != modelled).then_some((modelled, wire))
        })
        .collect()
}

/// `req` with every top-level body key that spells a renamed member's MEMBER NAME re-keyed
/// to the wire name the codec reads that member under, or `None` when there is nothing to
/// re-key.
///
/// `None` covers every request that is already read correctly: an operation whose input
/// renames no member, an empty body, a body that is not a JSON object, and a body that
/// already spells the member the way the codec reads it. A key present under BOTH spellings
/// keeps the wire one — that is the key the codec would have bound, so accepting the other
/// spelling can never change what a request already meant.
fn with_wire_member_names(req: &RtRequest, input_schema: &Schema) -> Option<RtRequest> {
    let renamed = renamed_members(input_schema);
    if renamed.is_empty() {
        return None;
    }
    let bytes = req.body().bytes()?;
    if bytes.is_empty() {
        return None;
    }
    let Ok(Value::Object(mut object)) = ::serde_json::from_slice::<Value>(bytes) else {
        return None;
    };
    let mut rekeyed = false;
    for (modelled, wire) in renamed {
        if object.contains_key(wire) {
            continue;
        }
        if let Some(value) = object.remove(modelled) {
            object.insert(wire.to_string(), value);
            rekeyed = true;
        }
    }
    if !rekeyed {
        return None;
    }

    let body = ::serde_json::to_vec(&Value::Object(object)).ok()?;
    let length = body.len().to_string();
    let mut out = RtRequest::new(SdkBody::from(body));
    out.set_uri(req.uri()).ok()?;
    out.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        // `append`, not `insert`: a repeated or multi-valued header must keep every value,
        // exactly as the router's own request rebuild does.
        out.headers_mut().append(name.to_string(), value.to_string());
    }
    // The re-keyed body is a different number of bytes, so a request that declared its
    // length declares the new one. A decimal length is always a valid header value; a value
    // the HTTP layer refused would leave a length describing the body that is no longer
    // there, so the header is REMOVED in that case.
    if out.headers().get("content-length").is_some()
        && out
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
    {
        out.headers_mut().remove("content-length");
    }
    Some(out)
}

/// One modelled operation, served with its body read under both spellings of every member
/// the model renames.
///
/// Nothing else is re-implemented: the request this wrapper hands on is the caller's, with
/// at most the KEY of a renamed member changed to the one the codec binds, and the generated
/// [`ErasedOperation::run`] then owns deserialize -> validate -> handle -> serialize,
/// telemetry and every modelled error exactly as it does for an unwrapped operation.
///
/// Only the DISPATCHED request is re-keyed; the [`OperationContext`] still carries the
/// request parts the client sent, and the body the emulator authenticated is the caller's
/// own, because authentication happens before any operation is named.
pub struct MemberNamedBody<O> {
    operation: O,
}

impl<O> MemberNamedBody<O> {
    /// Serve `operation` with its body read under both spellings.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O: Operation + ErasedOperation + 'static> ErasedOperation for MemberNamedBody<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match with_wire_member_names(req, O::INPUT_SCHEMA) {
                Some(rekeyed) => self.operation.run(svc, &rekeyed, ctx).await,
                None => self.operation.run(svc, req, ctx).await,
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's own schemes: wrapping a request's body cannot change how
        // the request is signed.
        self.operation.auth_schemes()
    }
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`MemberNamedBody`] and keyed by its modelled operation name — the routing key
/// `ServerProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! serve_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            serve(
                &mut $table,
                crate::ops::$module::$operation::new(Arc::clone($engine)),
            );
        )+
    };
}

/// Put `operation` in `table` under its modelled name, served with its body read under both
/// spellings and with the request id reported on whatever it answered.
///
/// The two wrappers compose in that order: the body rule decides what the operation is
/// handed, and the report is applied to the response it produced — a success, a modelled
/// error, one of this service's own envelopes or a constraint refusal raised while it was
/// being served. The report is `service_common`'s, which is where the rule for every
/// emulated service lives; a service that serves its operations wrapped composes it around
/// each of them, which is this one line.
fn serve<O: Operation + ErasedOperation + 'static>(
    table: &mut OperationRoutingTable,
    operation: O,
) {
    table.add_operation(
        O::NAME,
        Box::new(RequestIdReportingOperation::error_member(
            MemberNamedBody::new(operation),
        )),
    );
}

/// EVERY modelled operation of this service, each served behind [`MemberNamedBody`] and the
/// shared request-id report.
///
/// All of them, so the rule is applied by ONE code path rather than per operation: an
/// operation whose input renames no member simply has nothing to re-key, which the wrapper
/// reads off that operation's input schema. The operations are built from the SAME shared
/// engine, so a wrapped operation reads and writes the one store.
pub fn served_operations<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let mut table = OperationRoutingTable::new();
    serve_operations!(
        table,
        engine,
        configure_logs_for_channel::ConfigureLogsForChannel,
        configure_logs_for_playback_configuration::ConfigureLogsForPlaybackConfiguration,
        create_channel::CreateChannel,
        create_live_source::CreateLiveSource,
        create_prefetch_schedule::CreatePrefetchSchedule,
        create_program::CreateProgram,
        create_source_location::CreateSourceLocation,
        create_vod_source::CreateVodSource,
        delete_channel::DeleteChannel,
        delete_channel_policy::DeleteChannelPolicy,
        delete_function::DeleteFunction,
        delete_live_source::DeleteLiveSource,
        delete_playback_configuration::DeletePlaybackConfiguration,
        delete_prefetch_schedule::DeletePrefetchSchedule,
        delete_program::DeleteProgram,
        delete_source_location::DeleteSourceLocation,
        delete_vod_source::DeleteVodSource,
        describe_channel::DescribeChannel,
        describe_live_source::DescribeLiveSource,
        describe_program::DescribeProgram,
        describe_source_location::DescribeSourceLocation,
        describe_vod_source::DescribeVodSource,
        get_channel_policy::GetChannelPolicy,
        get_channel_schedule::GetChannelSchedule,
        get_function::GetFunction,
        get_playback_configuration::GetPlaybackConfiguration,
        get_prefetch_schedule::GetPrefetchSchedule,
        list_alerts::ListAlerts,
        list_channels::ListChannels,
        list_functions::ListFunctions,
        list_live_sources::ListLiveSources,
        list_playback_configurations::ListPlaybackConfigurations,
        list_prefetch_schedules::ListPrefetchSchedules,
        list_source_locations::ListSourceLocations,
        list_tags_for_resource::ListTagsForResource,
        list_vod_sources::ListVodSources,
        put_channel_policy::PutChannelPolicy,
        put_function::PutFunction,
        put_playback_configuration::PutPlaybackConfiguration,
        start_channel::StartChannel,
        stop_channel::StopChannel,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_channel::UpdateChannel,
        update_live_source::UpdateLiveSource,
        update_program::UpdateProgram,
        update_source_location::UpdateSourceLocation,
        update_vod_source::UpdateVodSource,
    );
    table
}

#[cfg(test)]
mod tests {
    //! The rule over the shapes a request body takes, read off the model's own schemas.

    use super::*;
    use crate::ops::types as t;

    /// A request carrying `body` for the operation whose input `schema` is.
    fn request(body: &str) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::from(body.to_string()));
        req.set_method("POST").expect("a method the test states");
        req.set_uri("/tags/arn").expect("a URI the test states");
        req.headers_mut()
            .insert("content-length", body.len().to_string());
        req
    }

    /// The body `req` carries, as a JSON value.
    fn body_of(req: &RtRequest) -> Value {
        ::serde_json::from_slice(req.body().bytes().expect("a buffered body"))
            .expect("the re-keyed body is JSON")
    }

    /// Which members the model renames is read off the schema: the tagging write renames
    /// its one map member, and an input whose members are all spelled as the model declares
    /// them renames nothing.
    #[test]
    fn the_model_says_which_members_are_renamed() {
        assert_eq!(
            renamed_members(t::TAG_RESOURCE_INPUT_SCHEMA),
            vec![("Tags", "tags")],
            "case TagResource: expected the modelled rename, got {:?}",
            renamed_members(t::TAG_RESOURCE_INPUT_SCHEMA)
        );
        for (case, schema) in [
            ("UntagResource", t::UNTAG_RESOURCE_INPUT_SCHEMA),
            ("UpdateChannel", t::UPDATE_CHANNEL_INPUT_SCHEMA),
            (
                "ConfigureLogsForChannel",
                t::CONFIGURE_LOGS_FOR_CHANNEL_INPUT_SCHEMA,
            ),
        ] {
            assert!(
                renamed_members(schema).is_empty(),
                "case {case}: no member is renamed, got {:?}",
                renamed_members(schema)
            );
        }
    }

    /// The bodies a caller can send for a member the model renames: the member name is
    /// re-keyed to the wire name, and everything already readable is handed on untouched.
    #[test]
    fn a_renamed_member_is_read_under_either_spelling() {
        // (case, body, the value the member holds after re-keying, or None for "handed on
        // as it was")
        let rows: [(&str, &str, Option<Value>); 7] = [
            (
                "the Smithy member name",
                r#"{"Tags":{"Env":"prod"}}"#,
                Some(::serde_json::json!({"Env":"prod"})),
            ),
            (
                "the member name beside another key",
                r#"{"Tags":{"Env":"prod"},"Other":1}"#,
                Some(::serde_json::json!({"Env":"prod"})),
            ),
            (
                "the member name with an empty map",
                r#"{"Tags":{}}"#,
                Some(::serde_json::json!({})),
            ),
            ("the wire name, as an SDK writes it", r#"{"tags":{}}"#, None),
            (
                "both spellings: the wire one is what the codec binds",
                r#"{"tags":{"Env":"wire"},"Tags":{"Env":"member"}}"#,
                None,
            ),
            ("no key for the member at all", r#"{"Other":1}"#, None),
            ("a body that is not a JSON object", r#"["Tags"]"#, None),
        ];

        for (case, body, expected) in rows {
            let rekeyed = with_wire_member_names(&request(body), t::TAG_RESOURCE_INPUT_SCHEMA);
            match expected {
                None => assert!(
                    rekeyed.is_none(),
                    "case {case:?}: expected the request handed on as it was, got {:?}",
                    rekeyed.map(|req| body_of(&req))
                ),
                Some(value) => {
                    let rekeyed = rekeyed
                        .unwrap_or_else(|| panic!("case {case:?}: expected the body re-keyed"));
                    let document = body_of(&rekeyed);
                    assert_eq!(
                        document.get("tags"),
                        Some(&value),
                        "case {case:?}: expected the member under its wire name, got {document}"
                    );
                    assert_eq!(
                        document.get("Tags"),
                        None,
                        "case {case:?}: the member travels under ONE key, got {document}"
                    );
                    assert_eq!(
                        rekeyed.headers().get("content-length"),
                        Some(::serde_json::to_vec(&document).expect("bytes").len().to_string())
                            .as_deref(),
                        "case {case:?}: the declared length is the re-keyed body's, got {:?}",
                        rekeyed.headers().get("content-length")
                    );
                    assert_eq!(
                        rekeyed.method(),
                        "POST",
                        "case {case:?}: the method is the caller's"
                    );
                    assert_eq!(
                        rekeyed.uri(),
                        "/tags/arn",
                        "case {case:?}: the target is the caller's"
                    );
                }
            }
        }
    }

    /// An empty body has no member to re-key, whatever the operation renames.
    #[test]
    fn an_empty_body_is_handed_on_as_it_was() {
        let empty = RtRequest::new(SdkBody::empty());
        assert!(
            with_wire_member_names(&empty, t::TAG_RESOURCE_INPUT_SCHEMA).is_none(),
            "an empty body carries no member to re-key"
        );
    }
}
