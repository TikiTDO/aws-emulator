// SPDX-License-Identifier: CC0-1.0

//! @op Scan
//! IMPLEMENTED: walks the stored table — "accessing every item in a table" — in
//! stored primary-key order, reporting `Count`/`ScannedCount` and the
//! `LastEvaluatedKey` computed from the page it stopped on.
//!
//! The page stops at the caller's `Limit` ("the maximum number of items to
//! evaluate") and in any case at the store's 1 MB dataset budget
//! ([`crate::state::MAX_PAGE_BYTES`]), and `ExclusiveStartKey` resumes strictly after
//! the key a previous page reported. A page reports a `LastEvaluatedKey` when it
//! stopped with items still left to evaluate, which is what lets a caller tell the
//! last page from a full one — "If LastEvaluatedKey is not empty, it does not
//! necessarily mean that there is more data in the result set", so a page that ends
//! exactly on the last item is free to report none. A parallel scan
//! (`Segment`/`TotalSegments`) divides the key space being walked into one block of
//! PARTITIONS per segment, so the segments together evaluate every item exactly once and
//! every item sharing a partition key is evaluated by one of them ([`Segmentation`],
//! [`Partitions`]).
//!
//! `FilterExpression` is applied AFTER the page has been read — "a filter
//! expression is applied after a Scan finishes but before the results are returned"
//! — so the items it removes are still scanned and still charged for, and
//! `ScannedCount` therefore counts what the page evaluated while `Count` counts what
//! it answered. The expression is the `ConditionExpression` language, parsed by the
//! one parser this crate has for it ([`crate::state::ParsedCondition`]).
//!
//! A scan that names an `IndexName` walks that SECONDARY INDEX instead — "accessing
//! every item in a table or a secondary index" — in the order the index holds its
//! entries, which is its own key order with the table's primary key breaking the tie;
//! an item the index does not cover is not there to be scanned, and each entry carries
//! the attributes the index projects. A read that needs an attribute its index does not
//! project is served from the parent table, which only a LOCAL index can do.
//!
//! What each reported item carries is the request's `Select` and whichever projection
//! member it named — `ProjectionExpression` with its `ExpressionAttributeNames` aliases,
//! or the legacy `AttributesToGet` — applied AFTER the filter, so an attribute the
//! response does not carry can still narrow the scan. The legacy `ScanFilter` /
//! `ConditionalOperator` pair states the same filter the expression states, lowered onto
//! the one legacy comparison evaluator this crate has
//! ([`crate::state::legacy_conditions_match`]).
//!
//! `ConsistentRead` needs no handling beyond its effect on `ConsumedCapacity` — this
//! store holds one copy of each item, so a scan of it always sees every completed write
//! — except on a global secondary index, which does not support it and refuses the
//! request. A `ProjectionExpression` path that reaches INSIDE an attribute (`a.b`,
//! `a[1]`) is served like any other: it reports the value at that path wrapped in the
//! containers the path steps through ([`crate::state::project_paths`]).
//!
//! Which index the request named, what each reported item is narrowed to and whether a
//! read has to go on to the parent table are decided by the rules `Query` reads an index
//! by too ([`crate::state::indexed_read`]).
//!
//! A table whose resource-based policy denies the caller `dynamodb:Scan` is
//! answered `AccessDeniedException` instead of its items.
//!
//! Every substitution token the request DEFINES must be one its expressions USE: an
//! `ExpressionAttributeNames`/`ExpressionAttributeValues` key no expression member
//! references makes the whole request invalid
//! ([`crate::state::Substitutions::check_all_referenced`]), the way a token supplied
//! beside no expression at all does.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `Scan::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `Scan`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")
//! ```
//!
//! The Scan operation returns one or more items and item attributes by accessing every item
//! in a table or a secondary index. To have DynamoDB return fewer items, you can provide a
//! FilterExpression operation. If the total size of scanned items exceeds the maximum
//! dataset size limit of 1 MB, the scan completes and results are returned to the user. The
//! LastEvaluatedKey value is also returned and the requestor can use the LastEvaluatedKey
//! to continue the scan in a subsequent operation. Each scan response also includes number
//! of items that were scanned (ScannedCount) as part of the request. If using a
//! FilterExpression, a scan result can result in no items meeting the criteria and the
//! Count will result in zero. If you did not use a FilterExpression in the scan request,
//! then Count is the same as ScannedCount. Count and ScannedCount only return the count of
//! items specific to a single scan request and, unless the table is less than 1MB, do not
//! represent the total number of items in the table. A single Scan operation first reads up
//! to the maximum number of items set (if using the Limit parameter) or a maximum of 1 MB
//! of data and then applies any filtering to the results if a FilterExpression is provided.
//! If LastEvaluatedKey is present in the response, pagination is required to complete the
//! full table scan. For more information, see Paginating the Results in the Amazon DynamoDB
//! Developer Guide. Scan operations proceed sequentially; however, for faster performance
//! on a large table or secondary index, applications can request a parallel Scan operation
//! by providing the Segment and TotalSegments parameters. For more information, see
//! Parallel Scan in the Amazon DynamoDB Developer Guide. By default, a Scan uses eventually
//! consistent reads when accessing the items in a table. Therefore, the results from an
//! eventually consistent Scan may not include the latest item changes at the time the scan
//! iterates through each item in the table. If you require a strongly consistent read of
//! each item as the scan iterates through the items in the table, you can set the
//! ConsistentRead parameter to true. Strong consistency only relates to the consistency of
//! the read at the item level. DynamoDB does not provide snapshot isolation for a scan
//! operation when the ConsistentRead parameter is set to true. Thus, a DynamoDB scan
//! operation does not guarantee that all reads in a scan see a consistent snapshot of the
//! table when the scan operation was requested.
//!
//! ## Input
//!
//! * `AttributesToGet` (AttributeNameList, len 1..) — This is a legacy parameter. Use
//!   ProjectionExpression instead. For more information, see AttributesToGet in the Amazon
//!   DynamoDB Developer Guide.
//! * `ConditionalOperator` (ConditionalOperator, one of: AND | OR) — This is a legacy
//!   parameter. Use FilterExpression instead. For more information, see ConditionalOperator
//!   in the Amazon DynamoDB Developer Guide.
//! * `ConsistentRead` (ConsistentRead) — A Boolean value that determines the read consistency
//!   model during the scan: If ConsistentRead is false, then the data returned from Scan
//!   might not contain the results from other recently completed write operations (PutItem,
//!   UpdateItem, or DeleteItem). If ConsistentRead is true, then all of the write operations
//!   that completed before the Scan began are guaranteed to be contained in the Scan
//!   response. The default setting for ConsistentRead is false. The ConsistentRead parameter
//!   is not supported on global secondary indexes. If you scan a global secondary index with
//!   ConsistentRead set to true, you will receive a ValidationException.
//! * `ExclusiveStartKey` (Key) — The primary key of the first item that this operation will
//!   evaluate. Use the value that was returned for LastEvaluatedKey in the previous
//!   operation. The data type for ExclusiveStartKey must be String, Number or Binary. No set
//!   data types are allowed. In a parallel scan, a Scan request that includes
//!   ExclusiveStartKey must specify the same segment whose previous Scan returned the
//!   corresponding value of LastEvaluatedKey.
//! * `ExpressionAttributeNames` (ExpressionAttributeNameMap) — One or more substitution
//!   tokens for attribute names in an expression. The following are some use cases for using
//!   ExpressionAttributeNames: To access an attribute whose name conflicts with a DynamoDB
//!   reserved word. To create a placeholder for repeating occurrences of an attribute name in
//!   an expression. To prevent special characters in an attribute name from being
//!   misinterpreted in an expression. Use the # character in an expression to dereference an
//!   attribute name. For example, consider the following attribute name: Percentile The name
//!   of this attribute conflicts with a reserved word, so it cannot be used directly in an
//!   expression. (For the complete list of reserved words, see Reserved Words …
//! * `ExpressionAttributeValues` (ExpressionAttributeValueMap) — One or more values that can
//!   be substituted in an expression. Use the : (colon) character in an expression to
//!   dereference an attribute value. For example, suppose that you wanted to check whether
//!   the value of the ProductStatus attribute was one of the following: Available |
//!   Backordered | Discontinued You would first need to specify ExpressionAttributeValues as
//!   follows: { ":avail":{"S":"Available"}, ":back":{"S":"Backordered"},
//!   ":disc":{"S":"Discontinued"} } You could then use these values in an expression, such as
//!   this: ProductStatus IN (:avail, :back, :disc) For more information on expression
//!   attribute values, see Condition Expressions in the Amazon DynamoDB Developer Guide.
//! * `FilterExpression` (ConditionExpression) — A string that contains conditions that
//!   DynamoDB applies after the Scan operation, but before the data is returned to you. Items
//!   that do not satisfy the FilterExpression criteria are not returned. A FilterExpression
//!   is applied after the items have already been read; the process of filtering does not
//!   consume any additional read capacity units. For more information, see Filter Expressions
//!   in the Amazon DynamoDB Developer Guide.
//! * `IndexName` (IndexName, len 3..255, pattern ^[a-zA-Z0-9_.-]+$) — The name of a secondary
//!   index to scan. This index can be any local secondary index or global secondary index.
//!   Note that if you use the IndexName parameter, you must also provide TableName.
//! * `Limit` (PositiveIntegerObject, range 1..) — The maximum number of items to evaluate
//!   (not necessarily the number of matching items). If DynamoDB processes the number of
//!   items up to the limit while processing the results, it stops the operation and returns
//!   the matching values up to that point, and a key in LastEvaluatedKey to apply in a
//!   subsequent operation, so that you can pick up where you left off. Also, if the processed
//!   dataset size exceeds 1 MB before DynamoDB reaches this limit, it stops the operation and
//!   returns the matching values up to the limit, and a key in LastEvaluatedKey to apply in a
//!   subsequent operation to continue the operation. For more information, see Working with
//!   Queries in the Amazon DynamoDB Developer Guide.
//! * `ProjectionExpression` (ProjectionExpression) — A string that identifies one or more
//!   attributes to retrieve from the specified table or index. These attributes can include
//!   scalars, sets, or elements of a JSON document. The attributes in the expression must be
//!   separated by commas. If no attribute names are specified, then all attributes will be
//!   returned. If any of the requested attributes are not found, they will not appear in the
//!   result. For more information, see Specifying Item Attributes in the Amazon DynamoDB
//!   Developer Guide.
//! * `ReturnConsumedCapacity` (ReturnConsumedCapacity, one of: INDEXES | NONE | TOTAL) —
//!   Determines the level of detail about either provisioned or on-demand throughput
//!   consumption that is returned in the response: INDEXES - The response includes the
//!   aggregate ConsumedCapacity for the operation, together with ConsumedCapacity for each
//!   table and secondary index that was accessed. Note that some operations, such as GetItem
//!   and BatchGetItem, do not access any indexes at all. In these cases, specifying INDEXES
//!   will only return ConsumedCapacity information for table(s). TOTAL - The response
//!   includes only the aggregate ConsumedCapacity for the operation. NONE - No
//!   ConsumedCapacity details are included in the response.
//! * `ScanFilter` (FilterConditionMap) — This is a legacy parameter. Use FilterExpression
//!   instead. For more information, see ScanFilter in the Amazon DynamoDB Developer Guide.
//! * `Segment` (ScanSegment, range 0..999999) — For a parallel Scan request, Segment
//!   identifies an individual segment to be scanned by an application worker. Segment IDs are
//!   zero-based, so the first segment is always 0. For example, if you want to use four
//!   application threads to scan a table or an index, then the first thread specifies a
//!   Segment value of 0, the second thread specifies 1, and so on. The value of
//!   LastEvaluatedKey returned from a parallel Scan request must be used as ExclusiveStartKey
//!   with the same segment ID in a subsequent Scan operation. The value for Segment must be
//!   greater than or equal to 0, and less than the value provided for TotalSegments. If you
//!   provide Segment, you must also provide TotalSegments.
//! * `Select` (Select, one of: ALL_ATTRIBUTES | ALL_PROJECTED_ATTRIBUTES | COUNT |
//!   SPECIFIC_ATTRIBUTES) — The attributes to be returned in the result. You can retrieve all
//!   item attributes, specific item attributes, the count of matching items, or in the case
//!   of an index, some or all of the attributes projected into the index. ALL_ATTRIBUTES -
//!   Returns all of the item attributes from the specified table or index. If you query a
//!   local secondary index, then for each matching item in the index, DynamoDB fetches the
//!   entire item from the parent table. If the index is configured to project all item
//!   attributes, then all of the data can be obtained from the local secondary index, and no
//!   fetching is required. ALL_PROJECTED_ATTRIBUTES - Allowed only when querying an index.
//!   Retrieves all attributes that …
//! * `TableName` (TableArn, len 1..1024) **required** — The name of the table containing the
//!   requested items or if you provide IndexName, the name of the table to which that index
//!   belongs. You can also provide the Amazon Resource Name (ARN) of the table in this
//!   parameter.
//! * `TotalSegments` (ScanTotalSegments, range 1..1000000) — For a parallel Scan request,
//!   TotalSegments represents the total number of segments into which the Scan operation will
//!   be divided. The value of TotalSegments corresponds to the number of application workers
//!   that will perform the parallel scan. For example, if you want to use four application
//!   threads to scan a table or an index, specify a TotalSegments value of 4. The value for
//!   TotalSegments must be greater than or equal to 1, and less than or equal to 1000000. If
//!   you specify a TotalSegments value of 1, the Scan operation will be sequential rather
//!   than parallel. If you specify TotalSegments, you must also specify Segment.
//!
//! ## Output
//!
//! * `ConsumedCapacity` (ConsumedCapacity) — The capacity units consumed by the Scan
//!   operation. The data returned includes the total provisioned throughput consumed, along
//!   with statistics for the table and any indexes involved in the operation.
//!   ConsumedCapacity is only returned if the ReturnConsumedCapacity parameter was specified.
//!   For more information, see Capacity unit consumption for read operations in the Amazon
//!   DynamoDB Developer Guide.
//! * `Count` (Integer, default 0) **required** — The number of items in the response. If you
//!   set ScanFilter in the request, then Count is the number of items returned after the
//!   filter was applied, and ScannedCount is the number of matching items before the filter
//!   was applied. If you did not use a filter in the request, then Count is the same as
//!   ScannedCount.
//! * `Items` (ItemList) — An array of item attributes that match the scan criteria. Each
//!   element in this array consists of an attribute name and the value for that attribute.
//! * `LastEvaluatedKey` (Key) — The primary key of the item where the operation stopped,
//!   inclusive of the previous result set. Use this value to start a new operation, excluding
//!   this value in the new request. If LastEvaluatedKey is empty, then the "last page" of
//!   results has been processed and there is no more data to be retrieved. If
//!   LastEvaluatedKey is not empty, it does not necessarily mean that there is more data in
//!   the result set. The only way to know when you have reached the end of the result set is
//!   when LastEvaluatedKey is empty.
//! * `ScannedCount` (Integer, default 0) **required** — The number of items evaluated, before
//!   any ScanFilter is applied. A high ScannedCount value with few, or no, Count results
//!   indicates an inefficient Scan operation. For more information, see Count and
//!   ScannedCount in the Amazon DynamoDB Developer Guide. If you did not use a filter in the
//!   request, then ScannedCount is the same as Count.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `InvalidEndpointException` (HTTP 421)
//! * `ProvisionedThroughputExceededException` (HTTP 400) — The request was denied due to
//!   request throttling. For detailed information about why the request was throttled and the
//!   ARN of the impacted resource, find the ThrottlingReason field in the returned exception.
//!   The Amazon Web Services SDKs for DynamoDB automatically retry requests that receive this
//!   exception. Your request is eventually successful, unless your retry queue is too large
//!   to finish. Reduce the frequency of requests and use exponential backoff. For more
//!   information, go to Error Retries and Exponential Backoff in the Amazon DynamoDB
//!   Developer Guide.
//! * `RequestLimitExceeded` (HTTP 400) — Throughput exceeds the current throughput quota for
//!   your account. For detailed information about why the request was throttled and the ARN
//!   of the impacted resource, find the ThrottlingReason field in the returned exception.
//!   Contact Amazon Web Services Support to request a quota increase.
//! * `ResourceNotFoundException` (HTTP 400) — The operation tried to access a nonexistent
//!   table or index. The resource might not be specified correctly, or its status might not
//!   be ACTIVE.
//! * `ThrottlingException` (HTTP 400) — The request was denied due to request throttling. For
//!   detailed information about why the request was throttled and the ARN of the impacted
//!   resource, find the ThrottlingReason field in the returned exception.

