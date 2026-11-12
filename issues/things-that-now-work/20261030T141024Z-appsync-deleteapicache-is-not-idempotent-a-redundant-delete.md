Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.op-contract-delete-api-cache
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.op-contract-delete-api-cache|run#18|AssertionError|assert no_op_code is None, f"expected the redundant delete to succeed as a no-op, got {no_op_code}"
Issue-Commit: 65165f71b3b3c14984c314d4becd7f5d11cc460b

# AppSync DeleteApiCache is not idempotent: a redundant delete on an API that holds no cache answers 404 NotFoundException where AWS answers 200 (1 cause)

> _Note: this issue was identified against the code base at commit `65165f71b3b3c14984c314d4becd7f5d11cc460b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApiCache

## Description
One net-new root cause in the AppSync ApiCache teardown path.

Cause 1 of 1 — DeleteApiCache on an API that holds no cache is refused instead of succeeding as a no-op.

AWS: DeleteApiCache is idempotent with respect to the absence of a cache. Once a cache has been deleted, a second DeleteApiCache for the same apiId returns HTTP 200 with the modeled empty DeleteApiCacheResponse body and no error. Smithy: DeleteApiCache input is `apiId` (String, @httpLabel, required); http {method: DELETE, uri: /v1/apis/{apiId}/ApiCaches, code: 200}; output shape DeleteApiCacheResponse has NO members; modeled errors are BadRequestException (400), ConcurrentModificationException (409), InternalFailureException (500), NotFoundException (404), UnauthorizedException (401). The model does not state which of those applies to an API whose cache slot is empty — recorded behavior only: the check documents, as measured on real AWS, that the redundant delete "succeed[s] as a no-op", and that the *following* GetApiCache is then refused 404 NotFoundException with the fixed sentence "Unable to get the cache as it doesn't exist, please create the cache first." (entirely fixed text — it carries no request or state values). So the not-found belongs to the reader of the absent cache, not to the deleter of it.

Emulator: the redundant DeleteApiCache is refused. The replayed call came back as a ClientError with error code `NotFoundException` (HTTP 404) where the check expected no error at all. The cause is in the handler: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_api_cache.rs treats an empty cache slot as a missing resource — after resolving the API it does `let Some(cache) = api.api_cache.as_ref() else { return Err(NotFoundException { message: absent_cache_message(CacheCall::Delete, api.api_cache_was_deleted()) }) }`, i.e. it asks for the absence itself and then refuses it. Its own module doc states this intent ("an API that holds no cache has no cache to delete — also a not-found"), so the divergence is by design in the emulator and not an edge case of the state machine.

Why: a client relying on AWS's behavior breaks on cleanup and on retry. Idempotent teardown is the normal shape of AppSync cache management — CloudFormation/CDK stack deletion, test fixtures and any at-exit cleanup issue DeleteApiCache unconditionally (or re-issue it after a timed-out/retried first attempt) precisely because AWS accepts it when there is nothing left to delete. Against this emulator that unconditional second delete raises NotFoundException, so the cleanup path fails, a retried delete reports failure even though the delete already succeeded, and callers cannot distinguish "the cache is gone" from "the request was wrong". It also hides the real distinction AWS makes: the absent-cache not-found is the GetApiCache/FlushApiCache/UpdateApiCache sentence, and the delete path should not be emitting a not-found sentence at all.

Fix: the DeleteApiCache handler's absent-cache branch. When the resolved API's cache slot is empty the handler must return the empty DeleteApiCacheOutput with the modeled 200 instead of constructing a NotFoundException, while leaving the cache-was-deleted marker in state intact so that the subsequent GetApiCache still selects the correct absent/deleted sentence. The apiId checks ahead of it (malformed apiId -> BadRequestException 400, unknown API -> NotFoundException 404) and the not-yet-created precondition (BadRequestException 400) are unchanged; only the "API exists, cache slot empty" branch is wrong. The absent_cache_message(CacheCall::Delete, ..) rendering becomes unreachable from this handler.

Related open issue that does NOT cover this: "AppSync ApiCache: refusal wording, encryption flags and applied update state all diverge (10 causes)" (20261029T200818Z-appsync-apicache-refusal-wording-encryption-flags-and-applied.md) — its ten causes are FlushApiCache/UpdateApiCache/CreateApiCache/GetApiCache wording, the two encryption flags, the un-applied update and the deleted-cache GetApiCache sentence. None of them is the DeleteApiCache idempotency contract, and it carries no key for this check.

Traces: appsync.op-contract-delete-api-cache.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout.

[mismatch] appsync.op-contract-delete-api-cache (group error-semantics, SERVICES=["appsync"]):

  File "check_appsync_op_contract_delete_api_cache.py", line 73, in run
    assert no_op_code is None, f"expected the redundant delete to succeed as a no-op, got {no_op_code}"
  AssertionError: expected the redundant delete to succeed as a no-op, got NotFoundException

The check reached that assertion, so every earlier step matched AWS: CreateGraphqlApi, CreateApiCache(ttl=300, apiCachingBehavior=FULL_REQUEST_CACHING, type=SMALL), the poll to status AVAILABLE, the first DeleteApiCache, the poll until GetApiCache reports the cache gone, and the post-delete GetApiCache refusal 404 NotFoundException "Unable to get the cache as it has been deleted, please re-create the cache."

Expected (AWS): the second `delete_api_cache(apiId=<api id>)` returns HTTP 200, empty DeleteApiCacheResponse body, no error; the GetApiCache that follows it is refused 404 NotFoundException with the fixed sentence "Unable to get the cache as it doesn't exist, please create the cache first."
Actual (emulator): the second delete_api_cache raises a ClientError with Error.Code `NotFoundException` (HTTP 404), so the check never reached the follow-on GetApiCache assertion.

Root cause in the checkout: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_api_cache.rs lines 185-198 — the `api.api_cache.as_ref()` else-branch returns DeleteApiCacheErrorModeled::NotFoundException with state::absent_cache_message(CacheCall::Delete, api.api_cache_was_deleted()). Module doc lines 14-17 state the same (wrong) intent.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the behavioral check `appsync.op-contract-delete-api-cache` from simparity-behavioral-aws-v1.
3. By hand, against the emulator's appsync endpoint:
   a. CreateGraphqlApi(name=<unique>, authenticationType="API_KEY") -> note apiId.
   b. CreateApiCache(apiId, ttl=300, apiCachingBehavior="FULL_REQUEST_CACHING", type="SMALL").
   c. Poll GetApiCache(apiId) until apiCache.status == "AVAILABLE".
   d. DeleteApiCache(apiId) -> 200 (this one is correct).
   e. Poll GetApiCache(apiId) until it is refused 404 NotFoundException.
   f. DeleteApiCache(apiId) a SECOND time. Observed: 404 NotFoundException. Expected: HTTP 200 with the empty DeleteApiCacheResponse body.
   g. GetApiCache(apiId) again. Expected: 404 NotFoundException with message "Unable to get the cache as it doesn't exist, please create the cache first."
