// SPDX-License-Identifier: CC0-1.0

//! The DynamoDB Streams seam.
//!
//! DynamoDB Streams models no create or delete API of its own: "a stream comes into
//! existence" when a `StreamSpecification` on `CreateTable`/`UpdateTable` enables
//! one, and the STREAMS service owns the registry its own consumers (`ListStreams`,
//! `DescribeStream`, `GetShardIterator`, `GetRecords`) read. So enabling a stream
//! here has to register it THERE, on the same shared engine, rather than keeping a
//! second copy of the resource in this service's store: what this service keeps is
//! the table's latest stream identity, projected into every `TableDescription`.
//!
//! It lives in its own module, re-exported from [`crate::state`], so the seam is one
//! file to read rather than a section at the end of the store.
//!
//! The DATA plane's half is [`publish_item_changes`]: an item write records what it
//! changed on this service's store and the handler hands the queue to the stores that
//! hold the records at the end of the request, so the records a consumer reads are
//! appended THERE too.

use std::time::SystemTime;

use crate::ops::types::{KeySchemaElement, StreamSpecification};
use crate::state::{RequestError, TableStream};

/// A table's stream must not look readable before the table itself serves item
/// writes: a consumer that waits for the stream's shards and then writes an item
/// would otherwise reach a table still `CREATING`. The streams registry publishes a
/// stream's shards when its enable falls due, so that span has to be at least this
/// service's own `CREATING` -> `ACTIVE` span, and this is where the two are held
/// together rather than left to agree by accident.
const _: () = assert!(
    crate::state::STATUS_TRANSITION.as_nanos()
        <= ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::state::STREAM_ENABLE_DELAY
            .as_nanos(),
    "a table must reach ACTIVE no later than its stream reports ENABLED"
);

/// AWS's sentence for a request that declares a stream ON a new table and names no
/// view type to capture in: the member is required as soon as the stream is enabled.
pub const STREAM_VIEW_TYPE_REQUIRED: &str =
    "One or more parameter values were invalid: StreamViewType is required when StreamEnabled \
     is true";

/// The same requirement where an EXISTING table's stream is being turned on, which is
/// how AWS words it for a request that changes a table rather than creating one.
pub const STREAM_VIEW_TYPE_REQUIRED_ON_ENABLE: &str =
    "One or more parameter values were invalid: If stream is being enabled then UpdateViewType \
     is required";

/// AWS's sentence for the other half of the same pair: a specification that leaves the
/// stream OFF and still names a view type to capture in.
///
/// There is nothing to capture, so the member states a setting for a stream the request
/// is not asking for. AWS words it over the view-type member (`UpdateViewType`, the name
/// its refusals use for it) and over what the request is doing to the table.
pub const STREAM_VIEW_TYPE_FORBIDDEN_ON_CREATE: &str =
    "One or more parameter values were invalid: Table is being created with a stream disabled, \
     UpdateViewType should not be specified";

/// The same fact for a request that CHANGES a table, worded over what that request is
/// doing — the pairing the two `STREAM_VIEW_TYPE_REQUIRED*` sentences above follow.
pub const STREAM_VIEW_TYPE_FORBIDDEN_ON_UPDATE: &str =
    "One or more parameter values were invalid: Table is being updated with a stream disabled, \
     UpdateViewType should not be specified";

/// The `StreamViewType` the specification asks for, refused when the pair cannot
/// hold together.
///
/// `StreamViewType` "determines what information is written to the stream for this
/// table" (`StreamSpecification$StreamViewType`), so the member and `StreamEnabled`
/// require each other BOTH ways: a specification that turns the stream ON without naming
/// one asks for a stream whose contents are undefined, and one that leaves it OFF while
/// naming one states what to capture for a stream it is not asking for. Either way the
/// specification contradicts itself and the request is refused — with whichever of the
/// two sentences the calling operation states.
pub fn stream_view_type<'s>(
    specification: &'s StreamSpecification,
    missing_view_type: &str,
    forbidden_view_type: &str,
) -> Result<Option<&'s str>, RequestError> {
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;

    if !specification.stream_enabled {
        if specification.stream_view_type.is_some() {
            return Err(RequestError::Validation(forbidden_view_type.to_string()));
        }
        return Ok(None);
    }
    match specification.stream_view_type.as_deref() {
        // The member is an enum in the model, so the boundary validator has already
        // refused a value outside the four it declares. The registry parses the same
        // four, and a value it cannot parse is still a BAD REQUEST rather than a
        // server fault, so the two agree here instead of at the seam.
        Some(view_type) if streams::state::StreamViewType::from_wire(view_type).is_some() => {
            Ok(Some(view_type))
        }
        Some(view_type) => Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: StreamViewType {view_type} is not one of \
             {}",
            // Rendered from this service's own model value list, so the message and
            // the member's declared enum cannot drift.
            crate::ops::types::STREAM_VIEW_TYPE_VALUES.join(", ")
        ))),
        None => Err(RequestError::Validation(missing_view_type.to_string())),
    }
}

