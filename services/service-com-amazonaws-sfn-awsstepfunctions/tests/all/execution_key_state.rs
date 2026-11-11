// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KEY STATE a call that uses a state machine's customer
//! managed KMS key is decided against, driven over both services' own wire protocols
//! through the production `MultiServiceRouter` with Step Functions and KMS mounted on
//! ONE engine, so the key state the call is judged against is the one KMS's own
//! handlers hold.
//!
//! The rule under test: a call that seals data under a state machine's customer managed
//! key, or unseals data that key sealed, can only be served while the key's own key
//! state admits cryptographic use of it, so it is refused with HTTP 400
//! `KmsInvalidStateException` — carrying that key state and the sentence KMS words the
//! refusal with — for every key state that does not, and is served again from the first
//! call after the key is back in one that does. A call that seals and unseals nothing
//! asks nothing of the key and is served throughout.
//!
//! The rows span the key states a key can be unusable in and how a caller gets it there
//! (disabling it, scheduling its deletion, creating it with no key material), the
//! readers and the writers the rule covers, the requests of those same operations that
//! use the key for nothing, and the data key reuse periods a state machine can name.
//!
//! The harness (`Chain`, the key policies, `customer_managed`) is the one
//! `execution_data_encryption` builds for the key POLICY side of the same decision, and
//! the writer table the one `encrypted_execution_writes` builds; `tests/all.rs` is this
//! binary's crate root, so both are reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::encrypted_execution_writes::{message, outstanding_task, writes, Write};
use crate::execution_data_encryption::{
    customer_managed, granting_policy, reads, Chain, Read, SFN,
};
use crate::state_machine_lifecycle::{error_code, string};

/// The modelled error class a call the key's state refuses answers, and the status it
/// carries: "The KMS key is not in valid state, for example: Disabled or Deleted"
/// (HTTP 400).
const KMS_INVALID_STATE: &str = "KmsInvalidStateException";
const KMS_INVALID_STATE_STATUS: u16 = 400;

/// The shortest deletion window `ScheduleKeyDeletion` accepts, which is the one a row
/// that schedules a deletion asks for: the window only says when the key material goes,
/// and the key is in `PendingDeletion` for all of it.
const SHORTEST_DELETION_WINDOW_DAYS: u32 = 7;

/// A payload of the shapes an execution's `input` can carry — an object, a string, a
/// number, an array and a nested object — so what a read hands back after the key is
/// usable again is compared on more than one JSON type.
const PAYLOAD: &str = r#"{"marker":"before-the-key-moved","items":[1,"two",{"three":3.5}]}"#;

