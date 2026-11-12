// SPDX-License-Identifier: CC0-1.0

//! How this service states a request the MODEL's own constraints refuse, end to end over the
//! wire.
//!
//! RULE: a value one of the model's constraints does not admit is refused HTTP 400
//! `ValidationException` with AWS's envelope — `<N> validation error(s) detected: ` over one
//! clause per broken constraint — and a clause names the member's dotted path, the
//! requirement it failed, and the value AS SUBMITTED: in full, with nothing elided or
//! annotated. Three parts of that sentence are the service's own: a `@pattern` is quoted
//! WITHOUT the `^`/`$` pair a model writes a whole-value match with and with every
//! alternative the front door admits, an enumerated type's value set is listed
//! lexicographically whatever order the model declares it in, and the channel-name member's
//! clause states no value at all.
//!
//! The recorded cases this was derived from are three rows of the tables below (a channel
//! name outside its expression, a malformed resource ARN, an out-of-set stream mode and an
//! over-long partition key); the other rows differ along what the rule generalises over —
//! which member carries the value, which constraint class it broke, which operation carried
//! it, how long the value is, and how many clauses one refusal holds.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! codes and messages asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    shard_id, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};

use crate::channel_lifecycle::{role_arn, stream_configuration};
use crate::stream_tags::{error_code, error_message, stream_arn, Emulator, REGION};

