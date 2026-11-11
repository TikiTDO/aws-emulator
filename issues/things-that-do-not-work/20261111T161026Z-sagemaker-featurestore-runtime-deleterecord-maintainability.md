Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f0a035afe3a6d4bc3a732ff8fc28ae95e972cd65

# DeleteRecord re-implements the shared TargetStores enum guard inline instead of calling crate::state::undeclared_target_store

_Filed: 2026-11-11T16:10:26Z_
_Operation: DeleteRecord | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f0a035afe3a6d4bc3a732ff8fc28ae95e972cd65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate centralises the `TargetStores` undeclared-value guard in `crate::state::undeclared_target_store`, whose own doc says it is "stated once so no two of them word it differently", and PutRecord, UpdateRecord and BatchWriteRecord all call it. DeleteRecord instead inlines the same rule — the same `undeclared_value(..., TARGET_STORE_VALUES)` lookup and a hand-copied duplicate of the helper's refusal message — so the one rule exists twice in this crate and the two copies can drift (a change to the shared message would silently leave DeleteRecord wording it the old way). The two copies produce byte-identical text today, so replacing the inline block with the helper call leaves every response and every existing test unchanged.

## Evidence

The shared helper, services/.../src/state.rs:592-607:
```
/// Every write of this data plane reads the list the same way — "is the OnlineStore
/// among these?" ([`targets_online_store`]) — so an undeclared value would be read as
/// "not the OnlineStore" and silently turn the write into a no-op success. The boundary
/// validator refuses it first; this is the handlers' own guard, stated once so no two of
/// them word it differently.
pub fn undeclared_target_store(target_stores: Option<&Vec<String>>) -> Option<String> {
    let unknown = undeclared_value(target_stores?, crate::ops::types::TARGET_STORE_VALUES)?;
    Some(format!(
        "TargetStores: '{unknown}' is not a TargetStore; expected one of {}",
        crate::ops::types::TARGET_STORE_VALUES.join(", ")
    ))
}
```
The second copy, services/.../src/ops/delete_record.rs:215-222:
```
        if let Some(stores) = input.target_stores.as_ref() {
            if let Some(unknown) = crate::state::undeclared_value(stores, TARGET_STORE_VALUES) {
                return Err(validation(format!(
                    "TargetStores: '{unknown}' is not a TargetStore; expected one of {}",
                    TARGET_STORE_VALUES.join(", ")
                )));
            }
        }
```
The callers that do use the helper: services/.../src/ops/put_record.rs:206 `if let Some(message) = crate::state::undeclared_target_store(input.target_stores.as_ref())`, services/.../src/ops/update_record.rs:237 (same line), services/.../src/ops/batch_write_record.rs:236-238 (same, per entry).
Test coverage of both copies: tests/all/online_store_record_lifecycle.rs:1268-1290 exercises the "PutRecord TargetStores typo" case (helper) and the "DeleteRecord TargetStores typo" case (inline copy) side by side, and tests/all/refusal_message_wording.rs asserts the refusal texts — so the messages are pinned and must stay identical after the de-duplication.

## Steps to Reproduce

1. Read services/.../src/state.rs:592-607: the one `TargetStores` guard, documented as "stated once so no two of them word it differently".
2. Read services/.../src/ops/delete_record.rs:215-222: the same rule and the same message literal written out again, bypassing that helper.
3. Note that put_record.rs:206, update_record.rs:237 and batch_write_record.rs:236 all call the helper, so DeleteRecord is the lone second copy.
4. Replace the inline block with `if let Some(message) = crate::state::undeclared_target_store(input.target_stores.as_ref()) { return Err(validation(message)); }` and re-run tests/all/online_store_record_lifecycle.rs and tests/all/refusal_message_wording.rs: responses are unchanged.