/// Why a stream could not be registered for a table.
#[derive(Debug, Clone)]
pub enum StreamFault {
    /// The DynamoDB Streams store cannot be reached at all (the engine is not the
    /// shared in-memory one), which a handler answers with its modelled SERVER
    /// error rather than with a table whose stream nothing registered.
    StoreUnavailable,
    /// The streams registry refused the registration and said why: a quota of the
    /// registry, which the control plane answers with its own
    /// `LimitExceededException`.
    Rejected(String),
    /// An invariant of this seam did not hold — a server fault, worded for what
    /// actually went wrong rather than as an unreachable store.
    Internal(&'static str),
}

/// Register a stream for `table_name` in the DynamoDB Streams store and report the
/// table's new latest-stream identity.
///
/// This is the publishing seam: the stream, its shards and its records live in the
/// streams service's state, so the ARN and label reported here are the ones its own
/// `ListStreams`/`DescribeStream` answer with.
///
/// LOCKING: this takes the STREAMS store's lock, and a handler calls it while
/// holding this service's own. That order is the only one in the emulator — the
/// streams service never reaches into the DynamoDB store — so the two cannot
/// deadlock, and the work done under both is bounded by constants (one prefix range
/// over the table's own streams and at most `MAX_STREAMS_PER_TABLE` label probes),
/// never by the request or by state a caller has grown.
pub fn register_table_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    table_name: &str,
    region: &str,
    key_schema: &[KeySchemaElement],
    view_type: &str,
    at: SystemTime,
) -> Result<TableStream, StreamFault> {
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;

    // The view type is validated by `stream_view_type` before any of this, so a
    // value the registry cannot parse is a bad request that never reaches here.
    let captured = streams::state::StreamViewType::from_wire(view_type).ok_or(
        StreamFault::Internal("the stream view type was not validated before registration"),
    )?;
    let store = streams::state::service_state(engine).ok_or(StreamFault::StoreUnavailable)?;
    let mut store = store.lock();
    // Everything the clock already owes the registry first, so a stream past the
    // documented 24 hours of its disable is reclaimed before this registration is
    // counted against the registry's quotas.
    store.materialize_due_expiries(at);
    let arn = store
        .register_stream(
            &streams::state::StreamRegistration {
                table_name: table_name.to_string(),
                stream_view_type: captured,
                // The stream records a change by the table's own primary key, so the
                // registry is given the schema that key is read with.
                key_schema: key_schema
                    .iter()
                    .map(|element| streams::ops::types::KeySchemaElement {
                        attribute_name: element.attribute_name.clone(),
                        key_type: element.key_type.clone(),
                    })
                    .collect(),
                region: region.to_string(),
            },
            at,
        )
        .map_err(|rejected| StreamFault::Rejected(rejected.to_string()))?;
    // The label is read back off the REGISTERED stream rather than derived again
    // here, so the table and the streams service cannot report two different
    // labels for one stream.
    let label = store
        .stream(&arn)
        .ok_or(StreamFault::Internal(
            "the registered stream could not be read back",
        ))?
        .stream_label
        .clone();
    Ok(TableStream {
        arn,
        label,
        view_type: view_type.to_string(),
        enabled: true,
    })
}

