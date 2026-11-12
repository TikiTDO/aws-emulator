// SPDX-License-Identifier: CC0-1.0

//! The KEYS of `sessionState.intent.slots`: which slots a turn reports, as
//! against which ones the conversation has filled.
//!
//! `Intent$slots` is "a map of all of the slots for the intent. The name of the
//! slot maps to the value of the slot. If a slot has not been filled, the value
//! is null", so the map is keyed by the slots the recognized intent DECLARES in
//! the built locale, whatever the dialog has reached: a slot with no value is
//! named with the documented `null` rather than left out, and a name the intent
//! does not declare is not a slot of it and is not reported.
//!
//! The rule is pinned over a TABLE of intents declaring a different NUMBER of
//! slots — none, one, two, three — each walked from the opening turn, which
//! fills nothing, to the turn that fills the last of them, and over every
//! carrier of the same session state: the `RecognizeText` body, the
//! `RecognizeUtterance` session-state header and `GetSession`. One bot declares
//! them all in one built locale, so what differs between the rows is the
//! intent's own slots and nothing else.
//!
//! `bot_locale_dialog.rs` and `dialog_turn_reporting.rs` assert the same rule
//! for an intent of one and of two slots; this suite is about how the map's keys
//! follow the DECLARATION rather than the dialog, which is also why the second
//! test seeds a slot map through `PutSession` and talks to the bot afterwards.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::harness::Emulator;
use crate::recognize_text_nlu::{build, control, id, say, Bot, LOCALE, ROLE_ARN, THRESHOLD};

/// The one custom slot type every slot of this suite's bot elicits values
/// against, resolving under `TopResolution` so an answer has to be a value it
/// declares.
const CHOICE: &str = "Choice";

/// The slots the intents below declare, in the order their `slotPriorities`
/// elicit them, with the answer that fills each one: every answer is a
/// `sampleValue` of [`CHOICE`], so the slot type resolves it.
const DECLARED: [(&str, &str); 3] = [
    ("Flavor", "chocolate"),
    ("Size", "large"),
    ("Topping", "sprinkles"),
];

/// One intent per slot count the rule has to hold for: its name, the sample
/// utterance that names it without naming any slot value, and how many of
/// [`DECLARED`] it declares as `Required` slots.
const INTENTS: [(&str, &str, usize); 4] = [
    ("OrderNothing", "order a shake", 0),
    ("OrderOne", "order a cone", 1),
    ("OrderTwo", "order a sundae", 2),
    ("OrderThree", "order a float", 3),
];

/// `ElicitSlot` and `Close`, two members of the modelled `DialogActionType`
/// enum (`smithy_query(query="shape",shape="DialogActionType")`: Close,
/// ConfirmIntent, Delegate, ElicitIntent, ElicitSlot, None).
const ELICIT_SLOT: &str = "ElicitSlot";
/// See [`ELICIT_SLOT`].
const CLOSE: &str = "Close";

