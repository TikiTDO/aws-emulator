// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's SERVER-SIDE ENCRYPTION, end to end over the wire:
//! `StartStreamEncryption` / `StopStreamEncryption` -> `DescribeStreamSummary` /
//! `DescribeStream`.
//!
//! RULE: an encryption call STORES the encryption configuration it asks for on the stream
//! its `StreamName` / `StreamARN` pair names, so that is the configuration every later read
//! of that stream reports: `StartStreamEncryption` makes the stream `EncryptionType` `KMS`
//! under the `KeyId` the request carries (enabling encryption, or updating the key of a
//! stream that already has one), and `StopStreamEncryption` DISABLES encryption, so the
//! stream stops naming a key and reports `NONE`. Either change is asynchronous, so the
//! stream is `UPDATING` until the transition back to `ACTIVE` falls due — and a change that
//! arrives while that one is in flight is served too, since "you can continue to read and
//! write data to your stream while its status is UPDATING". Disabling needs
//! encryption on the stream to disable, so a stream whose `EncryptionType` is already `NONE`
//! is refused with `InvalidArgumentException` and the fixed message "Cannot stop encrypting
//! an unencrypted stream." — and a refused request changes NOTHING: the stream keeps the
//! configuration and the status it had, since a rejected request must not start an update.
//! Either call is served only on a stream that is serving (one whose create has not fallen
//! due yet, or whose removal has been accepted, is `ResourceInUseException`), only for the
//! `EncryptionType` both operations document as their
//! only valid value, only for a stream this account and region hold
//! (`ResourceNotFoundException` otherwise, `InvalidArgumentException` when the request names
//! no stream or two different ones), and only 25 times per stream and per kind of change in
//! a rolling 24-hour period (`LimitExceededException` past that).
//!
//! The recorded flow this was derived from calls `StopStreamEncryption(StreamName=…,
//! EncryptionType="KMS", KeyId=…)` once on ONE unencrypted single-shard stream, which is
//! one row of the tables below. The rows differ along what the rule generalises over: the
//! encryption the stream carries when the call arrives, the form of the `KeyId` the request
//! carries, the identifier it addresses the stream by, the capacity mode and shard count of
//! the stream, the status the stream is in, the `EncryptionType` the request names, and how
//! many changes the stream has already spent inside the rolling window.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    stream_not_found_message, RegionalName, ENCRYPTION_CHANGE_WINDOW, ENCRYPTION_TYPE_KMS,
    ENCRYPTION_TYPE_NONE, ENCRYPTION_UPDATE_DELAY, MAX_ENCRYPTION_CHANGES_PER_WINDOW,
    STREAM_ACTIVE, STREAM_CREATING, STREAM_DELETING, stream_identifier_mismatch_message,
    STREAM_IDENTIFIER_REQUIRED_MESSAGE, STREAM_MODE_ON_DEMAND, STREAM_UPDATING,
    UNENCRYPTED_STREAM_MESSAGE,
};

use crate::shard_scaling::summary;
use crate::stream_tags::{error_code, error_message, past_activation, stream_arn, Emulator, REGION};

/// The forms a `KeyId` takes, as `KeyId` catalogs them: "This value can be a globally
/// unique identifier, a fully specified Amazon Resource Name (ARN) to either an alias or a
/// key, or an alias name prefixed by "alias/". You can also use a master key owned by
/// Kinesis Data Streams by specifying the alias aws/kinesis" — the entries are the member's
/// own examples
/// (`smithy_query(query="doc",shape="StopStreamEncryptionInput",member="KeyId")`).
const KEY_ID_GUID: &str = "12345678-1234-1234-1234-123456789012";
const KEY_ID_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012";
const KEY_ID_ALIAS_ARN: &str = "arn:aws:kms:us-east-1:123456789012:alias/MyAliasName";
const KEY_ID_ALIAS_NAME: &str = "alias/MyAliasName";
const KEY_ID_SERVICE_ALIAS: &str = "alias/aws/kinesis";