/// Hand every item-level change the writes of this request made to the stores that hold
/// the records: the DynamoDB Streams registry, and the Kinesis data stream of every
/// streaming destination the table delivers to.
///
/// This is the DATA plane's half of the publishing seam: a write landed an item in
/// THIS service's store and queued what it changed
/// ([`crate::state::PendingStreamRecord`]); the records themselves are the streams
/// service's resources, so they are appended THERE, which is where every stream
/// consumer reads them — and the same change is delivered to the table's Kinesis
/// streaming destinations ([`crate::state::deliver_item_changes`]), whose records are the
/// Kinesis service's own.
///
/// The queue is drained ONCE here and handed to both, so the two cannot disagree about
/// which changes a request made.
///
/// LOCKING: like [`register_table_stream`], this takes the STREAMS store's lock while
/// the caller holds this service's own, which is the one order the emulator uses, and then
/// the KINESIS store's under the same order. The work is bounded by the queue, and the
/// queue by one request's writes.
pub fn publish_item_changes<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &mut crate::state::ComAmazonawsDynamodbDynamodb20120810State,
    at: SystemTime,
) {
    let changes = store.take_stream_records();
    if changes.is_empty() {
        return;
    }
    publish_to_streams(engine, &changes, at);
    // The same changes, to the table's Kinesis streaming destinations: a table can publish
    // to its own stream, to a Kinesis data stream, or to both, so each store is handed the
    // queue independently of what the other holds.
    crate::state::deliver_item_changes(engine, store, &changes, at);
    // And the key activity each change made, to the CloudWatch Contributor Insights rules of
    // the table it was made on: a write of an item is an access of its key, so what the write
    // consumed is attributed to that key where those rules collect
    // ([`crate::state::contributor_insights`]).
    crate::state::contributor_insights::record_item_changes(engine, store, &changes, at);
}

/// Append every change to the DynamoDB stream it belongs to, for the changes made on a
/// table whose own stream is enabled: a change queued by a table with no stream belongs to
/// none and is only a Kinesis streaming destination's.
///
/// Which modification a record describes is derived from the images the write queued: an
/// item that was not there before is an `INSERT`, one that was and still is a `MODIFY`,
/// and one that is gone a `REMOVE` (the model's `OperationType`). The stream drops the
/// images its own `StreamViewType` does not capture, so what a consumer sees is never
/// more than the stream was created to capture.
///
/// Publication cannot fail the write that made the change — the item is already stored
/// and the response is already decided, and DynamoDB publishes to a stream
/// asynchronously — so a change the registry will not take is LOGGED rather than
/// turned into an error for the caller.
fn publish_to_streams<Eng: Send + Sync + 'static>(
    engine: &Eng,
    changes: &[crate::state::PendingStreamRecord],
    at: SystemTime,
) {
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;

    if changes.iter().all(|change| change.stream_arn.is_none()) {
        return;
    }
    let Some(registry) = streams::state::service_state(engine) else {
        ::tracing::error!(
            changes = changes.len(),
            "the DynamoDB Streams store is unreachable, so item changes reached no stream"
        );
        return;
    };
    let mut registry = registry.lock();
    for change in changes {
        let Some(stream_arn) = change.stream_arn.as_deref() else {
            continue;
        };
        let event_name = match (&change.old_image, &change.new_image) {
            (None, Some(_)) => streams::state::OperationType::Insert,
            (Some(_), Some(_)) => streams::state::OperationType::Modify,
            (Some(_), None) => streams::state::OperationType::Remove,
            // A queued change with neither image describes no modification, so there
            // is nothing to publish; no write path queues one.
            (None, None) => continue,
        };
        let event = streams::state::RecordAppend {
            event_name,
            keys: streams_image(&change.keys),
            new_image: change.new_image.as_ref().map(streams_image),
            old_image: change.old_image.as_ref().map(streams_image),
        };
        if let Err(rejected) = registry.append_record(stream_arn, &event, at) {
            ::tracing::error!(
                stream_arn = %stream_arn,
                event_name = event_name.as_str(),
                reason = ?rejected,
                "an item change could not be appended to its stream"
            );
        }
    }
}

/// One stored image, as the STREAMS service's own attribute map.
///
/// The two services model the same `AttributeValue` union, but each generates its own
/// type from its own model, so an image crossing the seam is rebuilt member for member
/// rather than transmuted.
fn streams_image(
    image: &crate::state::Item,
) -> ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::state::AttributeMap {
    image
        .iter()
        .map(|(name, value)| (name.clone(), streams_value(value)))
        .collect()
}