/// The `requestContentType` a text input stream carries ("Text format —
/// `text/plain; charset=utf-8`").
const TEXT_REQUEST_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// Author one bot whose `en_US` locale declares every intent of [`INTENTS`],
/// each with its own sample utterance and its own prefix of [`DECLARED`] as
/// prompted, `Required` slots of the [`CHOICE`] slot type.
async fn slot_map_bot(emulator: &Emulator) -> Bot {
    let created = control(
        emulator,
        "PUT",
        "/bots",
        json!({
            "botName": "DeclaredSlotMapBot",
            "roleArn": ROLE_ARN,
            "dataPrivacy": {"childDirected": false},
            "idleSessionTTLInSeconds": 300,
            "botType": "Bot",
        }),
        202,
        "CreateBot",
    )
    .await;
    let bot_id = id(&created, "botId");

    control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales"),
        json!({"localeId": LOCALE, "nluIntentConfidenceThreshold": THRESHOLD}),
        202,
        "CreateBotLocale",
    )
    .await;

    let slot_type_values: Vec<Value> = DECLARED
        .iter()
        .map(|(_, answer)| json!({"sampleValue": {"value": answer}}))
        .collect();
    let slot_type = control(
        emulator,
        "PUT",
        &format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/slottypes"),
        json!({
            "slotTypeName": CHOICE,
            "slotTypeValues": slot_type_values,
            "valueSelectionSetting": {"resolutionStrategy": "TopResolution"},
        }),
        200,
        "CreateSlotType",
    )
    .await;
    let slot_type_id = id(&slot_type, "slotTypeId");

    let intents = format!("/bots/{bot_id}/botversions/DRAFT/botlocales/{LOCALE}/intents");
    for (intent_name, utterance, count) in INTENTS {
        let mut authored = json!({
            "intentName": intent_name,
            "sampleUtterances": [{"utterance": utterance}],
        });
        let intent = control(
            emulator,
            "PUT",
            &intents,
            authored.clone(),
            200,
            "CreateIntent",
        )
        .await;
        let intent_id = id(&intent, "intentId");

        let mut priorities: Vec<Value> = Vec::new();
        for (priority, (slot_name, _)) in DECLARED.iter().take(count).enumerate() {
            let slot = control(
                emulator,
                "PUT",
                &format!("{intents}/{intent_id}/slots"),
                json!({
                    "slotName": slot_name,
                    "slotTypeId": slot_type_id,
                    "valueElicitationSetting": {
                        "slotConstraint": "Required",
                        "promptSpecification": {
                            "messageGroups": [{"message": {"plainTextMessage": {
                                "value": format!("Which {slot_name}?"),
                            }}}],
                            "maxRetries": 2,
                        },
                    },
                }),
                200,
                "CreateSlot",
            )
            .await;
            priorities.push(json!({"priority": priority + 1, "slotId": id(&slot, "slotId")}));
        }

        // A build requires every slot of the intent to be ordered by its
        // `slotPriorities`, and ordering them is `UpdateIntent`'s — which
        // replaces the intent, so everything the create gave it goes again.
        authored["slotPriorities"] = json!(priorities);
        control(
            emulator,
            "PUT",
            &format!("{intents}/{intent_id}"),
            authored,
            200,
            "UpdateIntent",
        )
        .await;
    }

    // The alias the runtime is addressed by is the one Amazon Lex keeps for
    // testing the bot, read off the control plane rather than assumed.
    let aliases = control(
        emulator,
        "POST",
        &format!("/bots/{bot_id}/botaliases"),
        json!({}),
        200,
        "ListBotAliases",
    )
    .await;
    let bot_alias_id = aliases
        .get("botAliasSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| summaries.first())
        .map(|summary| id(summary, "botAliasId"))
        .unwrap_or_else(|| panic!("the bot must have the test alias, got {aliases:?}"));

    build(emulator, &bot_id).await;

    Bot {
        bot_id,
        bot_alias_id,
        locale_id: LOCALE.to_string(),
    }
}

/// The slot map of a turn's intent, which the rule requires on every turn.
fn slot_map(case: &str, state: &Value) -> serde_json::Map<String, Value> {
    state
        .pointer("/intent/slots")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: intent.slots must be reported as a map, got {state}")
        })
}

/// Assert the map names exactly the slots `declared`, with a value for the
/// first `filled` of them (the ones the dialog elicits first) and the
/// documented `null` for the rest.
fn assert_declared(
    case: &str,
    map: &serde_json::Map<String, Value>,
    declared: &[(&str, &str)],
    filled: usize,
) {
    let mut names: Vec<&str> = map.keys().map(String::as_str).collect();
    names.sort_unstable();
    let mut expected: Vec<&str> = declared.iter().map(|(name, _)| *name).collect();
    expected.sort_unstable();
    assert_eq!(
        names, expected,
        "case {case:?}: the map names every slot the intent declares and nothing else, \
         got {map:?}"
    );

    for (index, (name, answer)) in declared.iter().enumerate() {
        let entry = map
            .get(*name)
            .unwrap_or_else(|| panic!("case {case:?}: slot {name:?} is named"));
        if index < filled {
            assert_eq!(
                entry.pointer("/value/interpretedValue").and_then(Value::as_str),
                Some(*answer),
                "case {case:?}: slot {name:?} holds what the conversation filled, got {entry}"
            );
        } else {
            assert!(
                entry.is_null(),
                "case {case:?}: slot {name:?} has no value yet, so it is the documented null \
                 rather than an object, got {entry}"
            );
        }
    }
}