#[allow(unused_imports)]
use super::types::*;

/// The most data one page of a `Scan` response carries, which is the store's own page
/// budget: "If the total size of scanned items exceeds the maximum dataset size limit
/// of 1 MB, the scan completes and results are returned to the user" (the `Scan`
/// doc). A page that stops there reports a `LastEvaluatedKey`, so the caller pages on,
/// and one request cannot amplify into a response the size of a table.
use crate::state::MAX_PAGE_BYTES;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ScanError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ScanError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#Scan")]
#[derive(Debug)]
pub enum ScanErrorModeled {
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    RequestLimitExceeded(RequestLimitExceeded),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::scan::ScanError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ScanError = crate::scaffolded::Scaffolded<ScanErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ScanErrorModeled> for ScanError {
    fn from(modeled: ScanErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `Scan` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct Scan<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> Scan<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for Scan<Eng>
{
    type Input = ScanInput;
    type Output = ScanOutput;
    type Error = ScanError;

    const NAME: &'static str = "Scan";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SCAN_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SCAN_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        let name = crate::state::resolve_table_name_in(&input.table_name, &region).map_err(lift)?;

        // Everything that speaks only about the REQUEST is settled before any table is
        // looked up, so a request that cannot hold together is refused as the client
        // error it is whatever the store holds and whichever members it carries.
        //
        // A request states its intent through the expression members or through the
        // legacy ones they replaced, never through both: the whole of each side is
        // named, and the check runs before either side is read. Each side is listed in
        // the order THIS operation's input shape declares its members
        // (`smithy_query(query="operation", operation="Scan")`), which is the order the
        // sentence states them in ([`crate::state::expression_conflict_between`]).
        crate::state::check_expression_parameter_mix(
            &[
                (
                    "AttributesToGet",
                    input
                        .attributes_to_get
                        .as_ref()
                        .is_some_and(|names| !names.is_empty()),
                ),
                (
                    "ScanFilter",
                    input.scan_filter.as_ref().is_some_and(|map| !map.is_empty()),
                ),
                ("ConditionalOperator", input.conditional_operator.is_some()),
            ],
            &[
                (
                    "ProjectionExpression",
                    input.projection_expression.is_some(),
                ),
                ("FilterExpression", input.filter_expression.is_some()),
            ],
        )
        .map_err(lift)?;
        // "A filter expression is applied after a Scan finishes but before the results
        // are returned" (`Scan$FilterExpression`), stated either in that language or
        // through the legacy map it replaced — the one filter both readers of this
        // service state it through ([`crate::state::ReadFilter`]); it is read BEFORE the
        // store is, so a filter the service would refuse is refused without reading
        // anything.
        let filter = crate::state::ReadFilter::resolve(
            "ScanFilter",
            input.filter_expression.as_deref(),
            input.scan_filter.as_ref(),
            input.conditional_operator.as_deref(),
            crate::state::Substitutions::new(
                input.expression_attribute_names.as_ref(),
                input.expression_attribute_values.as_ref(),
            ),
        )
        .map_err(lift)?;
        let requested = crate::state::requested_projection(
            "ProjectionExpression",
            input.projection_expression.as_deref(),
            input.attributes_to_get.as_deref(),
            input.expression_attribute_names.as_ref(),
        )
        .map_err(lift)?;
        // Every token the request DEFINES must be one an expression uses, and a
        // substitution table the request has no expression to substitute into is refused
        // rather than silently ignored: both rules belong to the one checker every
        // operation with expression members shares
        // ([`crate::state::Substitutions::check_all_referenced`]).
        crate::state::Substitutions::new(
            input.expression_attribute_names.as_ref(),
            input.expression_attribute_values.as_ref(),
        )
        .check_all_referenced(&[
            ("FilterExpression", input.filter_expression.as_deref()),
            ("ProjectionExpression", input.projection_expression.as_deref()),
        ])
        .map_err(lift)?;
        // A value a filter compares against is held to the documented rules for its
        // type, exactly as a value a write stores is: the rules belong to the value,
        // not to the surface it arrived through.
        for (placeholder, value) in input.expression_attribute_values.iter().flatten() {
            crate::state::check_bare_attribute_value(placeholder, value).map_err(lift)?;
        }
        // Which member names the index is a fact of the request, so what each reported
        // item carries is decided here too: the defaults differ between a table scan
        // and an index scan ([`resolve_selection`]).
        let selection = crate::state::resolve_selection(
            input.select.as_deref(),
            crate::state::stated_projection_member(
                input.projection_expression.as_deref(),
                input.attributes_to_get.as_deref(),
            ),
            requested,
            input.index_name.is_some(),
        )
        .map_err(lift)?;
        let segmentation = Segmentation::resolve(input.segment, input.total_segments)
            .map_err(crate::scaffolded::Scaffolded::validation)?;
        let consistent_read = input.consistent_read.unwrap_or(false);

        // WHO this request speaks for, resolved before this service's store is locked
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        let mut store = state.lock();
        store.apply_due_transitions(now);
        let table = store.active_table(&region, &name, now).map_err(lift)?;
        // A table the policies in force do not let this caller read answers the denial,
        // not its items: it is decided before anything is read.
        if let Some(denial) = crate::state::policy_denial(&caller, table, "dynamodb:Scan", now) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }
        // The items are stored ENCRYPTED under the table's KMS key, so a key KMS will not
        // use leaves nothing this scan could decrypt: the KMS fault is reported rather
        // than a page of items ([`crate::state::check_table_key_accessible`]).
        crate::state::check_table_key_accessible(&*self.engine, table).map_err(lift)?;
        // Which index this scan reads, with the two refusals that resolution carries —
        // all three facts of the request, decided before anything is read
        // ([`crate::state::resolve_index`]).
        let index = crate::state::resolve_index(
            table,
            input.index_name.as_deref(),
            consistent_read,
            &selection,
        )
        .map_err(lift)?;
        // "If you query or scan a global secondary index, you can only request
        // attributes that are projected into the index" (`Scan$Select`): a scan whose
        // projection names an attribute the index does not hold cannot be served from
        // the index, so it is refused rather than answered with a narrowed item. Its
        // FILTER is judged against the entry the index holds, so an attribute only the
        // filter names simply matches nothing
        // ([`crate::state::unprojected_projection_attributes`]).
        if let Some(index) = index.as_ref() {
            let unprojected = crate::state::unprojected_projection_attributes(
                index,
                &table.key_schema,
                &selection,
            );
            if !unprojected.is_empty() {
                return Err(lift(crate::state::unprojected_projection_refusal(
                    index.name,
                    &unprojected,
                )));
            }
        }
        // "If any of the requested attributes are not projected into the local secondary
        // index, DynamoDB fetches each of these attributes from the parent table"
        // (`Scan$Select`) — which is also what makes that read cost more, since what it
        // read is then the whole item rather than the index's entry.
        let fetches_from_table = index.as_ref().is_some_and(|index| {
            !index.global
                && crate::state::needs_unprojected(
                    index,
                    &table.key_schema,
                    &selection,
                    &filter.attributes(),
                )
        });

        // The walk is over what this scan READS, in the order that reading has: the
        // table's items in primary-key order, or the entries one secondary index holds
        // in the order the index orders them ([`crate::state::IndexItemKey`]). Either
        // way it is the order a `LastEvaluatedKey` resumes from. The list is one entry
        // per STORED item, so its size is what the table HOLDS and not anything this
        // request chose; the entry each row is read as is built inside the page below,
        // where the page budget bounds it.
        let candidates: Vec<(crate::state::ReadPosition<'_>, &crate::state::Item)> = match index {
            None => table
                .items
                .iter()
                .map(|(key, item)| (crate::state::ReadPosition::Table(key), item))
                .collect(),
            Some(index) => {
                // An item the index does not cover has no entry to scan at all — a
                // sparse index holds only the items that carry its key attributes
                // ([`crate::state::Table::index_item_key`]).
                let mut entries: Vec<(crate::state::IndexItemKey, &crate::state::Item)> = table
                    .items
                    .iter()
                    .filter_map(|(key, item)| {
                        table
                            .index_item_key(&index, key, item)
                            .map(|position| (position, item))
                    })
                    .collect();
                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                entries
                    .into_iter()
                    .map(|(position, item)| (crate::state::ReadPosition::Index(position), item))
                    .collect()
            }
        };
        // Where a previous page stopped, read back through the schema this scan walks
        // ([`crate::state::resume_from`]).
        let resumed = crate::state::resume_from(
            table,
            index.as_ref(),
            input.exclusive_start_key.as_ref(),
        )
        .map_err(lift)?;

        // Which PARTITION of the walked key space each row sits in, which is what the
        // segments of a parallel scan are shared out by ([`Partitions`]).
        let partitions = Partitions::of(&candidates, index.as_ref());
        // Segment membership is decided from the row's partition in that total order,
        // NOT from the position within one page, so a later page of the same segment
        // sees the same share of what is scanned.
        let evaluated = candidates
            .iter()
            .enumerate()
            .filter(|(position, _)| partitions.owned_by(*position, segmentation))
            .map(|(_, row)| row)
            // "Use this value to start a new operation, EXCLUDING this value in the
            // new request" (`Scan$LastEvaluatedKey`). A scan always walks forward.
            .filter(|(position, _)| {
                resumed
                    .as_ref()
                    .is_none_or(|from| position.past(from, true))
            });

        // "A single Scan operation first reads up to the maximum number of items set
        // (if using the Limit parameter) or a maximum of 1 MB of data" (the `Scan`
        // doc), so the page — and the work this request can ask for — is bounded
        // BEFORE any item is cloned, whether or not the caller named a `Limit`.
        let limit = input
            .limit
            .map_or(usize::MAX, |limit| usize::try_from(limit).unwrap_or(0));
        let mut page: Vec<crate::state::Read<'_>> = Vec::new();
        let mut page_size: i64 = 0;
        let mut truncated = false;
        for (_, item) in evaluated {
            let read = crate::state::Read::new(
                item,
                index.as_ref(),
                &table.key_schema,
                fetches_from_table,
            );
            let size = crate::state::item_size(read.seen());
            // The first row always fits, the way DynamoDB always returns at least
            // one item it read; after that the page stops BEFORE the row that would
            // carry it past the budget, so the response is bounded BY the budget
            // rather than by the budget plus one item.
            if page.len() >= limit
                || (!page.is_empty() && page_size.saturating_add(size) > MAX_PAGE_BYTES)
            {
                truncated = true;
                break;
            }
            page_size = page_size.saturating_add(size);
            page.push(read);
        }

        // "The primary key of the item where the operation stopped" — reported when
        // the walk stopped with items left to evaluate; "If LastEvaluatedKey is
        // empty, then the 'last page' of results has been processed". A page that
        // ends exactly on the last row reports none, which the member's own doc allows
        // ("If LastEvaluatedKey is not empty, it does not necessarily mean that there
        // is more data in the result set") and `Query` answers alike. An index scan
        // reports the index's key beside the table's, because that pair is what
        // addresses one entry of an index.
        let last_evaluated_key = truncated
            .then(|| page.last())
            .flatten()
            .map(|read| crate::state::evaluated_key(table, index.as_ref(), read.item()));
        // "A filter expression is applied after a Scan finishes": every row the page
        // READ is scanned — and charged for — and the ones the filter does not match
        // are left out of the response. "ScannedCount [is] the number of items
        // evaluated, before any ScanFilter is applied"; with no filter, "Count is the
        // same as ScannedCount".
        let scanned_count = i32::try_from(page.len()).unwrap_or(i32::MAX);
        let mut matched: Vec<&crate::state::Read<'_>> = Vec::with_capacity(page.len());
        for read in &page {
            if filter.matches(read.seen()).map_err(lift)? {
                matched.push(read);
            }
        }
        let count = i32::try_from(matched.len()).unwrap_or(i32::MAX);
        // The projection is applied AFTER the filter, so an attribute the response does
        // not carry still narrows the scan.
        let items = crate::state::reported_items(&selection, &matched);

        let units = crate::state::read_capacity_units(page_size, consistent_read);
        let requested_capacity = input.return_consumed_capacity.as_deref();
        // Projected while the index this scan read is still borrowed from the table, so
        // what the response bills is decided before the table is reached for again.
        let consumed_capacity = crate::state::consumed_read_capacity(
            requested_capacity,
            &name,
            index.as_ref(),
            units,
        );
        // The read is ADMITTED against the read capacity the table is provisioned for:
        // the units it consumes are the ones this response bills — every item the page
        // scanned, filtered or not — and a read past that capacity is refused rather
        // than served. A read served from an index spends the TABLE's provisioned
        // capacity here, because an index of a provisioned table has no capacity of its
        // own in this store.
        if let Err(throttle) = store
            .active_table_mut(&region, &name, now)
            .map_err(lift)?
            .admit_read(units, now)
        {
            return Err(ScanErrorModeled::ProvisionedThroughputExceededException(
                crate::state::read_throughput_exceeded(
                    &crate::state::table_arn(&region, &name),
                    throttle,
                ),
            )
            .into());
        }
        Ok(ScanOutput {
            items,
            count,
            scanned_count,
            last_evaluated_key,
            consumed_capacity,
        })
    }
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> ScanError {
    ScanErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> ScanError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            ScanErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}

/// How a parallel scan whose `Segment` is not inside its `TotalSegments` is refused:
/// "The value for Segment must be greater than or equal to 0, and less than the value
/// provided for TotalSegments" (`Scan$Segment`). The sentence is FIXED — it names the
/// two members the rule is about and reports neither value, so every client that
/// matches this refusal matches one string whatever it sent.
const SEGMENT_NOT_BELOW_TOTAL_SEGMENTS_MESSAGE: &str =
    "The Segment parameter is zero-based and must be less than parameter TotalSegments";

/// Which share of the table one `Scan` request evaluates.
///
/// "TotalSegments represents the total number of segments into which the Scan
/// operation will be divided ... Segment identifies an individual segment to be
/// scanned by an application worker" (`Scan$TotalSegments` / `Scan$Segment`). What is
/// divided is the table's or index's KEY SPACE, so a segment is a block of PARTITIONS
/// ([`Partitions`]): the segments together cover every item exactly once, no two of them
/// cover the same item, and every item sharing a partition key is evaluated by one
/// segment. A request that names neither member is the sequential scan: one segment,
/// holding everything.
#[derive(Debug, Clone, Copy)]
struct Segmentation {
    segment: usize,
    total: usize,
}

impl Segmentation {
    /// Bind the request's `Segment`/`TotalSegments` pair, which is all-or-nothing:
    /// "If you provide Segment, you must also provide TotalSegments" and "If you
    /// specify TotalSegments, you must also specify Segment", with "the value for
    /// Segment ... greater than or equal to 0, and less than the value provided for
    /// TotalSegments".
    fn resolve(segment: Option<i32>, total: Option<i32>) -> Result<Self, String> {
        match (segment, total) {
            (None, None) => Ok(Self {
                segment: 0,
                total: 1,
            }),
            (Some(_), None) => Err(
                "The TotalSegments parameter is required but was not present in the request when \
                 Segment parameter is present"
                    .to_string(),
            ),
            (None, Some(_)) => Err(
                "The Segment parameter is required but was not present in the request when \
                 parameter TotalSegments is present"
                    .to_string(),
            ),
            (Some(segment), Some(total)) => {
                if segment >= total {
                    return Err(SEGMENT_NOT_BELOW_TOTAL_SEGMENTS_MESSAGE.to_string());
                }
                // Both members' modelled ranges are non-negative, and they are
                // checked before this handler runs, so the conversions cannot fail.
                Ok(Self {
                    segment: usize::try_from(segment).unwrap_or(0),
                    total: usize::try_from(total).unwrap_or(1).max(1),
                })
            }
        }
    }

    /// Whether the partition at `partition` of the `partitions` the walk covers belongs
    /// to this request's segment.
    ///
    /// The segments divide the key space into as many CONTIGUOUS blocks of partitions as
    /// there are segments, so a request that names fewer partitions than segments leaves
    /// the trailing segments with nothing to evaluate, and the blocks together cover
    /// every partition exactly once.
    fn owns(&self, partition: usize, partitions: usize) -> bool {
        if self.total <= 1 {
            return true;
        }
        if partitions == 0 {
            return false;
        }
        partition.saturating_mul(self.total) / partitions == self.segment
    }
}

/// Which PARTITION of the walked key space each row of a scan's walk sits in.
///
/// A parallel scan divides the TABLE OR INDEX, not the row list: "DynamoDB divides the
/// table or index into TotalSegments segments" and a page of one segment resumes from
/// "the same segment whose previous Scan returned the corresponding value of
/// LastEvaluatedKey" (`Scan$ExclusiveStartKey`) — a segment is therefore a block of the
/// KEY SPACE, which keeps every row that shares a partition key in one segment and makes
/// a segment's share a contiguous run of the walk. Built once per request from the walk
/// this scan reads ([`crate::state::ReadPosition::partition`]), in that walk's order, so
/// every page of a segment sees the same share.
#[derive(Debug)]
struct Partitions {
    /// One entry per row of the walk, in walk order: which partition of the key space
    /// that row falls in, numbered from 0 in walk order.
    of_row: Vec<usize>,
    /// How many distinct partitions the walk covers.
    count: usize,
}

impl Partitions {
    /// Number the partitions the walk covers. Rows sharing a partition key are
    /// contiguous in either walk — the table's items are ordered partition-first
    /// ([`crate::state::ItemKey`]) and an index's entries by the index key
    /// ([`crate::state::IndexItemKey`]) — so one pass over the walk numbers them.
    fn of(
        candidates: &[(crate::state::ReadPosition<'_>, &crate::state::Item)],
        index: Option<&crate::state::SecondaryIndex<'_>>,
    ) -> Self {
        let mut of_row: Vec<usize> = Vec::with_capacity(candidates.len());
        let mut count = 0usize;
        let mut previous: Option<&[crate::state::AttributeValueKey]> = None;
        for (position, _) in candidates {
            let partition = position.partition(index);
            if previous.is_some_and(|previous| previous != partition) {
                count = count.saturating_add(1);
            }
            of_row.push(count);
            previous = Some(partition);
        }
        Self {
            of_row,
            count: if candidates.is_empty() {
                0
            } else {
                count.saturating_add(1)
            },
        }
    }

    /// Whether the row at `position` in the walk falls in `segmentation`'s share of the
    /// key space.
    fn owned_by(&self, position: usize, segmentation: Segmentation) -> bool {
        segmentation.owns(self.of_row.get(position).copied().unwrap_or(0), self.count)
    }
}
