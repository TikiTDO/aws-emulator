Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-boo6boyqxo7wbiseru4e6ndc3u"].deletes
Issue-Commit: 77267c797fffc6dc2dba7e94fff74073a30cc633

# AppSync ListApiKeys reports ApiKey deletes with wrong deletion grace period

> _Note: this issue was identified against the code base at commit `77267c797fffc6dc2dba7e94fff74073a30cc633`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
ListApiKeys projects each da2 API key's deletion time with a shorter grace period than AWS applies, so the listing surface reports a deletion date that does not exist on AWS. 1 cause.

Cause 1 of 1: ListApiKeys returns the wrong ApiKey$deletes for a listed da2 key.
AWS: ListApiKeys returns a list of ApiKey structures; for a da2 key it returns both the expiration time and the deletion time in seconds (Smithy: ApiKey docs, 'da2' bullet 'ListApiKeys returns the expiration time and deletion time in seconds'; ApiKey.deletes target Long, doc 'The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour'; ListApiKeys GET /v1/apis/{apiId}/apikeys, 200). AWS derives deletes deterministically from the key's expires value plus the service's fixed deletion grace period, so for a given expires there is exactly one correct deletes. In the recording the listed key's deletes sits a fixed 5,184,000 seconds (60 days) beyond the value the emulator computed, i.e. AWS applies the longer grace period the emulator does not.
Emulator: ListApiKeys serializes the key with a deletes computed from a shorter grace period than AWS's, so the value is smaller than AWS's by that fixed interval. Status and every other member of the listed key match; only the derived deletes value diverges.
Why: a client that inventories keys through ListApiKeys and schedules rotation, alerting or cleanup off deletes computes a reclamation date earlier than AWS's. It will treat keys that AWS still retains as already gone (and stop renewing them), or re-create keys it believes were reclaimed, because the only field describing the retention window is wrong.
Fix: the API-key handler's deletes derivation as projected by ListApiKeys (the expires -> deletes grace-period arithmetic shared by the API-key read path), not the fixture's values.
Traces: appsync-api-key-lifecycle#task#4 (ListApiKeys, 200).

Related open issue that does NOT carry this replay key: 20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md describes the same grace-period arithmetic for CreateApiKey/UpdateApiKey and mentions ListApiKeys, but its ListApiKeys Issue-Key embeds a different, run-generated API key id (da2-dq5e45sgwr3mamypsk3zabjbfx) than this run's divergence key, so this step's key is not owned by it and is reported net-new. A fixer correcting the shared derivation should expect both to close together.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.

appsync-api-key-lifecycle#task#4 ListApiKeys, expected status 200 / actual status 200, body mismatch:
  $.apiKeys[id="da2-..."].deletes: value_mismatch (expected 1803945600, got 1798761600)

The two values differ by exactly 5,184,000 seconds (60 days). The absolute epoch seconds and the key id above are per-run values from this recording; the requirement is the derivation (expires plus AWS's fixed deletion grace period, rounded down to the hour), not these literals. No other member of the listed key and no header or status diverged on this step.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus; fixture appsync-api-key-lifecycle mismatches at step task#4.
3. By hand: CreateGraphqlApi, then CreateApiKey with an explicit expires value, then GET /v1/apis/{apiId}/apikeys (ListApiKeys) and compare the listed key's deletes against expires: AWS's value is the expiry plus the service's fixed deletion grace period (60 days beyond the emulator's offset in the recording), rounded down to the hour.
