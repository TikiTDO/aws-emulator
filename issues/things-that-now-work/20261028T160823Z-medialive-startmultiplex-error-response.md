Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a24b8bb497f295cf2fd83b42be71d5dbb9740df2

# A start of a multiplex whose delete was accepted answers the retryable 409 ConflictException "can only be started from IDLE", not the crate's tombstone 404

_Filed: 2026-10-28T16:08:23Z_
_Operation: StartMultiplex | Class: error-response_

> _Note: this issue was identified against the code base at commit `a24b8bb497f295cf2fd83b42be71d5dbb9740df2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.start_multiplex` resolves its id over the whole `multiplexes` store and then judges the record on its stored state, so a multiplex in `DELETING` or in the terminal `DELETED` it keeps falls into `MultiplexStartRefusal::NotStartable(state)`. `StartMultiplex` turns that into 409 `ConflictException` with the message "Multiplex <id> is in state DELETED and can only be started from IDLE" — a retry instruction for an id that can never become IDLE again — and the `BatchStart` multiplex arm reports the same tombstone with the per-entry code `ConflictException`.

That contradicts this crate's own documented rule for a mutation of a record a delete has already been accepted for, stated service-wide in `state.rs` and implemented by the channel path (`mutable_channel`), the input path (`live_input`), the cluster path (`live_cluster`), the node path (`NodeStateRefusal::DeleteAccepted`) and the tag-writer path (`resource_tags_mut`): resolution comes FIRST and a dead id is answered with the operation's own modelled not-found error, the state conflict being reserved for a resource that still exists. `BatchStart` states that rule for BOTH starts in its own `failure_code` module and then breaks it in its multiplex arm, inside one file.

The multiplex slice already carries the notion it needs (`MultiplexUpdateRefusal::Gone`, `MultiplexProgramRefusal::MultiplexGone`, whose docs say the record "names no multiplex a caller may still change"); `MultiplexStartRefusal` simply has no such variant, so the start path cannot tell a tombstone from a busy multiplex.

Scope note: the `DeleteMultiplex` 409-on-DELETED answer is pinned with a stated reason by `tests/all/multiplex_deletion.rs:206-262` ("a multiplex is deleted from the stopped state only"), so it is deliberately out of scope here. The START path's DELETING/DELETED rows are pinned by no test: `tests/all/multiplex_start.rs:256-263` covers only CREATING, STARTING and RUNNING.

## Evidence

The service-wide rule — services/service-com-amazonaws-medialive-medialive/src/state.rs:73-84:
```
//! A kept record is READABLE, not writable: once a delete has been accepted for
//! an id, that id names no resource a caller may still change. Every MUTATION
//! therefore resolves its id through the live lookups
//! ([`ComAmazonawsMedialiveMedialiveState::live_input`],
//! [`ComAmazonawsMedialiveMedialiveState::live_cluster`],
//! [`ComAmazonawsMedialiveMedialiveState::mutable_channel`] over
//! [`ComAmazonawsMedialiveMedialiveState::live_channel_mut`], and the tag
//! writers' own private `resource_tags_mut`) and answers a dead id with its own
//! modelled not-found error. That resolution comes FIRST, before the mutation
//! looks at the resource's state at all, so a tombstone is answered as not found
//! and never with the state conflict a resource that still exists gets, while a
//! read goes on reporting the record in its terminal state.
```
and the harm the crate names for doing otherwise — src/state.rs:3587-3591 (`mutable_channel`):
```
    /// Taking this refusal first is what keeps a state conflict
    /// (`ConflictException`) for a channel that still EXISTS: a tombstone has no
    /// state a caller can conflict with, so reporting one would tell a client to
    /// retry an id that will never become startable again.
```

