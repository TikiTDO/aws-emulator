Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-ei5u6fnzrxadqojcfubtwa37ja"].deletes
Issue-Commit: 87807d9b7479bf5c1eafe07a63e234f45d99140a

# AppSync ListApiKeys returns the wrong ApiKey deletes grace period

> _Note: this issue was identified against the code base at commit `87807d9b7479bf5c1eafe07a63e234f45d99140a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
AppSync's ListApiKeys projection of ApiKey$deletes is derived with a shorter deletion grace period than AWS uses, so every listed da2 key reports a deletion time that is one fixed interval too early. 1 net-new cause.

Cause 1 of 1 — ListApiKeys reports ApiKey$deletes with the wrong grace-period offset.
AWS: for a da2 key, ListApiKeys "returns the expiration time and deletion time in seconds" and ApiKey$deletes is "the time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour" (Smithy: shape com.amazonaws.appsync#ApiKey, members expires and deletes, both target Long with default 0; ApiKey doc, da2 bullet list). AWS derives deletes from the key's expires value plus the service's fixed deletion grace period, so for a given expires the deletes value is determined, not free-running. The recording shows AWS's deletes sitting exactly one fixed interval (60 days, 5,184,000 s) beyond the value the emulator computes; status is 200 on both sides and no other member of the listed key diverges.
Emulator: serves the same key with deletes computed from a shorter grace period, so the listed value is 5,184,000 s lower than AWS's for the same expires.
Why: a client that schedules key rotation, renewal or inventory cleanup off the deletes it reads from ListApiKeys acts on a date AWS would not report — it believes keys are reclaimed while they are still live on AWS (or renews a key it thinks is about to vanish), and any assertion that CreateApiKey's deletes equals the deletes later listed for the same key is satisfied against the wrong absolute time.
Fix: the API-key state's deletes derivation as projected by the ListApiKeys handler — the same expires-plus-grace-period arithmetic shared with Create/UpdateApiKey, applied when listing.
Traces: appsync-api-key-lifecycle#task#4.

Related open issue that does NOT cover this step: "AppSync API-key deletion window and tagging responses diverge from AWS" (20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md) describes the same arithmetic on CreateApiKey/UpdateApiKey and carries a ListApiKeys replay key, but that key is bound to a different generated api-key id, so this cluster is not owned by it and would not be re-replayed when that issue is closed.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator up; /__admin/health 200).

appsync-api-key-lifecycle#task#4 ListApiKeys, verdict mismatch, expected status 200, actual status 200:
  $.apiKeys[id="da2-..."].deletes: value_mismatch (expected 1803945600, got 1798761600)

The two values differ by 5,184,000 seconds = 60 days. 1803945600 and 1798761600 are per-run absolute instants from this recording and are not themselves the requirement: the requirement is that deletes equals the key's expires plus AWS's fixed deletion grace period, rounded down to the hour, and that the value listed matches the one CreateApiKey/UpdateApiKey returned for that key. The api-key id in the JSON path is a generated value from this run.

Divergence key: service=appsync|ListApiKeys|status=200->200|keys=$.apiKeys[id="da2-..."].deletes (owned_by: none — net-new).

## Steps to Reproduce
1. git clone the repo, boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus; fixture appsync-api-key-lifecycle mismatches at step task#4.
3. By hand: CreateGraphqlApi, then CreateApiKey with an explicit expires value, note the deletes returned, then call ListApiKeys for the same apiId and compare the listed deletes against AWS's expires + fixed deletion grace period (the recording shows AWS 60 days beyond the emulator's value) and against the deletes CreateApiKey reported.