/// One KMS call that moves a key between key states: the operation, and the request
/// body it takes for a given key.
type KeyCall = (&'static str, fn(&str) -> Value);

/// One key state that admits no cryptographic use of the key: how a caller gets a key
/// into it, what `kmsKeyState` reports for it, the prose KMS's own sentence states, and
/// the calls that take the key back out of it.
struct Unusable {
    case: &'static str,
    /// The `CreateKey` request the key is made with, beside its policy.
    create: Value,
    /// The KMS calls that put the created key into this key state.
    into: Vec<KeyCall>,
    /// Whether the key is in this key state from the MOMENT IT EXISTS, with no call of
    /// the caller's to get it there.
    ///
    /// Such a key can carry no encrypted resource at all: a configuration naming a key
    /// in a key state that admits no cryptographic use of it is refused outright
    /// (`InvalidEncryptionConfiguration`), so the only way to observe this key state is
    /// that refusal, which
    /// `encrypted_resource_key_configuration` exercises.
    from_creation: bool,
    /// What KMS's own `KeyState` reports, which is what `DescribeKey` answers.
    reported: &'static str,
    /// The value of the refusal's `kmsKeyState` member
    /// (`smithy_query(query="shape",shape="KmsKeyState")`).
    kms_key_state: &'static str,
    /// The key state as KMS's refusal states it: "<key arn> is <this>."
    prose: &'static str,
    /// The KMS calls that put the key back into a usable key state — empty when no
    /// call does, which is a key state the refusal cannot be reversed out of here.
    back: Vec<KeyCall>,
}

/// A KMS request naming only the key.
fn by_key(key: &str) -> Value {
    json!({ "KeyId": key })
}

/// Every key state the rule refuses a call in, and how a caller reaches it.
fn unusable_states() -> Vec<Unusable> {
    vec![
        Unusable {
            case: "the key is disabled",
            create: json!({ "KeySpec": "SYMMETRIC_DEFAULT" }),
            into: vec![("DisableKey", by_key)],
            from_creation: false,
            reported: "Disabled",
            kms_key_state: "DISABLED",
            prose: "disabled",
            back: vec![("EnableKey", by_key)],
        },
        Unusable {
            case: "the key's deletion is scheduled",
            create: json!({ "KeySpec": "SYMMETRIC_DEFAULT" }),
            into: vec![(
                "ScheduleKeyDeletion",
                |key| json!({ "KeyId": key, "PendingWindowInDays": SHORTEST_DELETION_WINDOW_DAYS }),
            )],
            from_creation: false,
            reported: "PendingDeletion",
            kms_key_state: "PENDING_DELETION",
            prose: "pending deletion",
            // Cancelling the deletion leaves the key disabled, so it takes both calls
            // to put the key back into use.
            back: vec![("CancelKeyDeletion", by_key), ("EnableKey", by_key)],
        },
        Unusable {
            case: "the key waits for key material of its own",
            // "Origin EXTERNAL" mints a key with no key material at all, which waits
            // in `PendingImport` — a key state it is in from the moment it exists,
            // with no call of the caller's needed to get it there.
            create: json!({ "KeySpec": "SYMMETRIC_DEFAULT", "Origin": "EXTERNAL" }),
            into: Vec::new(),
            from_creation: true,
            reported: "PendingImport",
            kms_key_state: "PENDING_IMPORT",
            prose: "pending import",
            back: Vec::new(),
        },
    ]
}

/// Make `calls` on the key, each of which must be served.
async fn drive(chain: &Chain, key: &str, calls: &[KeyCall]) {
    for (operation, body) in calls {
        chain.kms(operation, &body(key)).await;
    }
}

/// What `DescribeKey` reports the key's `KeyState` as, which is the state the refusal
/// below is attributed to.
async fn reported_key_state(chain: &Chain, key: &str) -> String {
    let described = chain.kms("DescribeKey", &by_key(key)).await;
    string(&described["KeyMetadata"], "KeyState")
}

/// Assert `answer` is the modelled refusal for a key in `state`, naming the key by its
/// ARN and relaying KMS's own sentence with the metadata an AWS error carries.
///
/// The id of the refused inner call is minted per call, so the metadata that follows
/// the sentence is checked for its fixed text with that id and the attempt count read
/// out of it rather than compared to anything.
fn assert_refused(case: &str, status: u16, answer: &Value, key: &str, state: &Unusable) {
    assert_eq!(
        (status, error_code(answer).as_str()),
        (KMS_INVALID_STATE_STATUS, KMS_INVALID_STATE),
        "case {case:?}: a call that uses a key in {:?} is refused, got {status} with {answer:?}",
        state.kms_key_state
    );
    assert_eq!(
        answer.get("kmsKeyState").and_then(Value::as_str),
        Some(state.kms_key_state),
        "case {case:?}: the refusal reports the key state the caller has to change, got \
         {answer:?}"
    );
    let sentence = format!("{key} is {}.", state.prose);
    let relayed = message(answer);
    let trailer = relayed.strip_prefix(&sentence).unwrap_or_else(|| {
        panic!("case {case:?}: expected the refusal to state {sentence:?}, got {answer:?}")
    });
    let (request_id, attempts) = trailer
        .strip_prefix(" (Service: Kms, Status Code: 400, Request ID: ")
        .and_then(|rest| rest.split_once(") (SDK Attempt Count: "))
        .and_then(|(id, rest)| rest.strip_suffix(')').map(|count| (id, count)))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the relayed metadata \" (Service: Kms, Status Code: 400, \
                 Request ID: <id>) (SDK Attempt Count: <n>)\", got {trailer:?}"
            )
        });
    assert!(
        !request_id.is_empty() && !request_id.contains(' '),
        "case {case:?}: expected the id of the refused call, got {request_id:?}"
    );
    assert!(
        attempts.parse::<u32>().is_ok_and(|made| made >= 1),
        "case {case:?}: expected how many attempts were made, got {attempts:?}"
    );
}