The multiplex start that does the opposite — src/state/multiplexes.rs:1429-1442:
```
    pub fn start_multiplex(
        &mut self,
        multiplex_id: &str,
        now: SystemTime,
    ) -> Result<(), MultiplexStartRefusal> {
        let multiplex = self
            .multiplexes
            .get_mut(multiplex_id)
            .ok_or(MultiplexStartRefusal::NotFound)?;
        if !multiplex.is_startable() {
            return Err(MultiplexStartRefusal::NotStartable(
                multiplex.state.clone(),
            ));
        }
```
with `is_startable` admitting only IDLE — src/state/multiplexes.rs:901-906 ("never while it is still coming up, already starting or running, stopping, deleting or deleted") — and the handler mapping that to a conflict, src/ops/start_multiplex.rs:208-216:
```
        crate::state::MultiplexStartRefusal::NotStartable(state) => {
            StartMultiplexErrorModeled::ConflictException(ConflictException {
                message: Some(crate::state::multiplex_not_startable_message(
```
whose wording is src/state.rs:5396-5401: `"Multiplex {multiplex_id} is in state {state} and can only be started from {IDLE}"`.

The same file stating the rule and breaking it — src/ops/batch_start.rs:243-251:
```
    /// Neither an existing resource nor one a caller may still act on —
    /// `NotFoundException` (404), which both single-resource starts in this
    /// service model: no resource of that kind has that id, or the one it had
    /// has been deleted.
    pub const NOT_FOUND: &str = "NotFoundException";
    /// The resource exists but its current state does not allow a start —
    /// `ConflictException` (409), which both single-resource starts answer for a
    /// resource that has not stopped.
    pub const CONFLICT: &str = "ConflictException";
```
the channel arm honouring it — src/ops/batch_start.rs:305-314 ("so a channel whose delete has been accepted is reported with the NOT-FOUND code `StartChannel` answers for it rather than with a state conflict") — and the multiplex arm not, src/ops/batch_start.rs:352-358:
```
        Err(crate::state::MultiplexStartRefusal::NotFound) => Ok(not_found("Multiplex", id)),
        Err(crate::state::MultiplexStartRefusal::NotStartable(current)) => Ok(refused(
            id,
            state.multiplex_description(id).and_then(|m| m.arn),
            failure_code::CONFLICT,
```

The multiplex slice already models the tombstone class it does not use here — src/state/multiplexes.rs:404-409 (`MultiplexUpdateRefusal::Gone`): "The multiplex the request named is going away (`DELETING`) or gone (`DELETED`): its record is still readable, but a delete has already been accepted for that id, so it names no multiplex a caller may still change."

Test coverage: the channel side of the rule is pinned for every channel mutation and for all three batch forms — tests/all/post_delete_mutation_classification.rs:119-184 (`a_channel_mutation_after_the_delete_is_not_found_rather_than_a_conflict`, rows "while DELETING" / "once DELETED") and :238-293 (`a_batch_entry_for_a_deleted_channel_reports_not_found`, asserting `"code": "NotFoundException"` for BatchStart/BatchStop/BatchDelete). No test covers StartMultiplex on a DELETING/DELETED multiplex: tests/all/multiplex_start.rs:256-263 enumerates only `("while still creating", "CREATING"), ("while already starting", "STARTING"), ("while already running", "RUNNING")`.

## Steps to Reproduce

1. POST /prod/multiplexes with a name, two availabilityZones and valid multiplexSettings; note the returned id M.
2. Poll GET /prod/multiplexes/M until state is IDLE.
3. DELETE /prod/multiplexes/M -> 200 with state DELETING; wait for the transient span and confirm GET /prod/multiplexes/M reports state DELETED (the record is kept, per src/state/multiplexes.rs:922-929).
4. POST /prod/multiplexes/M/start -> the emulator answers 409 ConflictException "Multiplex M is in state DELETED and can only be started from IDLE". Expected, per src/state.rs:73-84 and src/ops/batch_start.rs:243-247, the operation's modelled 404 NotFoundException naming the id as already deleted — which is exactly what POST /prod/channels/<deleted-id>/start answers (pinned by tests/all/post_delete_mutation_classification.rs:119-184).
5. POST /prod/batch/start with {"multiplexIds":["M"]} -> the failed entry carries "code": "ConflictException", while the same batch's entry for a deleted channel carries "code": "NotFoundException" (tests/all/post_delete_mutation_classification.rs:280-291) — two classifications for one situation inside one handler.
6. Repeat step 4 inside the DELETING window (before the clock settles) for the same 409.