/// How long a case waits before treating a stream whose encryption changed as ready: past
/// the instant the change armed the `UPDATING` -> `ACTIVE` transition for.
fn past_encryption_update() -> Duration {
    ENCRYPTION_UPDATE_DELAY + Duration::from_millis(400)
}

/// How a case addresses the stream: "you must use either the StreamARN or the StreamName
/// parameter, or both" (`StopStreamEncryption`), so all three are rows of the tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum By {
    Name,
    Arn,
    Both,
}

/// An encryption-call body addressing `name` the way `by` says, for `key_id`. BOTH
/// operations take this input, so both are driven with it.
fn encryption_body(name: &str, key_id: &str, by: By) -> Value {
    let mut body = json!({ "EncryptionType": ENCRYPTION_TYPE_KMS, "KeyId": key_id });
    if by != By::Arn {
        body["StreamName"] = json!(name);
    }
    if by != By::Name {
        body["StreamARN"] = json!(stream_arn(REGION, name));
    }
    body
}

/// One stream named `name`, created with `shard_count` shards when it is provisioned and in
/// the on-demand mode when it is `None`.
async fn create_stream(emulator: &Emulator, case: &str, name: &str, shard_count: Option<u32>) {
    let body = match shard_count {
        Some(shards) => json!({ "StreamName": name, "ShardCount": shards }),
        None => json!({
            "StreamName": name,
            "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
        }),
    };
    emulator.ok(case, "CreateStream", &body).await;
}

/// Encrypt `name` with `key_id` over the wire. The caller waits for the change to fall due
/// before treating the stream as ready.
async fn encrypt(emulator: &Emulator, case: &str, name: &str, key_id: &str) {
    emulator
        .ok(
            case,
            "StartStreamEncryption",
            &encryption_body(name, key_id, By::Name),
        )
        .await;
}

/// The `EncryptionType` and `KeyId` `DescribeStreamSummary` reports for `name`.
async fn reported_encryption(
    emulator: &Emulator,
    case: &str,
    name: &str,
) -> (String, Option<String>) {
    let described = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
        .await;
    encryption_of(case, &described["StreamDescriptionSummary"], &described)
}

/// The `EncryptionType` and `KeyId` `DescribeStream` reports for `name`, which is the other
/// reader of the same stored configuration.
async fn described_encryption(
    emulator: &Emulator,
    case: &str,
    name: &str,
) -> (String, Option<String>) {
    let described = emulator
        .ok(case, "DescribeStream", &json!({ "StreamName": name }))
        .await;
    encryption_of(case, &described["StreamDescription"], &described)
}

/// The encryption configuration a read reports, as the pair the cases compare.
fn encryption_of(case: &str, reported: &Value, whole: &Value) -> (String, Option<String>) {
    let encryption_type = reported
        .get("EncryptionType")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the read must carry EncryptionType, got {whole}"))
        .to_string();
    let key_id = reported
        .get("KeyId")
        .and_then(Value::as_str)
        .map(str::to_string);
    (encryption_type, key_id)
}

/// The configuration a read reports for an encrypted stream.
fn encrypted_with(key_id: &str) -> (String, Option<String>) {
    (ENCRYPTION_TYPE_KMS.to_string(), Some(key_id.to_string()))
}

/// The configuration a read reports for a stream nothing encrypts.
fn unencrypted() -> (String, Option<String>) {
    (ENCRYPTION_TYPE_NONE.to_string(), None)
}

/// Spend `count` of the stream's rolling-window disables, each `ago` before now.
///
/// The ceiling is 25 changes per 24 hours, so the rows that exercise it put the spent ones
/// in the store the handlers read rather than making 25 calls and waiting out each
/// asynchronous change.
fn spend_disables(emulator: &Emulator, case: &str, name: &str, count: usize, ago: Duration) {
    let at = SystemTime::now()
        .checked_sub(ago)
        .unwrap_or_else(|| panic!("case {case}: {ago:?} before now must be representable"));
    let state = emulator.store();
    let mut store = state.lock();
    let stream = store
        .streams
        .get_mut(&RegionalName::new(REGION, name))
        .unwrap_or_else(|| panic!("case {case}: the store must hold the stream {name}"));
    stream.encryption_stops = vec![at; count];
}

