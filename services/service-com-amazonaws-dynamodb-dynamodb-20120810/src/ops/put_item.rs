// SPDX-License-Identifier: CC0-1.0

//! @op PutItem
//! IMPLEMENTED: writes the item into the stored table, keyed by the primary key
//! extracted from it, so a put over an existing key REPLACES that item. `Item` must
//! carry every key attribute the table's `KeySchema` declares, with the type its
//! `AttributeDefinitions` declare; an attribute that is part of a SECONDARY-INDEX key
//! schema must carry its declared type too ("If you specify any attributes that are
//! part of an index key, then the data types for those attributes must match those of
//! the schema in the table's attribute definition" — `PutItem$Item`) and must be
//! non-empty ("Attribute values of type String and Binary must have a length greater
//! than zero if the attribute is used as a key attribute for a table or index"), and a
//! violation of either is a `ValidationException` naming the index key and the index.
//! `ReturnValues=ALL_OLD` reports the replaced item.
//! An unknown — or not yet `ACTIVE` — table is `ResourceNotFoundException`.
//!
//! A put that LANDS on a table whose stream is enabled publishes one record on that
//! stream before this call answers — `INSERT` for a key that held no item, `MODIFY`
//! for one that did — through [`crate::state::publish_item_changes`], the seam every
//! item-write path in this crate hands its changes to.
//!
//! Every number the item carries is stored CANONICALLY — "DynamoDB treats them as
//! number type attributes" (`AttributeValue$N`), so the store keeps the number each
//! `N` denotes, not the lexeme that spelled it, whether it sits at the top level,
//! inside an `L`/`M` at any depth, or in an `NS`
//! ([`crate::state::canonicalize_item`]). A set that holds one value twice once its
//! members are canonical is refused with a `ValidationException`, and so is an `N`
//! that denotes no number.
//!
//! Conditional puts are served: `ConditionExpression` (or the legacy
//! `Expected`/`ConditionalOperator`) is read against the item this put would replace,
//! and an unmet condition is `ConditionalCheckFailedException`, carrying that item when
//! `ReturnValuesOnConditionCheckFailure=ALL_OLD`. The expression language lives in
//! [`crate::state`], shared with `UpdateItem` and `DeleteItem`, and every substitution
//! token the request defines must be referenced by one of its expressions
//! ([`crate::state::Substitutions::check_all_referenced`]).
//!
//! The write is then ADMITTED against the capacity the table is provisioned for and the
//! per-partition-key ceiling beneath it ([`crate::state::Table::admit_item_write`]): a
//! write neither covers is `ProvisionedThroughputExceededException`, carrying the
//! `ThrottlingReasons` entry of the ceiling that bound and the table's ARN, and stores
//! nothing.
//!
//! `ReturnConsumedCapacity=INDEXES` reports the capacity the write consumed on the table
//! AND on each secondary index whose stored entry this write CHANGES, and
//! `ReturnItemCollectionMetrics=SIZE` reports the item collection the write landed in —
//! both computed from the stored table by [`crate::state::Table::write_capacity`] and
//! [`crate::state::Table::item_collection_metrics`], so an index the item does not reach,
//! an index whose entry the put leaves exactly as it stood, and a table with no local
//! secondary index report nothing.
//!
//! Sizes a request can choose are bounded before the write lands: the item's own size
//! (the documented 400 KB limit, [`crate::state::MAX_ITEM_SIZE_BYTES`]) and its nesting
//! (the documented 32 levels). The service puts no ceiling on how much a table HOLDS, so
//! a legal item is stored however much data the table already carries.
//!
//! The VALUES a request carries are held to the documented rules for their type before
//! the write lands too: a key attribute's size and emptiness
//! ([`crate::state::Table::check_key_limits`], the documented 2048/1024-byte key
//! limits), and every value's own rules anywhere in the item
//! ([`crate::state::canonicalize_item`] — non-empty sets with distinct members,
//! numbers inside the documented precision and exponent range). A table whose
//! resource-based policy denies the caller `dynamodb:PutItem` is answered
//! `AccessDeniedException` instead of written to.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `PutItem::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `PutItem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json")
//! ```
//!
//! Creates a new item, or replaces an old item with a new item. If an item that has the
//! same primary key as the new item already exists in the specified table, the new item
//! completely replaces the existing item. You can perform a conditional put operation (add
//! a new item if one with the specified primary key doesn't exist), or replace an existing
//! item if it has certain attribute values. You can return the item's attribute values in
//! the same operation, using the ReturnValues parameter. When you add an item, the primary
//! key attributes are the only required attributes. Empty String and Binary attribute
//! values are allowed. Attribute values of type String and Binary must have a length
//! greater than zero if the attribute is used as a key attribute for a table or index. Set
//! type attributes cannot be empty. Invalid Requests with empty values will be rejected
//! with a ValidationException exception. To prevent a new item from replacing an existing
//! item, use a conditional expression that contains the attribute_not_exists function with
//! the name of the attribute being used as the partition key for the table. Since every
//! record must contain that attribute, the attribute_not_exists function will only succeed
//! if no matching item exists. To determine whether PutItem overwrote an existing item, use
//! ReturnValues set to ALL_OLD. If the response includes the Attributes element, an
//! existing item was overwritten. For more information about PutItem, see Working with
//! Items in the Amazon DynamoDB Developer Guide.
//!
//! ## Input
//!
//! * `ConditionExpression` (ConditionExpression) — A condition that must be satisfied in
//!   order for a conditional PutItem operation to succeed. An expression can contain any of
//!   the following: Functions: attribute_exists | attribute_not_exists | attribute_type |
//!   contains | begins_with | size These function names are case-sensitive. Comparison
//!   operators: = | <> | < | > | <= | >= | BETWEEN | IN Logical operators: AND | OR | NOT For
//!   more information on condition expressions, see Condition Expressions in the Amazon
//!   DynamoDB Developer Guide.
//! * `ConditionalOperator` (ConditionalOperator, one of: AND | OR) — This is a legacy
//!   parameter. Use ConditionExpression instead. For more information, see
//!   ConditionalOperator in the Amazon DynamoDB Developer Guide.
//! * `Expected` (ExpectedAttributeMap) — This is a legacy parameter. Use ConditionExpression
//!   instead. For more information, see Expected in the Amazon DynamoDB Developer Guide.
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
//! * `Item` (PutItemInputAttributeMap) **required** — A map of attribute name/value pairs,
//!   one for each attribute. Only the primary key attributes are required; you can optionally
//!   provide other attribute name-value pairs for the item. You must provide all of the
//!   attributes for the primary key. For example, with a simple primary key, you only need to
//!   provide a value for the partition key. For a composite primary key, you must provide
//!   both values for both the partition key and the sort key. If you specify any attributes
//!   that are part of an index key, then the data types for those attributes must match those
//!   of the schema in the table's attribute definition. If the table has vector indexes, the
//!   following validations apply to write operations. A …
//! * `ReturnConsumedCapacity` (ReturnConsumedCapacity, one of: INDEXES | NONE | TOTAL) —
//!   Determines the level of detail about either provisioned or on-demand throughput
//!   consumption that is returned in the response: INDEXES - The response includes the
//!   aggregate ConsumedCapacity for the operation, together with ConsumedCapacity for each
//!   table and secondary index that was accessed. Note that some operations, such as GetItem
//!   and BatchGetItem, do not access any indexes at all. In these cases, specifying INDEXES
//!   will only return ConsumedCapacity information for table(s). TOTAL - The response
//!   includes only the aggregate ConsumedCapacity for the operation. NONE - No
//!   ConsumedCapacity details are included in the response.
//! * `ReturnItemCollectionMetrics` (ReturnItemCollectionMetrics, one of: NONE | SIZE) —
//!   Determines whether item collection metrics are returned. If set to SIZE, the response
//!   includes statistics about item collections, if any, that were modified during the
//!   operation are returned in the response. If set to NONE (the default), no statistics are
//!   returned.
//! * `ReturnValues` (ReturnValue, one of: ALL_NEW | ALL_OLD | NONE | UPDATED_NEW |
//!   UPDATED_OLD) — Use ReturnValues if you want to get the item attributes as they appeared
//!   before they were updated with the PutItem request. For PutItem, the valid values are:
//!   NONE - If ReturnValues is not specified, or if its value is NONE, then nothing is
//!   returned. (This setting is the default for ReturnValues.) ALL_OLD - If PutItem overwrote
//!   an attribute name-value pair, then the content of the old item is returned. The values
//!   returned are strongly consistent. There is no additional cost associated with requesting
//!   a return value aside from the small network and processing overhead of receiving a
//!   larger response. No read capacity units are consumed. The ReturnValues parameter is used
//!   by several DynamoDB …
//! * `ReturnValuesOnConditionCheckFailure` (ReturnValuesOnConditionCheckFailure, one of:
//!   ALL_OLD | NONE) — An optional parameter that returns the item attributes for a PutItem
//!   operation that failed a condition check. There is no additional cost associated with
//!   requesting a return value aside from the small network and processing overhead of
//!   receiving a larger response. No read capacity units are consumed.
//! * `TableName` (TableArn, len 1..1024) **required** — The name of the table to contain the
//!   item. You can also provide the Amazon Resource Name (ARN) of the table in this
//!   parameter.
//!
//! ## Output
//!
//! * `Attributes` (AttributeMap) — The attribute values as they appeared before the PutItem
//!   operation, but only if ReturnValues is specified as ALL_OLD in the request. Each element
//!   consists of an attribute name and an attribute value.
//! * `ConsumedCapacity` (ConsumedCapacity) — The capacity units consumed by the PutItem
//!   operation. The data returned includes the total provisioned throughput consumed, along
//!   with statistics for the table and any indexes involved in the operation.
//!   ConsumedCapacity is only returned if the ReturnConsumedCapacity parameter was specified.
//!   For more information, see Capacity unity consumption for write operations in the Amazon
//!   DynamoDB Developer Guide. If the table has vector indexes, the response includes a
//!   VectorIndexes field with VectorWriteRequestBytes consumed for each affected vector
//!   index.
//! * `ItemCollectionMetrics` (ItemCollectionMetrics) — Information about item collections, if
//!   any, that were affected by the PutItem operation. ItemCollectionMetrics is only returned
//!   if the ReturnItemCollectionMetrics parameter was specified. If the table does not have
//!   any local secondary indexes, this information is not returned in the response. Each
//!   ItemCollectionMetrics element consists of: ItemCollectionKey - The partition key value
//!   of the item collection. This is the same as the partition key value of the item itself.
//!   SizeEstimateRangeGB - An estimate of item collection size, in gigabytes. This value is a
//!   two-element array containing a lower bound and an upper bound for the estimate. The
//!   estimate includes the size of all the items in the …
//!
//! ## Errors
//!
//! * `ConditionalCheckFailedException` (HTTP 400) — A condition specified in the operation
//!   failed to be evaluated.
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `InvalidEndpointException` (HTTP 421)
//! * `ItemCollectionSizeLimitExceededException` (HTTP 400) — An item collection is too large.
//!   This exception is only returned for tables that have one or more local secondary
//!   indexes.
//! * `ProvisionedThroughputExceededException` (HTTP 400) — The request was denied due to
//!   request throttling. For detailed information about why the request was throttled and the
//!   ARN of the impacted resource, find the ThrottlingReason field in the returned exception.
//!   The Amazon Web Services SDKs for DynamoDB automatically retry requests that receive this
//!   exception. Your request is eventually successful, unless your retry queue is too large
//!   to finish. Reduce the frequency of requests and use exponential backoff. For more
//!   information, go to Error Retries and Exponential Backoff in the Amazon DynamoDB
//!   Developer Guide.
//! * `ReplicatedWriteConflictException` (HTTP 400) — The request was rejected because one or
//!   more items in the request are being modified by a request in another Region.
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
//! * `TransactionConflictException` (HTTP 400) — Operation was rejected because there is an
//!   ongoing transaction for the item.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutItemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutItemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.dynamodb#PutItem")]
#[derive(Debug)]
pub enum PutItemErrorModeled {
    ConditionalCheckFailedException(ConditionalCheckFailedException),
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    ItemCollectionSizeLimitExceededException(ItemCollectionSizeLimitExceededException),
    ProvisionedThroughputExceededException(ProvisionedThroughputExceededException),
    ReplicatedWriteConflictException(ReplicatedWriteConflictException),
    RequestLimitExceeded(RequestLimitExceeded),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    TransactionConflictException(TransactionConflictException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_item::PutItemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutItemError = crate::scaffolded::Scaffolded<PutItemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutItemErrorModeled> for PutItemError {
    fn from(modeled: PutItemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutItem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutItem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutItem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutItem<Eng>
{
    type Input = PutItemInput;
    type Output = PutItemOutput;
    type Error = PutItemError;

    const NAME: &'static str = "PutItem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_ITEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_ITEM_OUTPUT_SCHEMA;
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

        if input.return_values_on_condition_check_failure.is_some()
            && !matches!(
                input.return_values_on_condition_check_failure.as_deref(),
                Some("NONE" | "ALL_OLD")
            )
        {
            return Err(crate::scaffolded::Scaffolded::validation(
                "ReturnValuesOnConditionCheckFailure can only be ALL_OLD or NONE",
            ));
        }
        let return_old_on_failure =
            input.return_values_on_condition_check_failure.as_deref() == Some("ALL_OLD");
        let substitutions = crate::state::Substitutions::new(
            input.expression_attribute_names.as_ref(),
            input.expression_attribute_values.as_ref(),
        );
        // A request states its intent through the expression members or through the
        // legacy ones they replaced, never through both.
        crate::state::check_expression_parameter_mix(
            &[
                (
                    "Expected",
                    input.expected.as_ref().is_some_and(|map| !map.is_empty()),
                ),
                ("ConditionalOperator", input.conditional_operator.is_some()),
            ],
            &[("ConditionExpression", input.condition_expression.is_some())],
        )
        .map_err(lift)?;
        // Every token the request DEFINES must be one an expression uses: an unused
        // entry of either substitution table makes the whole request invalid.
        substitutions
            .check_all_referenced(&[(
                "ConditionExpression",
                input.condition_expression.as_deref(),
            )])
            .map_err(lift)?;
        // "Use ReturnValues if you want to get the item attributes as they appeared
        // before they were updated": PutItem accepts only NONE and ALL_OLD
        // (`PutItem$ReturnValues`).
        let return_old = match input.return_values.as_deref() {
            None | Some("NONE") => false,
            Some("ALL_OLD") => true,
            Some(_) => {
                return Err(lift(
                    crate::state::return_values_must_be_old_or_none(),
                ))
            }
        };

        let report_item_collection = crate::state::returns_item_collection_metrics(
            input.return_item_collection_metrics.as_deref(),
        )
        .map_err(lift)?;

        // Nesting and collection cardinality are refused before the item is sized or
        // stored: "DynamoDB supports nested attributes up to 32 levels deep", plus this
        // emulator's own ceiling on the members one set, list or map may carry.
        for (attribute, value) in &input.item {
            crate::state::check_attribute_value(attribute, value).map_err(lift)?;
        }
        // A value the condition compares against is held to the same limits, for the
        // same reason: both terms bound the work this request can ask for. All THREE
        // places one request carries a value are bounded here — the item, the values an
        // expression binds, and the values the legacy `Expected` map compares against —
        // so none of them reaches the walk that reads it unbounded.
        for (placeholder, value) in input.expression_attribute_values.iter().flatten() {
            crate::state::check_attribute_value(placeholder, value).map_err(lift)?;
        }
        crate::state::check_expected_condition_values(input.expected.as_ref()).map_err(lift)?;

        // Numbers are canonicalized BEFORE the item is sized, keyed or stored, so
        // every figure reported below — the capacity billed, the item-collection key
        // echoed — is computed from the item as STORED rather than from the lexemes
        // the request spelled. Like the depth check above and the size check below it
        // is request validation, so it precedes the table lookup; the store
        // canonicalizes again at its write site, which is idempotent, so no other
        // write path can land a non-canonical number.
        let mut item = input.item;
        crate::state::canonicalize_item(&mut item).map_err(lift)?;

        let size = crate::state::item_size(&item);
        if size > crate::state::MAX_ITEM_SIZE_BYTES {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::ITEM_SIZE_EXCEEDED_MESSAGE,
            ));
        }

        // Every value the item carries is held to the documented rules for its type as
        // it is canonicalized above (set emptiness and duplicate members, numeric
        // precision and range), BEFORE any table is looked up, because none of those
        // rules depends on the table.

        // The condition this write states is read for every fact of the REQUEST it
        // carries — the members it states the condition through, the expression's own
        // grammar, and what each legacy entry expects — before the table is looked up,
        // because none of that depends on the table either: a request that cannot be
        // served however the store looks is refused as the malformed request it is
        // rather than reported as a missing table.
        crate::state::check_write_condition(
            input.condition_expression.as_deref(),
            input.expected.as_ref(),
            input.conditional_operator.as_deref(),
            substitutions,
        )
        .map_err(lift)?;

        // WHO this request speaks for, resolved before this service's store is locked
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        let mut store = state.lock();
        store.apply_due_transitions(now);
        let key = {
            let table = store.active_table(&region, &name, now).map_err(lift)?;
            // A table the policies in force do not let this caller write is not written
            // to at all: authorization is decided before the item is keyed, against the
            // partition the item this put addresses belongs to.
            if let Some(denial) = crate::state::authorization::denial_addressing(
                &caller,
                table,
                "dynamodb:PutItem",
                now,
                &crate::state::authorization::AddressedKeys::of_maps(table, [&item]),
            ) {
                return Err(crate::scaffolded::Scaffolded::access_denied(denial));
            }
            // The item would be stored ENCRYPTED under the table's KMS key, so a key KMS
            // will not use leaves nothing this write could be encrypted with: the KMS
            // fault is reported and NOTHING is written
            // ([`crate::state::check_table_key_accessible`]).
            crate::state::check_table_key_accessible(&*self.engine, table).map_err(lift)?;
            // The key attributes are bounded by the documented key-value limits, for
            // this table's own key schema (inside `key_of_item`) and for every index
            // key the item would be projected into — which is also where an index key
            // whose type is not the one the table declared for it, or whose `S`/`B`
            // value is empty, is refused.
            table.check_index_keys(&item).map_err(lift)?;
            table.key_of_item(&item).map_err(lift)?
        };
        // The condition is read against the item this put would replace, which is the
        // only state it can speak about ("replace an existing item if it has certain
        // attribute values").
        let replaced_before = store
            .active_table(&region, &name, now)
            .map_err(lift)?
            .items
            .get(&key)
            .cloned();
        let satisfied = crate::state::write_condition_satisfied(
            input.condition_expression.as_deref(),
            input.expected.as_ref(),
            input.conditional_operator.as_deref(),
            substitutions,
            replaced_before.as_ref(),
        )
        .map_err(lift)?;
        if !satisfied {
            return Err(PutItemErrorModeled::ConditionalCheckFailedException(
                ConditionalCheckFailedException {
                    message: Some(crate::state::CONDITION_FAILED_MESSAGE.to_string()),
                    item: if return_old_on_failure {
                        replaced_before
                    } else {
                        None
                    },
                },
            )
            .into());
        }
        // The write is ADMITTED before it lands: the capacity it would consume is
        // computed from the item it would replace and the item it would store — the same
        // two images the response bills for — and held to the table's provisioned write
        // capacity and to the per-partition-key ceiling beneath it. A write neither
        // covers is throttled, and a throttled write leaves nothing behind.
        let (capacity, throttle) = {
            let table = store
                .active_table_mut(&region, &name, now)
                .map_err(lift)?;
            let capacity = table.write_capacity(replaced_before.as_ref(), Some(&item));
            let throttle = table
                .admit_item_write(&key.partition, &capacity, now)
                .err();
            (capacity, throttle)
        };
        if let Some(throttle) = throttle {
            // The key activity of a write refused for exceeding the throughput the table is
            // provisioned for, or the ceiling beneath it: the key it addressed was THROTTLED,
            // which is what a throttled-keys insight rule collects
            // ([`crate::state::contributor_insights`]). A write that LANDS is attributed
            // through the change it published instead.
            crate::state::contributor_insights::record_key_access(
                &*self.engine,
                &store,
                &region,
                &name,
                &item,
                crate::state::contributor_insights::Access::Throttled,
                now,
            );
            return Err(PutItemErrorModeled::ProvisionedThroughputExceededException(
                crate::state::throughput_exceeded(
                    &crate::state::table_arn(&region, &name),
                    throttle,
                ),
            )
            .into());
        }
        // The store enforces its own budgets here: the only growth site this
        // operation has is the item it inserts.
        let replaced = store
            .put_item(&region, &name, now, key, item.clone())
            .map_err(lift)?;

        // Both figures are read back from the table the write landed in, so the capacity
        // fan-out and the item-collection size are computed from stored state rather
        // than from the request alone.
        let table = store.active_table(&region, &name, now).map_err(lift)?;
        let item_collection_metrics =
            report_item_collection.then(|| table.item_collection_metrics(&item));
        // The change this write made reaches the table's stream before the response
        // does, so a consumer that reads the stream after seeing this answer sees it.
        crate::state::publish_item_changes(&*self.engine, &mut store, now);

        Ok(PutItemOutput {
            attributes: if return_old { replaced } else { None },
            consumed_capacity: crate::state::consumed_write_capacity(
                input.return_consumed_capacity.as_deref(),
                &name,
                &capacity,
                crate::state::CapacityKind::Write,
            ),
            item_collection_metrics: item_collection_metrics.flatten(),
        })
    }
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> PutItemError {
    PutItemErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> PutItemError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            PutItemErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}