/// A read of data the key sealed follows the KEY STATE at the instant of the call:
/// refused with 400 `KmsInvalidStateException` for every key state that admits no
/// cryptographic use of the key, and served again from the first call after the key is
/// back in one that does. A read that reports none of the sealed data is served
/// throughout, since it asks nothing of the key.
#[tokio::test]
async fn a_read_of_encrypted_data_follows_the_keys_state() {
    for (index, state) in unusable_states().into_iter().enumerate() {
        // A key that is unusable from the moment it exists carries no encrypted
        // resource to read: configuring one on it is refused instead (see
        // [`Unusable::from_creation`]).
        if state.from_creation {
            continue;
        }
        let chain = Chain::new();
        let mut create = state.create.clone();
        create["Policy"] = Value::String(granting_policy().to_string());
        let key = string(&chain.kms("CreateKey", &create).await["KeyMetadata"], "Arn");
        let (machine, execution) = chain
            .run_with_input(
                &format!("key-state-read-{index}"),
                Some(customer_managed(&key)),
                PAYLOAD,
            )
            .await;

        drive(&chain, &key, &state.into).await;
        let reported = reported_key_state(&chain, &key).await;
        assert_eq!(
            reported, state.reported,
            "case {:?}: expected KMS to report the key in {:?}, got {reported:?}",
            state.case, state.reported
        );

        for Read {
            case,
            operation,
            decrypts,
            request,
        } in reads()
        {
            let body = request(&machine, &execution);
            let (status, answer) = chain.call(&SFN, operation, &body).await;
            let case = format!("{} / {case}", state.case);
            if decrypts {
                assert_refused(&case, status, &answer, &key, &state);
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a read that uses the key for nothing is served whatever its \
                     key state, got {status} with {answer:?}"
                );
            }
        }

        if state.back.is_empty() {
            continue;
        }
        // Nothing latched: the key is usable again, so the very same reads are served.
        drive(&chain, &key, &state.back).await;
        for Read {
            case,
            operation,
            request,
            ..
        } in reads()
        {
            let (status, answer) = chain
                .call(&SFN, operation, &request(&machine, &execution))
                .await;
            assert_eq!(
                status, 200,
                "case {:?} / {case:?}: a key back in a usable key state serves the read again, \
                 got {status} with {answer:?}",
                state.case
            );
        }
    }
}

/// A write of data the key must seal follows the same key state: refused while the key
/// admits no cryptographic use of it, and — the very same request, so the refusal
/// consumed neither the task nor the execution — served once the key is usable again. A
/// request carrying nothing to seal is served while the key is unusable.
#[tokio::test]
async fn a_write_of_sealed_data_follows_the_keys_state() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    // The one key state a caller moves a key in and out of with a single call each,
    // which is what a write that must survive the round trip needs.
    let disabled = unusable_states()
        .into_iter()
        .find(|state| state.kms_key_state == "DISABLED")
        .expect("case the rows: a disabled key is one of them");

    for (index, write) in writes().into_iter().enumerate() {
        let Write {
            case,
            operation,
            seals,
            request,
        } = write;
        let case = format!("the key is disabled / {case}");

        // The execution is started and its task claimed while the key is usable: what
        // is under test is the write, not the setup.
        drive(&chain, &key, &disabled.back).await;
        let (execution, token) = outstanding_task(
            &chain,
            &format!("key-state-write-{index}"),
            Some(customer_managed(&key)),
        )
        .await;
        let body = request(&token, &execution);

        drive(&chain, &key, &disabled.into).await;
        let (status, answer) = chain.call(&SFN, operation, &body).await;
        if !seals {
            assert_eq!(
                status, 200,
                "case {case:?}: a request that seals nothing under the key is served whatever its \
                 key state, got {status} with {answer:?}"
            );
            continue;
        }
        assert_refused(&case, status, &answer, &key, &disabled);

        drive(&chain, &key, &disabled.back).await;
        let (status, answer) = chain.call(&SFN, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a key back in a usable key state serves the same write, so the \
             refusal consumed nothing, got {status} with {answer:?}"
        );
    }
}