/// How many disables the stream has recorded, which is what the rolling-window ceiling is
/// counted over.
fn stored_disables(emulator: &Emulator, case: &str, name: &str) -> usize {
    let state = emulator.store();
    let store = state.lock();
    store
        .streams
        .get(&RegionalName::new(REGION, name))
        .unwrap_or_else(|| panic!("case {case}: the store must hold the stream {name}"))
        .encryption_stops
        .len()
}

/// A stream nothing has encrypted has no encryption to disable: the call is refused with the
/// modelled client error and its fixed message, and the stream is left exactly as it was.
#[tokio::test]
async fn stopping_encryption_on_an_unencrypted_stream_is_refused_and_changes_nothing() {
    // (case and stream name, the shard count the stream is created with — `None` is the
    // on-demand mode, the KeyId form the request carries, the identifier it addresses the
    // stream by)
    let cases: Vec<(&str, Option<u32>, &str, By)> = vec![
        ("stop-none-guid", Some(1), KEY_ID_GUID, By::Name),
        ("stop-none-key-arn", Some(3), KEY_ID_KEY_ARN, By::Arn),
        ("stop-none-alias-arn", Some(2), KEY_ID_ALIAS_ARN, By::Both),
        ("stop-none-alias-name", None, KEY_ID_ALIAS_NAME, By::Name),
        (
            "stop-none-service-alias",
            Some(1),
            KEY_ID_SERVICE_ALIAS,
            By::Arn,
        ),
    ];

    let emulator = Emulator::new();
    for (case, shards, _, _) in &cases {
        create_stream(&emulator, case, case, *shards).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (case, _, key_id, by) in &cases {
        let before = reported_encryption(&emulator, case, case).await;
        assert_eq!(
            before,
            unencrypted(),
            "case {case}: expected a stream nothing encrypted to report \
             {ENCRYPTION_TYPE_NONE} and no key, got {before:?}"
        );

        let (status, response) = emulator
            .call("StopStreamEncryption", &encryption_body(case, key_id, *by))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, UNENCRYPTED_STREAM_MESSAGE,
            "case {case}: expected the refusal to say {UNENCRYPTED_STREAM_MESSAGE:?}, got \
             {message:?}"
        );

        // A rejected request must not start an update: the stream is untouched.
        let after = reported_encryption(&emulator, case, case).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refused call to leave the encryption as it was \
             ({before:?}), got {after:?}"
        );
        let (reported_status, _) = summary(&emulator, case, case).await;
        assert_eq!(
            reported_status, STREAM_ACTIVE,
            "case {case}: expected the refused call to leave the stream {STREAM_ACTIVE}, got \
             {reported_status}"
        );
        assert_eq!(
            stored_disables(&emulator, case, case),
            0,
            "case {case}: expected a refused call to spend none of the stream's disables"
        );
    }
}

