Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.concurrent-update-api-cache appsync.flush-api-cache-deleted-cache-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.concurrent-update-api-cache|run#21|AssertionError|assert msg_after == "Unable to get the cache as it has been deleted, please re-create the cache.", (
Issue-Key: appsync.flush-api-cache-deleted-cache-rejected|_error#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: 64124677a1e83b958c06aaf4813fe97fc9a0a2ac

# AppSync ApiCache teardown: 2 net-new causes — a racing duplicate DeleteApiCache erases the "was deleted" state so GetApiCache reports the never-configured sentence, and DeleteApiCache on an API that never had a cache answers 200 instead of 404 NotFoundException

> _Note: this issue was identified against the code base at commit `64124677a1e83b958c06aaf4813fe97fc9a0a2ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApiCache

## Description
Two independent net-new root causes in the AppSync ApiCache teardown path, both grounded in failing behavioral checks of this service. 2 causes in this issue; BOTH must be closed before it is submitted.

Context on what does NOT cover these: the resolved issue 20261029T174359Z-appsync-apicache-wrong-notfoundexception-badrequestexception-messages-for-get.md carries both of these checks' replay keys, but it is fixed and therefore covers neither failure (both checks fail again, on the causes below). The resolved issue 20261030T141024Z-appsync-deleteapicache-is-not-idempotent-a-redundant-delete.md is the direct neighbour of Cause 2: its fix made DeleteApiCache answer 200 for EVERY empty cache slot, which is right for the redundant delete it was grounded in (appsync.op-contract-delete-api-cache, now passing) but wrong for an API that never had a cache. The open issue 20261029T200818Z-appsync-apicache-refusal-wording-encryption-flags-and-applied.md and 20261109T150433Z-appsync-4-net-new-causes-apicache-creating-state.md cover ApiCache wording/encryption/applied-update state and the CREATING-state gate; neither is the teardown-window state machine or the never-configured delete refusal below, and neither carries these two checks.

Cause 1 of 2: a duplicate DeleteApiCache issued while the cache is still being torn down erases the "this cache was deleted" state, so the terminal GetApiCache reports the never-configured sentence.
AWS: Smithy — DeleteApiCache DELETE /v1/apis/{apiId}/ApiCaches, input apiId (String, @httpLabel, required), empty DeleteApiCacheResponse, success code 200, modeled errors BadRequestException|400, ConcurrentModificationException|409, InternalFailureException|500, NotFoundException|404, UnauthorizedException|401; GetApiCache GET /v1/apis/{apiId}/ApiCaches with the same error set. Recorded behavior only for the teardown window: a cache that has been deleted LINGERS in status DELETING, so a second DeleteApiCache fired at the same time still finds an instance to remove and is also accepted (the check states real AppSync lets more than one racer win), and once the teardown settles GetApiCache is refused 404 NotFoundException with the sentence "Unable to get the cache as it has been deleted, please re-create the cache." — entirely fixed text apart from the operation verb ("get"), which the operation itself supplies; it carries no request or state values. AWS keeps the distinction between a cache that WAS torn down and an API that never had one for as long as the slot stays empty; only a sequential redundant delete issued AFTER the teardown has fully settled returns the API to the never-configured state (that leg is what appsync.op-contract-delete-api-cache records, and it still passes).
Emulator: after two concurrent DeleteApiCache calls on an AVAILABLE cache, the polled terminal GetApiCache answers 404 NotFoundException with "Unable to get the cache as it doesn't exist, please create the cache first." — the never-configured sentence for a cache that was just deleted. The error code and status are correct; the message is not. Cause: the delete removes the record synchronously instead of parking it in DELETING, so the second racer finds the slot already empty, and the state writer then clears the marker the first delete set.
Why: a client cannot tell "your cache was torn down, re-create it" from "you never configured caching on this API" whenever its teardown is retried or issued concurrently — which is the normal shape of cleanup code and of CloudFormation/CDK stack deletion. Delete-then-verify tooling reports the wrong remediation, and any logic that keys on the deleted wording never fires. It also means the emulator's ApiCache has no DELETING phase at all, so a client that waits for it observes a state machine AWS does not have.
Fix: the DeleteApiCache handler plus the ApiCache state component's teardown — model the DELETING window (park the cache in status DELETING and drop the record when the clock settles it, as the state already does for the CREATING transition) so a delete arriving during teardown still removes an instance and keeps the deletion marker set; equivalently, a teardown that finds the slot already empty must never clear a deletion the previous teardown recorded.
Traces: appsync.concurrent-update-api-cache. Code sites for orientation: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_api_cache.rs lines 208-218 (the record is removed inline and the call returns the empty 200 output), services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/state.rs lines 166-180 (take_api_cache sets api_cache_deleted = self.api_cache.take().is_some(), i.e. an empty slot RESETS the marker to false; api_cache_was_deleted feeds absent_cache_message) and state.rs lines 2839-2859 (cache_deleted_message / absent_cache_message, which already render the correct sentence when the marker survives).

Cause 2 of 2: DeleteApiCache on an API that never had a cache succeeds with 200 instead of being refused 404 NotFoundException.
AWS: recorded behavior — DeleteApiCache against a GraphQL API on which CreateApiCache was never called is refused 404 NotFoundException with the sentence "Unable to delete the cache as it doesn't exist, please create the cache first." (fixed text apart from the operation verb "delete", which the operation supplies; no request or state values appear in it). Smithy backs the class: DeleteApiCache models NotFoundException with httpError 404. The idempotent 200 AWS gives is specific to a REDUNDANT delete of a cache that this API did have and that a previous delete removed (appsync.op-contract-delete-api-cache), so presence-of-a-prior-cache is what separates the 200 from the 404 — absence alone does not.
Emulator: the call is accepted — HTTP 200 with the empty DeleteApiCacheResponse body ({}), no error — for an API whose cache slot was never filled. The companion FlushApiCache and GetApiCache refusals on the same API are reached only after this call, so the whole never-configured negative class is unverifiable against the emulator.
Why: a client relying on AWS's behavior loses a refusal it is entitled to: a teardown aimed at the wrong apiId, or at an API where caching was never enabled, reports success, so provisioning/validation code that probes with DeleteApiCache concludes a cache existed and was removed. A negative test for this class cannot pass, and the three ApiCache operations no longer agree on the resource's nonexistence (Get and Flush refuse 404 where Delete returns 200).
Fix: the DeleteApiCache handler's empty-cache-slot branch — it must distinguish the two reasons the slot is empty, refusing 404 NotFoundException with the delete-verb never-configured sentence when no cache was ever created under the API, and keeping the empty 200 only when a previous teardown removed one (the state component already records which case applies).
Traces: appsync.flush-api-cache-deleted-cache-rejected. Code site for orientation: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_api_cache.rs lines 208-218 serve the 200 for both reasons; state.rs api_cache_was_deleted (lines 175-180) and absent_cache_message/cache_not_found_message (lines 2850-2859) already hold the distinction and the correct sentence.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 158/424 passed, bootstrap_signal false (the emulator serves; the appsync control plane answered every earlier step of both checks).

Cause 1 — [mismatch] appsync.concurrent-update-api-cache (group concurrency, SERVICES=["appsync"]):
  File "check_appsync_concurrent_update_api_cache.py", line 164, in run
    assert msg_after == "Unable to get the cache as it has been deleted, please re-create the cache."
  AssertionError: unexpected message: "Unable to get the cache as it doesn't exist, please create the cache first."
The check reached that assertion, so everything before it matched AWS: CreateGraphqlApi(API_KEY), CreateApiCache(ttl=300, FULL_REQUEST_CACHING, SMALL), the poll to status AVAILABLE, five concurrent UpdateApiCache racers (one winner, losers ConcurrentModificationException "Unable to update cache due to concurrent updates"), and two concurrent DeleteApiCache racers with at least one winner. It then polled GetApiCache until it stopped reporting a present cache.
Expected (AWS): 404 NotFoundException, message "Unable to get the cache as it has been deleted, please re-create the cache."
Actual (emulator): 404 NotFoundException, message "Unable to get the cache as it doesn't exist, please create the cache first." — correct code and status, never-configured sentence.

Cause 2 — [mismatch] appsync.flush-api-cache-deleted-cache-rejected (group error-semantics, SERVICES=["appsync"]):
  File "check_appsync_flush_api_cache_deleted_cache_rejected.py", line 23, in run
    delete_code, delete_message = _error(lambda: appsync.delete_api_cache(apiId=api_id))
  File "check_appsync_flush_api_cache_deleted_cache_rejected.py", line 74, in _error
    raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
  AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-type': 'application/json', 'content-length': '2' ...}}
The only preceding step is CreateGraphqlApi(name=<unique>, authenticationType="API_KEY"); no CreateApiCache was ever issued for that apiId.
Expected (AWS): 404 NotFoundException, message "Unable to delete the cache as it doesn't exist, please create the cache first."
Actual (emulator): HTTP 200, empty DeleteApiCacheResponse body (content-length 2, i.e. {}), no error. (The response date/request-id values are per-run and are not part of the requirement.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay simparity-behavioral-aws-v1; the two checks under Traces fail.
3. Cause 1 by hand against the emulator's appsync endpoint:
   a. CreateGraphqlApi(name=<unique>, authenticationType="API_KEY") -> note apiId.
   b. CreateApiCache(apiId, ttl=300, apiCachingBehavior="FULL_REQUEST_CACHING", type="SMALL").
   c. Poll GetApiCache(apiId) until apiCache.status == "AVAILABLE".
   d. Fire 5 UpdateApiCache(apiId, ttl=300+10*i, FULL_REQUEST_CACHING, SMALL) concurrently (at least one 200; losers 409 ConcurrentModificationException).
   e. Fire 2 DeleteApiCache(apiId) concurrently. On AWS the cache lingers in status DELETING, so both are accepted.
   f. Poll GetApiCache(apiId) until it is refused. Expect 404 NotFoundException "Unable to get the cache as it has been deleted, please re-create the cache."; the emulator answers "Unable to get the cache as it doesn't exist, please create the cache first." because the second delete found the slot already empty and cleared the deletion marker.
   g. Regression guard: the SEQUENTIAL variant (delete once, poll until gone, delete again, then GetApiCache) must keep answering the never-configured sentence — that is appsync.op-contract-delete-api-cache, which currently passes.
4. Cause 2 by hand:
   a. CreateGraphqlApi(name=<unique>, authenticationType="API_KEY") -> note apiId. Do NOT call CreateApiCache.
   b. DeleteApiCache(apiId). Expect 404 NotFoundException "Unable to delete the cache as it doesn't exist, please create the cache first."; the emulator returns HTTP 200 with an empty body.
   c. Then FlushApiCache(apiId) and GetApiCache(apiId) on the same API — both must be 404 NotFoundException with "Unable to flush the cache as it doesn't exist, please create the cache first." and "Unable to get the cache as it doesn't exist, please create the cache first." respectively, and CreateApiCache followed by GetApiCache/FlushApiCache must then find the cache (the check's positive control).