/// The reuse period a state machine names bounds how long a data key MAY serve without
/// KMS being asked again — "maximum duration that Step Functions will reuse data keys"
/// — so it is no permission to serve a read the key's state refuses: the refusal
/// appears for every period the member accepts, including each bound of its range
/// (`kmsDataKeyReusePeriodSeconds`, range 60..900,
/// `smithy_query(query="shape",shape="EncryptionConfiguration")`) and the configuration
/// that names no period at all.
#[tokio::test]
async fn the_refusal_does_not_wait_for_the_data_key_reuse_period() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    let disabled = unusable_states()
        .into_iter()
        .find(|state| state.kms_key_state == "DISABLED")
        .expect("case the rows: a disabled key is one of them");

    // Rows differ along the period the state machine's encryption configuration names.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the shortest period the member accepts",
            json!({
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": key,
                "kmsDataKeyReusePeriodSeconds": 60
            }),
        ),
        (
            "the longest period the member accepts",
            json!({
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": key,
                "kmsDataKeyReusePeriodSeconds": 900
            }),
        ),
        (
            "no period named at all",
            json!({ "type": "CUSTOMER_MANAGED_KMS_KEY", "kmsKeyId": key }),
        ),
    ];

    for (index, (case, encryption)) in rows.into_iter().enumerate() {
        drive(&chain, &key, &disabled.back).await;
        let (_machine, execution) = chain
            .run_with_input(&format!("reuse-period-{index}"), Some(encryption), PAYLOAD)
            .await;

        drive(&chain, &key, &disabled.into).await;
        let (status, answer) = chain
            .call(&SFN, "DescribeExecution", &json!({ "executionArn": execution }))
            .await;
        assert_refused(case, status, &answer, &key, &disabled);
    }
}

/// The whole contract the refusal exists for, end to end: the execution's data is read
/// back while the key is usable, both data-bearing reads are refused once the key is
/// disabled, and after the key is enabled again they answer with the SAME input, the
/// SAME output and the SAME history event sequence as before the key was touched — the
/// refusal withholds the data, it does not change it.
#[tokio::test]
async fn disabling_and_enabling_the_key_withholds_the_data_and_hands_it_back_unchanged() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    let (_machine, execution) = chain
        .run_with_input("key-state-round-trip", Some(customer_managed(&key)), PAYLOAD)
        .await;
    let request = json!({ "executionArn": execution });

    let described = chain.sfn("DescribeExecution", &request).await;
    let input = string(&described, "input");
    let output = string(&described, "output");
    assert_eq!(
        input, PAYLOAD,
        "case before the key moved: expected the execution's input to be read back verbatim, got \
         {input:?}"
    );
    let history = event_types(&chain.sfn("GetExecutionHistory", &request).await);
    assert!(
        !history.is_empty(),
        "case before the key moved: expected the history to carry events, got {history:?}"
    );

    chain.kms("DisableKey", &by_key(&key)).await;
    let disabled = unusable_states()
        .into_iter()
        .find(|state| state.kms_key_state == "DISABLED")
        .expect("case the rows: a disabled key is one of them");
    for operation in ["DescribeExecution", "GetExecutionHistory"] {
        let (status, answer) = chain.call(&SFN, operation, &request).await;
        assert_refused(
            &format!("the key is disabled / {operation}"),
            status,
            &answer,
            &key,
            &disabled,
        );
    }

    chain.kms("EnableKey", &by_key(&key)).await;
    let described = chain.sfn("DescribeExecution", &request).await;
    assert_eq!(
        (
            string(&described, "input").as_str(),
            string(&described, "output").as_str()
        ),
        (input.as_str(), output.as_str()),
        "case after the key was enabled: expected the same input and output as before, got \
         {described:?}"
    );
    let after = event_types(&chain.sfn("GetExecutionHistory", &request).await);
    assert_eq!(
        after, history,
        "case after the key was enabled: expected the same history event sequence as before, got \
         {after:?}"
    );
}