/// An accepted `StartStreamEncryption` is what every later read of the stream reports: the
/// key the request named, whether the stream had none before or another one.
#[tokio::test]
async fn an_accepted_start_applies_the_key_the_request_names() {
    // (case and stream name, the key the stream is first encrypted with, the key a second
    // call updates it to — the same one for a row that re-applies it, the identifier the
    // calls address the stream by)
    let cases: Vec<(&str, &str, &str, By)> = vec![
        ("start-guid-to-key-arn", KEY_ID_GUID, KEY_ID_KEY_ARN, By::Name),
        (
            "start-alias-arn-to-alias-name",
            KEY_ID_ALIAS_ARN,
            KEY_ID_ALIAS_NAME,
            By::Arn,
        ),
        (
            "start-service-alias-again",
            KEY_ID_SERVICE_ALIAS,
            KEY_ID_SERVICE_ALIAS,
            By::Both,
        ),
    ];

    let emulator = Emulator::new();
    for (case, _, _, _) in &cases {
        create_stream(&emulator, case, case, Some(1)).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (case, first, _, by) in &cases {
        emulator
            .ok(
                case,
                "StartStreamEncryption",
                &encryption_body(case, first, *by),
            )
            .await;

        // The change is asynchronous, so the stream is UPDATING straight after the call and
        // already reports the configuration the call stored.
        let (status, _) = summary(&emulator, case, case).await;
        assert_eq!(
            status, STREAM_UPDATING,
            "case {case}: expected the stream to be {STREAM_UPDATING} straight after the \
             call, got {status}"
        );
        let reported = reported_encryption(&emulator, case, case).await;
        assert_eq!(
            reported,
            encrypted_with(first),
            "case {case}: expected the stream to report the key the call named, got \
             {reported:?}"
        );
    }

    tokio::time::sleep(past_encryption_update()).await;
    for (case, first, second, by) in &cases {
        let (status, _) = summary(&emulator, case, case).await;
        assert_eq!(
            status, STREAM_ACTIVE,
            "case {case}: expected the stream to go back to {STREAM_ACTIVE} once the change \
             fell due, got {status}"
        );
        let reported = reported_encryption(&emulator, case, case).await;
        assert_eq!(
            reported,
            encrypted_with(first),
            "case {case}: expected the stream to keep the key once the change fell due, got \
             {reported:?}"
        );
        let described = described_encryption(&emulator, case, case).await;
        assert_eq!(
            described, reported,
            "case {case}: expected both readers of the stored configuration to agree, got \
             {described:?} and {reported:?}"
        );

        // "Enables or UPDATES server-side encryption using an Amazon Web Services KMS key":
        // the second call is served on an already-encrypted stream and the key it names is
        // what the stream reports from then on.
        emulator
            .ok(
                case,
                "StartStreamEncryption",
                &encryption_body(case, second, *by),
            )
            .await;
        let updated = reported_encryption(&emulator, case, case).await;
        assert_eq!(
            updated,
            encrypted_with(second),
            "case {case}: expected the update to store the key the second call named, got \
             {updated:?}"
        );
    }
}

/// An accepted stop is what every later read of the stream reports: no encryption and no
/// key, `UPDATING` until the change falls due and `ACTIVE` from then on.
#[tokio::test]
async fn an_accepted_stop_clears_the_stream_s_encryption() {
    // (case and stream name, the KeyId the stream is encrypted with, the identifier the stop
    // addresses the stream by)
    let cases: Vec<(&str, &str, By)> = vec![
        ("stop-kms-guid", KEY_ID_GUID, By::Name),
        ("stop-kms-key-arn", KEY_ID_KEY_ARN, By::Arn),
        ("stop-kms-alias-name", KEY_ID_ALIAS_NAME, By::Both),
    ];

    let emulator = Emulator::new();
    for (case, _, _) in &cases {
        create_stream(&emulator, case, case, Some(1)).await;
    }
    tokio::time::sleep(past_activation()).await;
    for (case, key_id, _) in &cases {
        encrypt(&emulator, case, case, key_id).await;
    }
    tokio::time::sleep(past_encryption_update()).await;

    for (case, key_id, by) in &cases {
        let before = reported_encryption(&emulator, case, case).await;
        assert_eq!(
            before,
            encrypted_with(key_id),
            "case {case}: expected the encrypted stream to report {ENCRYPTION_TYPE_KMS} and \
             its key, got {before:?}"
        );

        emulator
            .ok(
                case,
                "StopStreamEncryption",
                &encryption_body(case, key_id, *by),
            )
            .await;

        // The change is asynchronous, so the stream is UPDATING straight after the call and
        // already reports the configuration the call stored.
        let (status, _) = summary(&emulator, case, case).await;
        assert_eq!(
            status, STREAM_UPDATING,
            "case {case}: expected the stream to be {STREAM_UPDATING} straight after the \
             call, got {status}"
        );
        let cleared = reported_encryption(&emulator, case, case).await;
        assert_eq!(
            cleared,
            unencrypted(),
            "case {case}: expected the stream to report {ENCRYPTION_TYPE_NONE} and no key, \
             got {cleared:?}"
        );
        let described = described_encryption(&emulator, case, case).await;
        assert_eq!(
            described, cleared,
            "case {case}: expected both readers of the stored configuration to agree, got \
             {described:?} and {cleared:?}"
        );
        assert_eq!(
            stored_disables(&emulator, case, case),
            1,
            "case {case}: expected the accepted call to spend one of the stream's disables"
        );
    }

    tokio::time::sleep(past_encryption_update()).await;
    for (case, _, _) in &cases {
        let (status, _) = summary(&emulator, case, case).await;
        assert_eq!(
            status, STREAM_ACTIVE,
            "case {case}: expected the stream to go back to {STREAM_ACTIVE} once the change \
             fell due, got {status}"
        );
        let settled = reported_encryption(&emulator, case, case).await;
        assert_eq!(
            settled,
            unencrypted(),
            "case {case}: expected the stream to stay unencrypted once the change fell due, \
             got {settled:?}"
        );
    }
}

/// The recorded positive control, as its own row: a stop that arrives while the start's own
/// change is still in flight is SERVED — "you can continue to read and write data to your
/// stream while its status is UPDATING" — and the configuration it stores is the one that
/// stands.
#[tokio::test]
async fn a_stop_right_after_a_start_is_served() {
    let emulator = Emulator::new();
    let case = "stop-right-after-start";
    create_stream(&emulator, case, case, Some(1)).await;
    tokio::time::sleep(past_activation()).await;

    encrypt(&emulator, case, case, KEY_ID_GUID).await;
    let (status, _) = summary(&emulator, case, case).await;
    assert_eq!(
        status, STREAM_UPDATING,
        "case {case}: expected the start to leave the stream {STREAM_UPDATING}, got {status}"
    );

    // No wait: the stop is issued while the stream is still UPDATING from the start.
    emulator
        .ok(
            case,
            "StopStreamEncryption",
            &encryption_body(case, KEY_ID_GUID, By::Name),
        )
        .await;
    let cleared = reported_encryption(&emulator, case, case).await;
    assert_eq!(
        cleared,
        unencrypted(),
        "case {case}: expected the stop to clear the encryption, got {cleared:?}"
    );

    tokio::time::sleep(past_encryption_update()).await;
    let (status, _) = summary(&emulator, case, case).await;
    assert_eq!(
        status, STREAM_ACTIVE,
        "case {case}: expected the stream to settle {STREAM_ACTIVE}, got {status}"
    );
    let settled = reported_encryption(&emulator, case, case).await;
    assert_eq!(
        settled,
        unencrypted(),
        "case {case}: expected the stream to settle unencrypted, got {settled:?}"
    );
}

/// Every other refusal the operation models, each leaving the stored configuration as it
/// was.
#[tokio::test]
async fn a_refused_stop_answers_the_modelled_error_and_changes_nothing() {
    let emulator = Emulator::new();

    // "The encryption type. The only valid value is KMS"
    // (`StopStreamEncryptionInput$EncryptionType`).
    let wrong_type = "stop-encryption-type";
    create_stream(&emulator, wrong_type, wrong_type, Some(1)).await;
    tokio::time::sleep(past_activation()).await;
    encrypt(&emulator, wrong_type, wrong_type, KEY_ID_GUID).await;
    tokio::time::sleep(past_encryption_update()).await;
    let (status, response) = emulator
        .call(
            "StopStreamEncryption",
            &json!({
                "StreamName": wrong_type,
                "EncryptionType": ENCRYPTION_TYPE_NONE,
                "KeyId": KEY_ID_GUID,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {wrong_type}: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidArgumentException",
        "case {wrong_type}: expected InvalidArgumentException, got {response}"
    );
    let message = error_message(&response);
    assert!(
        message.contains(ENCRYPTION_TYPE_NONE) && message.contains(ENCRYPTION_TYPE_KMS),
        "case {wrong_type}: expected the message to name the type at fault and the only \
         valid one, got {message:?}"
    );
    let kept = reported_encryption(&emulator, wrong_type, wrong_type).await;
    assert_eq!(
        kept,
        encrypted_with(KEY_ID_GUID),
        "case {wrong_type}: expected the refused call to leave the encryption on, got {kept:?}"
    );

    // A `StreamARN` this account and region hold no stream under.
    let absent = stream_arn(REGION, "stop-no-such-stream");
    let (status, response) = emulator
        .call(
            "StopStreamEncryption",
            &json!({
                "StreamARN": absent,
                "EncryptionType": ENCRYPTION_TYPE_KMS,
                "KeyId": KEY_ID_GUID,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case absent: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case absent: expected ResourceNotFoundException, got {response}"
    );
    let message = error_message(&response);
    let says = stream_not_found_message(&absent);
    assert!(
        message.contains(&says),
        "case absent: expected the message to name {says:?}, got {message:?}"
    );

    // A request naming no stream at all.
    let (status, response) = emulator
        .call(
            "StopStreamEncryption",
            &json!({ "EncryptionType": ENCRYPTION_TYPE_KMS, "KeyId": KEY_ID_GUID }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case unspecified: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidArgumentException",
        "case unspecified: expected InvalidArgumentException, got {response}"
    );
    let message = error_message(&response);
    assert!(
        message.contains(STREAM_IDENTIFIER_REQUIRED_MESSAGE),
        "case unspecified: expected the message to name the rule \
         {STREAM_IDENTIFIER_REQUIRED_MESSAGE:?}, got {message:?}"
    );

    // One naming two different streams.
    let other = "stop-other-stream";
    create_stream(&emulator, other, other, Some(1)).await;
    tokio::time::sleep(past_activation()).await;
    let (status, response) = emulator
        .call(
            "StopStreamEncryption",
            &json!({
                "StreamName": other,
                "StreamARN": stream_arn(REGION, wrong_type),
                "EncryptionType": ENCRYPTION_TYPE_KMS,
                "KeyId": KEY_ID_GUID,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case mismatch: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidArgumentException",
        "case mismatch: expected InvalidArgumentException, got {response}"
    );
    let message = error_message(&response);
    let mismatch = stream_identifier_mismatch_message(other, &stream_arn(REGION, wrong_type));
    assert!(
        message.contains(&mismatch),
        "case mismatch: expected the message to name the rule {mismatch:?}, got {message:?}"
    );

    // "For successful operation, the resource must be in the ACTIVE state"
    // (`ResourceInUseException`): a stream whose removal has been accepted is not serving,
    // so its encryption is nothing's to change.
    let deleting = "stop-still-deleting";
    create_stream(&emulator, deleting, deleting, Some(1)).await;
    tokio::time::sleep(past_activation()).await;
    encrypt(&emulator, deleting, deleting, KEY_ID_KEY_ARN).await;
    tokio::time::sleep(past_encryption_update()).await;
    emulator
        .ok(deleting, "DeleteStream", &json!({ "StreamName": deleting }))
        .await;
    let (status, response) = emulator
        .call(
            "StopStreamEncryption",
            &encryption_body(deleting, KEY_ID_KEY_ARN, By::Name),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {deleting}: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case {deleting}: expected ResourceInUseException, got {response}"
    );
    let message = error_message(&response);
    assert!(
        message.contains(STREAM_DELETING),
        "case {deleting}: expected the message to name the status that refused the call, got \
         {message:?}"
    );
    let kept = reported_encryption(&emulator, deleting, deleting).await;
    assert_eq!(
        kept,
        encrypted_with(KEY_ID_KEY_ARN),
        "case {deleting}: expected the refused call to leave the encryption on, got {kept:?}"
    );

    // The same rule at the other end of the lifecycle: a stream whose create has not fallen
    // due yet is not serving either, so encryption is nothing's to turn on.
    let creating = "start-still-creating";
    create_stream(&emulator, creating, creating, Some(1)).await;
    let (status, response) = emulator
        .call(
            "StartStreamEncryption",
            &encryption_body(creating, KEY_ID_GUID, By::Name),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {creating}: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case {creating}: expected ResourceInUseException, got {response}"
    );
    let message = error_message(&response);
    assert!(
        message.contains(STREAM_CREATING),
        "case {creating}: expected the message to name the status that refused the call, got \
         {message:?}"
    );
    tokio::time::sleep(past_activation()).await;
    let kept = reported_encryption(&emulator, creating, creating).await;
    assert_eq!(
        kept,
        unencrypted(),
        "case {creating}: expected the refused call to leave the stream unencrypted, got \
         {kept:?}"
    );
}

/// "You can successfully disable server-side encryption 25 times in a rolling 24-hour
/// period" (`StopStreamEncryption`): the ceiling counts the disables inside the window, so
/// one spent before the window rolled past does not count, and an accepted call counts.
#[tokio::test]
async fn the_rolling_window_bounds_how_often_encryption_is_disabled() {
    let emulator = Emulator::new();
    let inside = Duration::from_secs(60 * 60);
    let outside = ENCRYPTION_CHANGE_WINDOW + Duration::from_secs(60 * 60);

    // (case and stream name, how many disables are already spent, how long before now they
    // were spent, whether the call is served)
    let cases: Vec<(&str, usize, Duration, bool)> = vec![
        (
            "stop-window-spent",
            MAX_ENCRYPTION_CHANGES_PER_WINDOW,
            inside,
            false,
        ),
        (
            "stop-window-rolled",
            MAX_ENCRYPTION_CHANGES_PER_WINDOW,
            outside,
            true,
        ),
        (
            "stop-window-last",
            MAX_ENCRYPTION_CHANGES_PER_WINDOW - 1,
            inside,
            true,
        ),
        ("stop-window-unspent", 0, inside, true),
    ];

    for (case, _, _, _) in &cases {
        create_stream(&emulator, case, case, Some(1)).await;
    }
    tokio::time::sleep(past_activation()).await;
    for (case, _, _, _) in &cases {
        encrypt(&emulator, case, case, KEY_ID_GUID).await;
    }
    tokio::time::sleep(past_encryption_update()).await;

    for (case, spent, ago, served) in &cases {
        spend_disables(&emulator, case, case, *spent, *ago);

        let (status, response) = emulator
            .call(
                "StopStreamEncryption",
                &encryption_body(case, KEY_ID_GUID, By::Name),
            )
            .await;
        let reported = reported_encryption(&emulator, case, case).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case}: expected the call to be served, got {status} with body {response}"
            );
            assert_eq!(
                reported,
                unencrypted(),
                "case {case}: expected the served call to clear the encryption, got {reported:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: expected a client error, got {status} with body {response}"
            );
            assert_eq!(
                error_code(&response),
                "LimitExceededException",
                "case {case}: expected LimitExceededException, got {response}"
            );
            let message = error_message(&response);
            assert!(
                message.contains(&MAX_ENCRYPTION_CHANGES_PER_WINDOW.to_string()),
                "case {case}: expected the message to name the ceiling \
                 {MAX_ENCRYPTION_CHANGES_PER_WINDOW}, got {message:?}"
            );
            assert_eq!(
                reported,
                encrypted_with(KEY_ID_GUID),
                "case {case}: expected the refused call to leave the encryption on, got \
                 {reported:?}"
            );
        }
    }

    // The accepted call COUNTS: the stream that had one disable left has none now, so
    // encrypting it again and disabling it again is refused.
    let last = "stop-window-last";
    tokio::time::sleep(past_encryption_update()).await;
    encrypt(&emulator, last, last, KEY_ID_GUID).await;
    tokio::time::sleep(past_encryption_update()).await;
    assert_eq!(
        stored_disables(&emulator, last, last),
        MAX_ENCRYPTION_CHANGES_PER_WINDOW,
        "case {last}: expected the accepted disable to be counted beside the spent ones"
    );
    let (status, response) = emulator
        .call(
            "StopStreamEncryption",
            &encryption_body(last, KEY_ID_GUID, By::Name),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {last}: expected the disable past the ceiling to be refused, got {status} with \
         body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case {last}: expected LimitExceededException once the window's disables are spent, \
         got {response}"
    );
}