/// The dialog action a turn ends on, and the slot it elicits.
fn action(case: &str, state: &Value) -> (String, Option<String>) {
    let action = state
        .pointer("/dialogAction")
        .unwrap_or_else(|| panic!("case {case:?}: a turn must report its dialog action, got {state}"));
    (
        action
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the dialog action has a type, got {action}"))
            .to_string(),
        action
            .get("slotToElicit")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// Assert the turn elicits the first declared slot that has no value yet, and
/// closes once every one of them has one.
fn assert_elicits(case: &str, state: &Value, declared: &[(&str, &str)], filled: usize) {
    let (kind, slot) = action(case, state);
    match declared.get(filled) {
        Some((open, _)) => {
            assert_eq!(
                (kind.as_str(), slot.as_deref()),
                (ELICIT_SLOT, Some(*open)),
                "case {case:?}: the open slot is the one the turn asks for, got {state}"
            );
        }
        None => {
            assert_eq!(
                (kind.as_str(), slot.as_deref()),
                (CLOSE, None),
                "case {case:?}: with nothing left open the dialog closes, got {state}"
            );
        }
    }
}

/// The session state `GetSession` reports for one conversation.
async fn stored_state(emulator: &Emulator, bot: &Bot, session_id: &str, case: &str) -> Value {
    let read = emulator.send("GET", &bot.session(session_id), None).await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: the conversation must be readable, got {} {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    read.json()
        .get("sessionState")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetSession reports the session state"))
}

/// The map's keys follow the intent's DECLARATION, over intents declaring none,
/// one, two and three slots, from the opening turn to the turn that fills the
/// last of them — in the turn's own answer and in every other carrier of the
/// same session state.
#[tokio::test]
async fn a_turn_names_every_slot_the_intent_declares_whatever_it_has_filled() {
    let emulator = Emulator::new();
    let bot = slot_map_bot(&emulator).await;

    for (intent_name, utterance, count) in INTENTS {
        let declared = &DECLARED[..count];
        let session = format!("{intent_name}-text");

        let case = format!("{intent_name} declaring {count} slot(s): the opening turn");
        let turn = say(&emulator, &bot, &session, utterance).await;
        let state = turn
            .get("sessionState")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: a turn reports its session state"));
        assert_eq!(
            state.pointer("/intent/name").and_then(Value::as_str),
            Some(intent_name),
            "case {case:?}: the utterance names this intent, got {turn}"
        );
        // Nothing is filled by an utterance that names no slot value, and every
        // declared slot is still reported.
        assert_declared(&case, &slot_map(&case, &state), declared, 0);
        assert_elicits(&case, &state, declared, 0);

        // The map is session state, so a read of the conversation reports it.
        let stored = stored_state(&emulator, &bot, &session, &case).await;
        assert_eq!(
            stored.pointer("/intent/slots"),
            state.pointer("/intent/slots"),
            "case {case:?}: GetSession reports the map the turn left, got {stored}"
        );

        // And so does the other encoding of the same turn, in its own gzipped,
        // base64-encoded session-state header.
        let spoken = emulator
            .send_raw(
                "POST",
                &format!("{}/utterance", bot.session(&format!("{intent_name}-utterance"))),
                Bytes::from(utterance.to_string()),
                TEXT_REQUEST_CONTENT_TYPE,
                &[],
            )
            .await;
        assert_eq!(
            spoken.status,
            200,
            "case {case:?}: RecognizeUtterance must succeed, got {} {:?}",
            spoken.status,
            String::from_utf8_lossy(&spoken.body)
        );
        let by_utterance = spoken
            .lex_header("x-amz-lex-session-state")
            .unwrap_or_else(|| panic!("case {case:?}: the session-state header is reported"));
        assert_eq!(
            by_utterance.pointer("/intent/slots"),
            state.pointer("/intent/slots"),
            "case {case:?}: RecognizeUtterance reports the same map, got {by_utterance}"
        );

        // Then answer each elicitation in turn: a slot that gains a value is
        // reported with it, and the ones still open keep their `null` keys.
        for (answered, (_, answer)) in declared.iter().enumerate() {
            let filled = answered + 1;
            let case =
                format!("{intent_name} declaring {count} slot(s): {filled} of them filled");
            let turn = say(&emulator, &bot, &session, answer).await;
            let state = turn
                .get("sessionState")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: a turn reports its session state"));
            assert_eq!(
                state.pointer("/intent/name").and_then(Value::as_str),
                Some(intent_name),
                "case {case:?}: an answer to an elicitation stays on the intent, got {turn}"
            );
            assert_declared(&case, &slot_map(&case, &state), declared, filled);
            assert_elicits(&case, &state, declared, filled);

            let stored = stored_state(&emulator, &bot, &session, &case).await;
            assert_eq!(
                stored.pointer("/intent/slots"),
                state.pointer("/intent/slots"),
                "case {case:?}: GetSession reports the map the turn left, got {stored}"
            );
        }
    }
}

/// A name the intent does not declare is not a slot of it: a caller can seed a
/// session's slot map with one through `PutSession`, and the turn that follows
/// reports the intent's own slots — the seeded value for the slot it declares,
/// and no key at all for the rest.
#[tokio::test]
async fn a_seeded_name_the_intent_does_not_declare_is_not_reported_as_a_slot() {
    let emulator = Emulator::new();
    let bot = slot_map_bot(&emulator).await;

    // The intent that declares exactly one slot, so one seeded entry is a slot
    // of it and every other one is not.
    let (intent_name, utterance, count) = INTENTS[1];
    let declared = &DECLARED[..count];
    let (slot_name, answer) = DECLARED[0];

    // (case, the extra name the caller seeds beside the declared slot) — a slot
    // name another intent of the locale declares, a name no intent of it
    // declares at all, and one that looks like a built-in slot type.
    let rows: [(&str, &str); 3] = [
        ("a slot another intent of the locale declares", DECLARED[1].0),
        ("a name nothing of the locale declares", "Cutlery"),
        ("a name shaped like a built-in slot type", "AMAZON.Number"),
    ];

    for (index, (case, stray)) in rows.iter().enumerate() {
        let session = format!("seeded-{index}");
        // The slot map the caller seeds: the slot the intent declares, plus the
        // row's own name, which it does not.
        let mut seeded_slots = serde_json::Map::new();
        seeded_slots.insert(
            slot_name.to_string(),
            json!({"value": {"interpretedValue": answer}}),
        );
        seeded_slots.insert(
            (*stray).to_string(),
            json!({"value": {"interpretedValue": "whatever"}}),
        );
        let seeded = emulator
            .send(
                "POST",
                &bot.session(&session),
                Some(json!({
                    "sessionState": {
                        "dialogAction": {"type": "Delegate"},
                        "intent": {
                            "name": intent_name,
                            "state": "InProgress",
                            "slots": seeded_slots,
                        },
                    },
                })),
            )
            .await;
        assert_eq!(
            seeded.status,
            200,
            "case {case:?}: PutSession must accept the seeded state, got {} {:?}",
            seeded.status,
            String::from_utf8_lossy(&seeded.body)
        );

        let turn = say(&emulator, &bot, &session, utterance).await;
        let state = turn
            .get("sessionState")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: a turn reports its session state"));
        let map = slot_map(case, &state);
        assert!(
            !map.contains_key(*stray),
            "case {case:?}: {stray:?} is not a slot of {intent_name}, so the map must not \
             name it, got {map:?}"
        );
        // The declared slot keeps the value the caller seeded for it, so the
        // dialog has nothing left to elicit.
        assert_declared(case, &map, declared, declared.len());
        assert_elicits(case, &state, declared, declared.len());

        let stored = stored_state(&emulator, &bot, &session, case).await;
        assert_eq!(
            stored.pointer("/intent/slots"),
            state.pointer("/intent/slots"),
            "case {case:?}: GetSession reports the map the turn left, got {stored}"
        );
    }
}