/// The `type` of each event a history answer carries, in the order it reports them.
fn event_types(answer: &Value) -> Vec<String> {
    answer["events"]
        .as_array()
        .unwrap_or_else(|| panic!("case the history: expected an event array, got {answer:?}"))
        .iter()
        .map(|event| string(event, "type"))
        .collect()
}

/// A state machine on an Amazon Web Services owned key names no key of the account's,
/// so no key state of any customer managed key withholds its data: AWS manages the key
/// the service owns, and the reads are served while a key the account disabled sits
/// beside them.
#[tokio::test]
async fn a_state_machine_on_an_aws_owned_key_has_no_key_state_to_follow() {
    let chain = Chain::new();
    // A key the account disables, to show that a state machine not on it is unaffected
    // by what state it is in.
    let key = chain.create_key(&granting_policy()).await;
    chain.kms("DisableKey", &by_key(&key)).await;

    // Rows differ along HOW the state machine ends up on the AWS owned key: by naming
    // no encryption configuration at all, and by naming that type.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no encryption configuration", None),
        (
            "the AWS owned key named",
            Some(json!({ "type": "AWS_OWNED_KEY" })),
        ),
    ];

    for (index, (case, encryption)) in rows.into_iter().enumerate() {
        let (machine, execution) = chain
            .run_with_input(&format!("owned-key-state-{index}"), encryption, PAYLOAD)
            .await;
        for Read {
            case: read,
            operation,
            request,
            ..
        } in reads()
        {
            let (status, answer) = chain
                .call(&SFN, operation, &request(&machine, &execution))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} / {read:?}: a resource on the AWS owned key follows no customer \
                 managed key's state, got {status} with {answer:?}"
            );
        }
    }
}

/// An identifier that names no key KMS holds attributes no key state to the resource,
/// so the read is served: the refusal is the KEY's state, and a state nobody can
/// observe refuses nothing.
#[tokio::test]
async fn a_configuration_naming_no_key_refuses_nothing() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;
    // A key ARN of the same account and Region that KMS holds no key under: the one
    // KMS minted, with its key id replaced by another well-formed one.
    let absent = format!(
        "{}key/11111111-2222-3333-4444-555555555555",
        key.split_once("key/")
            .map(|(prefix, _)| prefix)
            .unwrap_or_else(|| panic!("case the key: expected a key ARN, got {key:?}"))
    );
    let (machine, execution) = chain
        .run_with_input("absent-key", Some(customer_managed(&absent)), PAYLOAD)
        .await;

    for Read {
        case,
        operation,
        request,
        ..
    } in reads()
    {
        let (status, answer) = chain
            .call(&SFN, operation, &request(&machine, &execution))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an identifier naming no key attributes no key state, got {status} \
             with {answer:?}"
        );
    }
    // And the key that does exist is untouched by the state machine that named a
    // different identifier.
    let reported = reported_key_state(&chain, &key).await;
    assert_eq!(
        reported, "Enabled",
        "case the key KMS does hold: expected it left in Enabled, got {reported:?}"
    );
}
