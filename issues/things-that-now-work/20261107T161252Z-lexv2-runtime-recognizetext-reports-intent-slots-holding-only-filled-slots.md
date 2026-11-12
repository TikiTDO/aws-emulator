Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-runtime.recognize-text-elicits-missing-required-slot
Issue-Shape-Id: com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0
Issue-Key: lexv2-runtime.recognize-text-elicits-missing-required-slot|run#12|AssertionError|assert intent["slots"] == {_SLOT: None}, (
Issue-Commit: 46d4bc769d688157e8bc6d12ab890b12b1627210

# RecognizeText reports Intent.slots holding only FILLED slots, omitting the intent's declared-but-unfilled slots instead of keying them to null

> _Note: this issue was identified against the code base at commit `46d4bc769d688157e8bc6d12ab890b12b1627210`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RecognizeText

## Description
One net-new root cause in the lexv2-runtime dialog turn.

Related open issue, which does NOT cover this: 20261107T092347Z-lexv2-runtime-dialog-turn-diverges-on-intent-state.md. Its Cause 2 describes Intent.slots being ABSENT from the serialized Intent (the check then died with KeyError: 'slots'). That half has landed — the member is now always present — but the map's CONTENTS are still wrong, and the check now fails on a different assertion with a different exception (AssertionError, slots were {}), so this divergence is not the one that issue's replay key carries.

Cause 1 of 1: Intent.slots is reported as a map of only the slots the conversation has FILLED, so on a turn that elicits a required slot the map comes back empty instead of naming that slot with a null value.
AWS: Smithy com.amazonaws.lexruntimev2#Intent.slots targets Slots (map<NonEmptyString, Slot>) and is documented "A map of all of the slots for the intent. The name of the slot maps to the value of the slot. If a slot has not been filled, the value is null." So a turn that recognizes an intent reports one key per slot the intent DECLARES; each slot with no value yet maps to null. For an intent declaring a single Required slot and nothing filled, the turn that elicits it reports sessionState.intent.slots = {"<slotName>": null} (dialogAction.type ElicitSlot, dialogAction.slotToElicit = that slot name, intent.state InProgress — all three of which the emulator now gets right), and GetSession on the same botId/botAliasId/localeId/sessionId reports the same map. The slot NAMES are values from the authored locale (the intent's declared slots), not from the request; the null value for an unfilled slot is the fixed rule.
Emulator: the map is present but carries only filled entries, so the elicitation turn reports slots = {} — "AssertionError: slots were {}, expected {'Flavor': None} -- the slot must be reported and unfilled". In src/state/nlu.rs::interpret the `slots` map is seeded only from the slots the session already filled (lines 162-164) and then gains an entry only when an utterance capture or an elicitation answer resolves a value (lines 170-186); it is serialized as `slots: Some(slots)` at line 241, so a declared slot that has no value yet never appears as a key at all. The module comment at lines 237-240 states the weaker rule it implements ("the map is reported on every turn — empty while nothing is filled"), which is the defect: AWS reports the declared slots, not just the filled ones.
Why: a Lex dialog loop indexes sessionState.intent.slots to learn which of the intent's slots are still open and which already have values. Against the emulator an intent mid-elicitation looks like an intent with NO slots: client code that iterates the map to render progress, to decide whether to prompt, or to count remaining slots sees zero work outstanding while the bot is in fact asking a question, and a client cannot distinguish "slot declared and unfilled" from "intent declares no slots". Code that reads slots["<name>"]["value"] expecting an explicit null entry raises instead.
Fix: the turn component in the runtime crate (src/state/nlu.rs::interpret, where the Intent is assembled) must populate Intent.slots with one key for EVERY slot the recognized intent declares in the built locale, mapping each unfilled slot to a null value and each filled slot to its Slot, and the session state it persists must carry that same map so GetSession (and PutSession/RecognizeUtterance, which serialize the same sessionState) report it identically.
Traces: lexv2-runtime.recognize-text-elicits-missing-required-slot.

## Evidence
Suite: lexv2-runtime 6/10 (60%), 4 failed; lexv2-models__lexv2-runtime 4/4 (100%). Three of the four failures are marked OWNED by open issues and were skipped. The one clustered here:

[mismatch] lexv2-runtime.recognize-text-elicits-missing-required-slot
  AssertionError: slots were {}, expected {'Flavor': None} -- the slot must be reported and unfilled
  Call: RecognizeText(botId=<bot>, botAliasId=<DRAFT test alias>, localeId=en_US, sessionId=<fresh>, text="order a shake") against a built en_US locale whose OrderShake intent declares one Required slot (Flavor, over a custom chocolate/vanilla/strawberry slot type) with promptSpecification "What flavor would you like?".
  Expected (AWS): sessionState.intent.slots present as {"Flavor": null} — one key per slot the intent declares, null for the unfilled one; GetSession on the same labels reports the same map.
  Actual: sessionState.intent.slots == {} — the member is present (that part was fixed) but holds no key for the declared, unfilled slot. dialogAction.type ElicitSlot, dialogAction.slotToElicit "Flavor" and intent.state "InProgress" all passed, so only the map contents diverge.

Source evidence (services/service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0): src/state/nlu.rs:162-164 (the map is seeded only from slots the session already filled), :170-186 (entries added only when a value resolves), :237-241 (serialized as Some(slots) with the module comment asserting the weaker "empty while nothing is filled" rule).

Smithy cited: com.amazonaws.lexruntimev2#Intent.slots -> Slots (map<NonEmptyString, Slot>), doc "A map of all of the slots for the intent. The name of the slot maps to the value of the slot. If a slot has not been filled, the value is null."

## Steps to Reproduce
1. lexv2-models: CreateBot with an IAM role lexv2.amazonaws.com can assume; wait for botStatus Available. CreateBotLocale botVersion DRAFT, localeId en_US, nluIntentConfidenceThreshold 0.40. CreateSlotType "Flavor" with sample values chocolate/vanilla/strawberry. CreateIntent "OrderShake" with sampleUtterances ["i would like a {Flavor} shake", "order a shake", "i would like a shake"] and intentClosingSetting closingResponse "Your shake is on the way.". CreateSlot "Flavor" with slotConstraint Required and promptSpecification "What flavor would you like?"; UpdateIntent with slotPriorities for it. Note the bot's DRAFT test alias id.
2. BuildBotLocale(botId, DRAFT, en_US) and poll DescribeBotLocale until botLocaleStatus is Built.
3. lexv2-runtime: RecognizeText(botId, botAliasId=<the DRAFT test alias>, localeId=en_US, sessionId=<fresh>, text="order a shake") — an utterance that names the intent but not the slot.
4. Read sessionState.intent["slots"]. Emulator: {} (empty map). AWS: {"Flavor": null} — the declared slot keyed with a null value, alongside dialogAction.type ElicitSlot / slotToElicit "Flavor" and intent.state InProgress.
5. GetSession(botId, botAliasId, localeId=en_US, sessionId=<the same session>) and read sessionState.intent["slots"]. Emulator: {}. AWS: the same {"Flavor": null} map, because the elicitation state is persisted.