/// One stored attribute value, as the STREAMS service's own `AttributeValue`.
///
/// ITERATIVE, over an explicit worklist, for the reason the streams store gives for
/// its own walks over an image: `M` and `L` nest, and a recursive walk over a nesting
/// depth a client chose overflows the thread stack — which in Rust aborts the process
/// instead of unwinding. Every write path holds a value to
/// [`crate::state::MAX_ATTRIBUTE_DEPTH`] before it is stored, and the registry checks
/// the same limit again on append; this walk does not have to rely on either of them.
///
/// A nesting value is met twice: once to put its members on the worklist, and once to
/// rebuild it from the members converted since. A map's names are CARRIED from the
/// pass that pushed its members rather than read off the map a second time, so the
/// pairing does not depend on two iterations of one map agreeing. The members of a
/// container occupy the top of `converted` when it is rebuilt, in the reverse of the
/// order they were pushed in, which is why each is read back reversed.
fn streams_value(
    root: &crate::ops::types::AttributeValue,
) -> ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::AttributeValue {
    use crate::ops::types::AttributeValue as Stored;
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::AttributeValue as Published;

    /// What the walk still owes: a value to convert, or a container to rebuild from
    /// the members already converted — a map from the names its members were pushed
    /// under, a list from how many it pushed.
    enum Step<'a> {
        Convert(&'a Stored),
        RebuildMap(Vec<&'a String>),
        RebuildList(usize),
    }

    let mut worklist = vec![Step::Convert(root)];
    let mut converted: Vec<Published> = Vec::new();
    while let Some(step) = worklist.pop() {
        match step {
            Step::Convert(Stored::M(members)) => {
                // ONE pass over the map: the names go on the rebuild step and their
                // values on the worklist, in the same order.
                let mut names = Vec::with_capacity(members.len());
                let mut values = Vec::with_capacity(members.len());
                for (name, nested) in members {
                    names.push(name);
                    values.push(nested);
                }
                worklist.push(Step::RebuildMap(names));
                worklist.extend(values.into_iter().map(Step::Convert));
            }
            Step::Convert(Stored::L(members)) => {
                worklist.push(Step::RebuildList(members.len()));
                worklist.extend(members.iter().map(Step::Convert));
            }
            // Every other member holds no nested value, so it converts on its own.
            Step::Convert(Stored::S(text)) => converted.push(Published::S(text.clone())),
            Step::Convert(Stored::N(number)) => converted.push(Published::N(number.clone())),
            Step::Convert(Stored::B(bytes)) => converted.push(Published::B(bytes.clone())),
            Step::Convert(Stored::Ss(members)) => converted.push(Published::Ss(members.clone())),
            Step::Convert(Stored::Ns(members)) => converted.push(Published::Ns(members.clone())),
            Step::Convert(Stored::Bs(members)) => converted.push(Published::Bs(members.clone())),
            Step::Convert(Stored::Null(is_null)) => converted.push(Published::Null(*is_null)),
            Step::Convert(Stored::Bool(flag)) => converted.push(Published::Bool(*flag)),
            Step::RebuildMap(names) => {
                let nested = take_members(&mut converted, names.len());
                converted.push(Published::M(
                    names
                        .into_iter()
                        .cloned()
                        .zip(nested.into_iter().rev())
                        .collect(),
                ));
            }
            Step::RebuildList(count) => {
                let mut nested = take_members(&mut converted, count);
                nested.reverse();
                converted.push(Published::L(nested));
            }
        }
    }
    // The walk pushes exactly one value for the root, so this is it. A `NULL` stands in
    // only for a state this construction cannot reach, rather than letting the seam
    // panic inside a request that has already written its item; reaching it would be a
    // defect in the walk above, so it is reported as one.
    debug_assert_eq!(
        converted.len(),
        1,
        "the walk leaves exactly the root's converted value"
    );
    converted.pop().unwrap_or_else(|| {
        ::tracing::error!("an attribute value converted to no value for its stream record");
        Published::Null(true)
    })
}

/// The `count` values a container's members converted to, taken off the top of the
/// walk's stack.
///
/// Every `Step::Convert` pushes exactly one value, so the top `count` are this
/// container's. A shorter stack would be a defect in that invariant, so it is reported
/// rather than silently truncating the container the caller is about to publish.
fn take_members<T>(converted: &mut Vec<T>, count: usize) -> Vec<T> {
    if converted.len() < count {
        ::tracing::error!(
            converted = converted.len(),
            members = count,
            "fewer converted values than the attribute's members"
        );
    }
    debug_assert!(
        converted.len() >= count,
        "every member converts to exactly one value"
    );
    let from = converted.len().saturating_sub(count);
    converted.split_off(from)
}

/// Turn a table's stream OFF in the DynamoDB Streams store.
///
/// The stream stays registered and keeps the records it already carries — AWS
/// deletes a disabled stream only after 24 hours — so a consumer can still discover
/// it and drain it; it reports `DISABLING` and then `DISABLED`, and takes no further
/// records. `false` when the streams store cannot be reached or holds no stream with
/// that ARN.
pub fn disable_table_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stream_arn: &str,
    at: SystemTime,
) -> bool {
    use ::service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810 as streams;

    streams::state::service_state(engine)
        .is_some_and(|store| store.lock().disable_stream(stream_arn, at))
}