/// The error code every constraint refusal of this service answers with.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// AWS's envelope around the clauses of ONE refusal.
fn envelope(clauses: &[&str]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// A `CreateChannel` request that is valid apart from the `ChannelName` it carries, so a
/// refusal of it holds exactly the one clause the case is about.
fn create_channel(name: &str) -> Value {
    json!({
        "ChannelName": name,
        "ServiceExecutionRoleARN": role_arn("channel-delivery"),
        "StreamConfigurationList": [stream_configuration(
            &stream_arn(REGION, "constraint-source"),
            "JSON",
            None,
        )],
    })
}

/// The one refusal `operation` answers `body` with: its code and its message.
async fn refusal(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> String {
    let (status, response) = emulator.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected the constraint refusal, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        VALIDATION_EXCEPTION,
        "case {case}: expected {VALIDATION_EXCEPTION}, got {response}"
    );
    error_message(&response)
}

/// A `@pattern` requirement is quoted as the expression a value has to match — without the
/// anchors the model writes the whole-value match with — and the clause's value half follows
/// the member it is about.
#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_models_anchors() {
    let emulator = Emulator::new();
    let malformed_resource = format!(
        "arn:aws:kinesis:{REGION}:{}:stream/",
        engine::EMULATOR_ACCOUNT_ID
    );

    // (case, operation, body, the clause the refusal must carry). The expressions are the
    // model's own, with the anchors dropped: `smithy_query(query="shape",
    // shape="com.amazonaws.kinesis#<Shape>")` for ChannelName, ResourceARN, StreamARN,
    // ChannelARN and StreamName.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            // The channel's name is the one member whose clause states no value.
            "a channel name carrying spaces",
            "CreateChannel",
            create_channel("bad channel name!!"),
            "Value at 'channelName' failed to satisfy constraint: Member must satisfy regular \
             expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
        (
            "a channel name carrying a slash",
            "CreateChannel",
            create_channel("channels/one"),
            "Value at 'channelName' failed to satisfy constraint: Member must satisfy regular \
             expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
        (
            // The resource-ARN expression names every resource class the front door admits,
            // the queue class included, and the clause echoes the ARN that was sent.
            "a resource ARN with no resource name",
            "TagResource",
            json!({ "ResourceARN": malformed_resource, "Tags": { "k": "v" } }),
            format!(
                "Value '{malformed_resource}' at 'resourceARN' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: \
                 arn:aws.*:kinesis:.*:\\d{{12}}:.*(stream|channel|queue)/\\S+"
            ),
        ),
        (
            "a resource ARN that is no ARN at all",
            "UntagResource",
            json!({ "ResourceARN": "not-an-arn", "TagKeys": ["k"] }),
            "Value 'not-an-arn' at 'resourceARN' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: \
             arn:aws.*:kinesis:.*:\\d{12}:.*(stream|channel|queue)/\\S+"
                .to_string(),
        ),
        (
            "a stream ARN that is no ARN at all",
            "DescribeStream",
            json!({ "StreamARN": "stream/one" }),
            "Value 'stream/one' at 'streamARN' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: arn:aws.*:kinesis:.*:\\d{12}:stream/\\S+"
                .to_string(),
        ),
        (
            "a channel ARN that is no ARN at all",
            "DeleteChannel",
            json!({ "ChannelARN": "channel-one" }),
            "Value 'channel-one' at 'channelARN' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: arn:aws.*:kinesis:.*:\\d{12}:channel/\\S+"
                .to_string(),
        ),
        (
            "a stream name carrying a space",
            "GetShardIterator",
            json!({
                "StreamName": "bad name",
                "ShardId": shard_id(0),
                "ShardIteratorType": "TRIM_HORIZON",
            }),
            "Value 'bad name' at 'streamName' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
    ];

    for (case, operation, body, clause) in rows {
        let message = refusal(&emulator, case, operation, &body).await;
        assert_eq!(
            message,
            envelope(&[clause.as_str()]),
            "case {case:?}: expected the clause {clause:?}"
        );
    }
}

/// An enumerated type's value set is listed in AWS's order — lexicographic — whatever order
/// the model declares its members in.
#[tokio::test]
async fn an_out_of_set_value_lists_the_value_set_in_aws_order() {
    let emulator = Emulator::new();
    let arn = stream_arn(REGION, "enum-order");

    // (case, operation, body, member path, the rejected value, the set as it must be listed).
    // Each set is the model's own, sorted: `smithy_query(query="shape",
    // shape="com.amazonaws.kinesis#<Enum>")` declares StreamMode as [PROVISIONED,
    // ON_DEMAND], EncryptionType as [NONE, KMS] and ShardIteratorType as
    // [AT_SEQUENCE_NUMBER, AFTER_SEQUENCE_NUMBER, TRIM_HORIZON, LATEST, AT_TIMESTAMP].
    let rows: Vec<(&str, &str, Value, &str, &str, &str)> = vec![
        (
            "a stream mode an update does not admit",
            "UpdateStreamMode",
            json!({ "StreamARN": arn, "StreamModeDetails": { "StreamMode": "BOGUS_MODE" } }),
            "streamModeDetails.streamMode",
            "BOGUS_MODE",
            "[ON_DEMAND, PROVISIONED]",
        ),
        (
            "a stream mode a create does not admit",
            "CreateStream",
            json!({
                "StreamName": "enum-order",
                "StreamModeDetails": { "StreamMode": "on-demand" },
            }),
            "streamModeDetails.streamMode",
            "on-demand",
            "[ON_DEMAND, PROVISIONED]",
        ),
        (
            "an encryption type nothing encrypts with",
            "StopStreamEncryption",
            json!({ "StreamARN": arn, "EncryptionType": "AES256", "KeyId": "alias/aws/kinesis" }),
            "encryptionType",
            "AES256",
            "[KMS, NONE]",
        ),
        (
            "an iterator type naming no position",
            "GetShardIterator",
            json!({
                "StreamName": "enum-order",
                "ShardId": shard_id(0),
                "ShardIteratorType": "SOMETIME",
            }),
            "shardIteratorType",
            "SOMETIME",
            "[AFTER_SEQUENCE_NUMBER, AT_SEQUENCE_NUMBER, AT_TIMESTAMP, LATEST, TRIM_HORIZON]",
        ),
    ];

    for (case, operation, body, path, value, listed) in rows {
        let message = refusal(&emulator, case, operation, &body).await;
        let clause = format!(
            "Value '{value}' at '{path}' failed to satisfy constraint: Member must satisfy \
             enum value set: {listed}"
        );
        assert_eq!(
            message,
            envelope(&[clause.as_str()]),
            "case {case:?}: expected the value set listed as {listed:?}"
        );
    }
}

/// A `@length` requirement echoes the value the request carried IN FULL: however long the
/// value is, the clause quotes those bytes and nothing else — no prefix, no byte count.
#[tokio::test]
async fn a_length_requirement_echoes_the_whole_submitted_value() {
    let emulator = Emulator::new();

    // The `@length` maxima the rows break, from the model
    // (`smithy_query(query="shape", shape="com.amazonaws.kinesis#PartitionKey")` -> 1..256,
    // `StreamName` -> 1..128).
    const PARTITION_KEY_MAX: usize = 256;
    const STREAM_NAME_MAX: usize = 128;

    // One character past the bound, well past it, and far enough past it that the
    // framework's own rendering of the value would have been shortened several times over.
    let overshoots = [1usize, 2, 44, 768];
    for overshoot in overshoots {
        let key = "k".repeat(PARTITION_KEY_MAX + overshoot);
        let case = format!("a partition key of {} characters", key.len());
        let message = refusal(
            &emulator,
            &case,
            "PutRecord",
            &json!({
                "StreamName": "echoed-values",
                "PartitionKey": key.clone(),
                "Data": aws_smithy_types::base64::encode("x"),
            }),
        )
        .await;
        let clause = format!(
            "Value '{key}' at 'partitionKey' failed to satisfy constraint: Member must have \
             length less than or equal to {PARTITION_KEY_MAX}"
        );
        assert_eq!(
            message,
            envelope(&[clause.as_str()]),
            "case {case:?}: expected the whole key echoed, got {message:?}"
        );

        // The same rule inside a batch, where the member sits at a position of a list.
        let message = refusal(
            &emulator,
            &case,
            "PutRecords",
            &json!({
                "StreamName": "echoed-values",
                "Records": [{
                    "PartitionKey": key.clone(),
                    "Data": aws_smithy_types::base64::encode("x"),
                }],
            }),
        )
        .await;
        let clause = format!(
            "Value '{key}' at 'records.1.member.partitionKey' failed to satisfy constraint: \
             Member must have length less than or equal to {PARTITION_KEY_MAX}"
        );
        assert_eq!(
            message,
            envelope(&[clause.as_str()]),
            "case {case:?}: expected the whole key echoed inside the batch, got {message:?}"
        );

        // And on another member of another operation, so the rule is the service's and not
        // one member's.
        let name = "n".repeat(STREAM_NAME_MAX + overshoot);
        let case = format!("a stream name of {} characters", name.len());
        let message = refusal(
            &emulator,
            &case,
            "DescribeStreamSummary",
            &json!({ "StreamName": name.clone() }),
        )
        .await;
        let clause = format!(
            "Value '{name}' at 'streamName' failed to satisfy constraint: Member must have \
             length less than or equal to {STREAM_NAME_MAX}"
        );
        assert_eq!(
            message,
            envelope(&[clause.as_str()]),
            "case {case:?}: expected the whole name echoed, got {message:?}"
        );
    }
}

/// A request breaking TWO constraints is one refusal carrying both clauses, counted in
/// AWS's envelope.
#[tokio::test]
async fn a_request_breaking_two_constraints_states_both_clauses() {
    let emulator = Emulator::new();
    let message = refusal(
        &emulator,
        "two broken members",
        "GetShardIterator",
        &json!({
            "StreamName": "two clauses",
            "ShardId": shard_id(0),
            "ShardIteratorType": "SOMETIME",
        }),
    )
    .await;
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case two broken members: expected both clauses counted, got {message:?}"
    );
    for clause in [
        "Value 'two clauses' at 'streamName' failed to satisfy constraint: Member must \
         satisfy regular expression pattern: [a-zA-Z0-9_.-]+",
        "Value 'SOMETIME' at 'shardIteratorType' failed to satisfy constraint: Member must \
         satisfy enum value set: [AFTER_SEQUENCE_NUMBER, AT_SEQUENCE_NUMBER, AT_TIMESTAMP, \
         LATEST, TRIM_HORIZON]",
    ] {
        assert!(
            message.contains(clause),
            "case two broken members: expected the clause {clause:?}, got {message:?}"
        );
    }
}

/// A request every constraint admits is SERVED: the wrapper that words a refusal refuses
/// nothing the model's own walk accepts.
#[tokio::test]
async fn a_request_inside_every_constraint_is_served() {
    let emulator = Emulator::new();
    for (case, mode) in [
        ("provisioned", STREAM_MODE_PROVISIONED),
        ("on-demand", STREAM_MODE_ON_DEMAND),
    ] {
        let name = format!("served-{case}");
        let mut body = json!({
            "StreamName": name.clone(),
            "StreamModeDetails": { "StreamMode": mode },
        });
        if mode == STREAM_MODE_PROVISIONED {
            body["ShardCount"] = json!(1);
        }
        emulator.ok(case, "CreateStream", &body).await;
        let summary = emulator
            .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
            .await;
        assert_eq!(
            summary["StreamDescriptionSummary"]["StreamModeDetails"]["StreamMode"],
            json!(mode),
            "case {case}: expected the create to be served and read back, got {summary}"
        );
    }
}
