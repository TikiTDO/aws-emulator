Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.cache-cluster-resp-protocol-round-trip-over-socket elasticache.e2e-serverless-snapshot-restore-flow
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.cache-cluster-resp-protocol-round-trip-over-socket|_probe_call#24|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Key: elasticache.e2e-serverless-snapshot-restore-flow|_probe_call#12|AssertionError|assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
Issue-Commit: 0a7cae03cb1d4210d38997f5682a54b52a42bce2

# ElastiCache in-VPC data plane still unverifiable: the emulator's Lambda python3.12 runtime has no _ssl module, so the RESP probe dies at import (1 cause, 2 unowned checks)

> _Note: this issue was identified against the code base at commit `0a7cae03cb1d4210d38997f5682a54b52a42bce2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One root cause blocks the two remaining ElastiCache data-plane checks that the replay report does not bind to any open issue. Related open issues report the SAME family but own a different set of replay keys, and the report binds neither id below to them, so this is re-grounded here: 20261108T152921Z-elasticache-in-vpc-data-plane-unverifiable-the-emulator.md (same _ssl symptom, different check keys — its key for cache-cluster-resp-protocol-round-trip-over-socket is pinned to frame `_probe_call#25`, which no longer matches the failure) and 20261105T090541Z-elasticache-every-in-vpc-data-plane-check-is.md (same family, symptom `os.register_at_fork` missing). Neither covers the two ids listed below.

Cause 1 of 1: the emulator's Lambda Python runtime cannot import the standard-library `ssl` module, so the in-VPC RESP probe handler fails at module import and no ElastiCache data-plane behavior is ever exercised.
AWS: recorded behavior only (no Smithy constraint governs a runtime image) — a function created with `Runtime="python3.12"`, `Handler="index.handler"` and a handler module whose first statement is `import socket, ssl` imports cleanly; `Invoke` returns HTTP 200 with NO `FunctionError` header and the handler's JSON payload as the body. That complete CPython standard library is what lets the probe open both a plaintext and a TLS RESP session to an ElastiCache endpoint from inside the VPC. The runtime an execution environment advertises (`AWS_LAMBDA_EXEC_ENV=AWS_Lambda_python3.12`) is a promise about the interpreter and its stdlib.
Emulator: `Invoke` answers 200 but with `FunctionError` set and the body `{"errorMessage": "Unable to import module 'index': No module named '_ssl'", "errorType": "Runtime.ImportModuleError", "requestId": "", "stackTrace": []}`. The fixed text of that message is AWS's own import-failure wording `Unable to import module '<module>': <ImportError message>` with `errorType` `Runtime.ImportModuleError`; the variable parts are the handler module name taken from the request's `Handler` (`index`) and the missing module name from the interpreter (`_ssl`). The `_ssl` extension module is simply absent from the runtime image, so EVERY handler that imports `ssl` dies before the handler function runs — this fires even for the plaintext session (`{"tls": false, ...}`) of cache-cluster-resp-protocol-round-trip-over-socket, because the import is at module scope.
Why: a client that provisions an ElastiCache cluster or serverless cache against this emulator cannot verify the cache serves data at all. Both checks reach their data-plane step with a cache the control plane reported `available` and an advertised `Endpoint.Address`/`Endpoint.Port`, then get nothing back: the RESP round trip (SET/GET, null vs empty bulk string, TTL -1/-2, INCR type error, unknown-command error, DEL idempotence) and the serverless snapshot/restore data-persistence proof are both unmeasurable. A transit-encryption-required endpoint is unreachable outright, since TLS needs `ssl`. The emulator reports success for CreateFunction and Invoke, so a client cannot distinguish 'my cache is broken' from 'this runtime has no TLS'.
Fix: the Lambda runtime image/interpreter the emulator executes functions with — it must ship a complete CPython standard library including the `_ssl` extension, and the interpreter must match the runtime identifier it advertises. (A previous replay recorded the interpreter as python 3.14.0.alpha while presenting `AWS_Lambda_python3.12`.)
Traces: elasticache.cache-cluster-resp-protocol-round-trip-over-socket (plaintext session, `_probe_call` via `run`), elasticache.e2e-serverless-snapshot-restore-flow (TLS session, `_probe_call` via `_tls_session`).

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout: 81/186 passed; elasticache 76/156.

1. elasticache.cache-cluster-resp-protocol-round-trip-over-socket [mismatch]
   check_elasticache_cache_cluster_resp_protocol_round_trip_over_socket.py, line 221, in run:
     (session,) = _probe_call(ctx, probe, host, port, [{"tls": False, "commands": plan}])
   -> line 508, in _probe_call: assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
   AssertionError: the in-VPC probe raised: {'errorMessage': "Unable to import module 'index': No module named '_ssl'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '', 'stackTrace': []}
   Note this session requests tls=False, so the failure is purely the module-scope `import socket, ssl` in the probe package.

2. elasticache.e2e-serverless-snapshot-restore-flow [mismatch]
   check_elasticache_e2e_serverless_snapshot_restore_flow.py, line 178, in _tls_session:
     (session,) = _probe_call(ctx, probe, host, port, [{"tls": True, "commands": [...]}])
   -> line 388, in _probe_call: assert "FunctionError" not in resp, f"the in-VPC probe raised: {out!r}"
   AssertionError: the in-VPC probe raised: {'errorMessage': "Unable to import module 'index': No module named '_ssl'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '', 'stackTrace': []}

Expected (AWS): Invoke returns 200 with no FunctionError and the body {"sessions": [{"replies": [...], "error": null}]}; for check 1 the replies prove RESP2 semantics ("+OK", "$<value>", null bulk for a missing key, integer -1/-2 from TTL, an "-ERR ... not an integer ..." reply from INCR on a string, an error reply for an unknown command, 1 then 0 from DEL); for check 2 the restored cache returns the value written before the snapshot.
Actual (emulator): no session ever runs; Invoke returns FunctionError with Runtime.ImportModuleError for `_ssl`.
Function names, hosts, ports, VPC/subnet/security-group ids and request ids in these runs are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Minimal reproduction, no ElastiCache needed: CreateRole for Lambda, then CreateFunction with Runtime='python3.12', Handler='index.handler' and a zip whose index.py begins `import socket, ssl` and defines `def handler(event, context): return {"ok": True}`; wait for State='Active', then Invoke it.
   Expect (AWS): HTTP 200, no FunctionError header, payload {"ok": true}.
   Observe: HTTP 200 with FunctionError set and payload {"errorMessage": "Unable to import module 'index': No module named '_ssl'", "errorType": "Runtime.ImportModuleError", "requestId": "", "stackTrace": []}.
3. Re-replay the two grounded checks to confirm the fix:
   - elasticache.cache-cluster-resp-protocol-round-trip-over-socket (plaintext RESP round trip against a cache.t3.micro redis cluster on port 6379 from an in-VPC Lambda).
   - elasticache.e2e-serverless-snapshot-restore-flow (TLS RESP SET/GET against a serverless cache, CreateServerlessCacheSnapshot, delete, restore via SnapshotArnsToRestore, then read the value back).
4. Both must reach their assertions on RESP replies rather than failing in _probe_call.
