Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.nodejs-core-module-surface lambda.nodejs-child-process-exec-sync lambda.nodejs-worker-threads-run lambda.nodejs-zlib-gzip-round-trip lambda.nodejs-web-globals-present lambda.nodejs-crypto-cipher-round-trip lambda.nodejs-resolves-package-exports-map lambda.nodejs-handler-reads-bundled-config-file lambda.nodejs-native-addon-from-npm-loads lambda.nodejs-package-json-dependency-tree-imports lambda.nodejs-package-json-esm-dependency-with-exports lambda.nodejs-package-json-lodash-and-dayjs lambda.python-asyncio-run-completes lambda.python-ctypes-loads-libc lambda.python-lzma-and-bz2-round-trip lambda.python-sqlite3-in-memory-query lambda.python-subprocess-runs-a-command lambda.python-zoneinfo-resolves lambda.python-ssl-default-context-verifies lambda.python-threadpool-executor-runs lambda.python-runtime-reports-configured-version
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.nodejs-core-module-surface|run#4|AssertionError|assert out["failed"] == [], f"core modules failed to load: {out['failed']!r}"
Issue-Key: lambda.nodejs-child-process-exec-sync|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.nodejs-worker-threads-run|_invoke_ok#4|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.nodejs-zlib-gzip-round-trip|_invoke_ok#5|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.nodejs-web-globals-present|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.nodejs-crypto-cipher-round-trip|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.nodejs-resolves-package-exports-map|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.nodejs-handler-reads-bundled-config-file|run#5|AssertionError|assert out["dirname"] == "/var/task", f"the handler's __dirname was {out['dirname']!r}"
Issue-Key: lambda.nodejs-native-addon-from-npm-loads|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-dependency-tree-imports|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-esm-dependency-with-exports|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.nodejs-package-json-lodash-and-dayjs|_npm_package#2|Timeout|r = subprocess.run(
Issue-Key: lambda.python-asyncio-run-completes|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-ctypes-loads-libc|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-lzma-and-bz2-round-trip|_invoke_ok#4|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-sqlite3-in-memory-query|_invoke_ok#4|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-subprocess-runs-a-command|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-zoneinfo-resolves|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-ssl-default-context-verifies|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-threadpool-executor-runs|_invoke_ok#3|AssertionError|assert err is None, f"the function failed: {err!r} {out!r}"
Issue-Key: lambda.python-runtime-reports-configured-version|run#4|AssertionError|assert out["version"] == [3, 12], (
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# Lambda managed runtimes are stub interpreters: 10 Node.js/Python execution-environment causes

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
The emulator's managed Lambda execution environments for nodejs*.x and python3.* are not Node.js and CPython; they are reduced interpreters whose module and global surface is a small subset of the real runtime. Ten distinct causes, each with its own failing checks. (Open issues 20261107T091747Z-lambda-the-execution-environment-has-no-filesystem-or.md and 20261107T135301Z-lambda-10-net-new-root-causes-in-the--r2.md describe neighbouring runtime gaps but do not carry any of the replay keys below, so none of these causes is covered by them.)

Cause 1 of 10: the Node.js runtime provides no Node core modules.
AWS: recorded behavior only - a nodejs managed runtime is Node.js, so `require(...)`/`import` of the built-in modules (http, https, net, dns, tls, zlib, child_process, worker_threads, perf_hooks, async_hooks, readline, v8, vm, ...) resolves and the handler runs. The model says nothing about the runtime's module set; Smithy: Invoke returns StatusCode 200 with no X-Amz-Function-Error when the handler succeeds.
Emulator: every core-module require fails and the invoke comes back with X-Amz-Function-Error: Unhandled and errorType `Runtime.ImportModuleError`, errorMessage `Error: Cannot find module '<module>'\nRequire stack:\n- /var/task/index.js\n- /var/runtime/index.mjs`. The fixed text is `Error: Cannot find module '<name>'` plus the require stack; `<name>` is the module the handler asked for. Observed for http, https, net, dns, tls, zlib, child_process, worker_threads, perf_hooks, async_hooks, readline, v8 and vm.
Why: no real Node handler runs - any function that opens a socket, spawns a process, compresses data or uses a worker fails at import, so a client testing its own Lambda code against the emulator sees an import error that AWS never produces.
Fix: the Node.js runtime provider of the Lambda invoke path - the module resolver/built-in registry of the emulator's JS execution environment.
Failing checks: lambda.nodejs-core-module-surface, lambda.nodejs-child-process-exec-sync, lambda.nodejs-worker-threads-run, lambda.nodejs-zlib-gzip-round-trip

Cause 2 of 10: the Node.js runtime is missing standard web globals.
AWS: recorded behavior only - `structuredClone` (and the other WHATWG globals Node 18+ exposes) is defined in a nodejs managed runtime.
Emulator: the handler fails with X-Amz-Function-Error: Unhandled, errorType `ReferenceError`, errorMessage `structuredClone is not defined`.
Why: handlers written against the documented global surface fail at runtime on the emulator only.
Fix: the global object the emulator's JS execution environment installs before the handler module is evaluated.
Failing checks: lambda.nodejs-web-globals-present

Cause 3 of 10: the Node.js runtime's crypto cipher API is not callable.
AWS: recorded behavior only - a nodejs runtime's `crypto.createCipheriv`/`createDecipheriv` round-trips a payload.
Emulator: the handler fails with X-Amz-Function-Error: Unhandled, errorType `TypeError`, errorMessage `not a function`, with a trace frame inside the emulator's own `eval_script` harness rather than Node.
Why: encryption/decryption in a handler cannot be exercised at all.
Fix: the crypto binding of the emulator's JS execution environment.
Failing checks: lambda.nodejs-crypto-cipher-round-trip

Cause 4 of 10: the Node.js module resolver ignores a package.json "exports" subpath map.
AWS: recorded behavior only - Node resolves `require('pkg/sub')`/`import 'pkg/sub'` through the dependency's package.json `exports` map.
Emulator: the handler fails with errorType `Error` and errorMessage `Cannot find module 'expkg/sub'` plus a require stack, where `expkg/sub` is the subpath specifier from the handler.
Why: any bundled dependency that publishes subpath exports (the common modern layout) is unloadable.
Fix: the subpath/`exports` resolution step of the emulator's JS module resolver.
Failing checks: lambda.nodejs-resolves-package-exports-map

Cause 5 of 10: `__dirname` in a Node handler carries a trailing slash.
AWS: recorded behavior only - in a CommonJS handler at /var/task/index.js, `__dirname` is the directory path with no trailing separator, i.e. `/var/task`.
Emulator: `__dirname` is reported as `/var/task/`.
Why: handlers that join `__dirname` with a relative asset path, or compare it to LAMBDA_TASK_ROOT, build a different path on the emulator than on AWS.
Fix: the environment the emulator's JS runtime builds for a CommonJS module (the `__dirname`/`__filename` pair).
Failing checks: lambda.nodejs-handler-reads-bundled-config-file

Cause 6 of 10: a Node package whose deployment bundle carries an npm dependency tree never finishes its invoke.
AWS: recorded behavior only - a zip that bundles node_modules (pure-JS dependencies, an ESM dependency with an exports map, or a native addon) is imported and the handler answers within the normal invoke latency.
Emulator: the check never reaches a verdict - the invoke does not return and the step ends `Check timed out after 60.0s`.
Why: the whole "deploy your real package" path is unusable, and a hang is worse than a refusal because a client cannot distinguish it from a slow function.
Fix: the deployment-package extraction/module-load path of the Lambda invoke provisioner for nodejs runtimes.
Failing checks: lambda.nodejs-native-addon-from-npm-loads, lambda.nodejs-package-json-dependency-tree-imports, lambda.nodejs-package-json-esm-dependency-with-exports, lambda.nodejs-package-json-lodash-and-dayjs

Cause 7 of 10: the Python runtime is missing CPython extension modules.
AWS: recorded behavior only - a python3.x managed runtime is CPython with its standard library, so `signal`/`asyncio`, `ctypes`, `bz2`/`lzma`, `sqlite3`, `subprocess` and `zoneinfo` all import.
Emulator: the invoke answers 200 with X-Amz-Function-Error: Unhandled and errorType `Runtime.ImportModuleError`; errorMessage is `Unable to import module '<handler module>': No module named '<ext>'` - fixed text `Unable to import module '...': No module named '...'`, with the handler module from the function's Handler and `<ext>` the missing C extension. Observed `_signal`, `_ctypes`, `_bz2`, `_sqlite3` and `_sysconfigdata_t_linux_aarch64-linux-gnu`.
Why: the majority of real handlers import at least one of these transitively (asyncio, requests, pandas, zoneinfo), so they fail at import on the emulator and never on AWS.
Fix: the Python execution environment's stdlib/extension-module set in the Lambda invoke runtime.
Failing checks: lambda.python-asyncio-run-completes, lambda.python-ctypes-loads-libc, lambda.python-lzma-and-bz2-round-trip, lambda.python-sqlite3-in-memory-query, lambda.python-subprocess-runs-a-command, lambda.python-zoneinfo-resolves

Cause 8 of 10: the Python runtime's `ssl` module is a stub.
AWS: recorded behavior only - `ssl.OPENSSL_VERSION` exists and `ssl.create_default_context()` verifies a TLS peer.
Emulator: the handler fails with errorType `AttributeError` and errorMessage `module 'ssl' has no attribute 'OPENSSL_VERSION'`.
Why: any handler that makes an HTTPS call with the stdlib (urllib, http.client) or inspects the TLS stack breaks.
Fix: the `ssl` module provided by the emulator's Python execution environment.
Failing checks: lambda.python-ssl-default-context-verifies

Cause 9 of 10: a Python handler cannot start a thread.
AWS: recorded behavior only - threads and `concurrent.futures.ThreadPoolExecutor` work inside a Lambda handler.
Emulator: the handler fails with errorType `RuntimeError` and errorMessage `a Lambda function in this emulator cannot start a thread`, raised from `threading.Thread.start`. That message is the emulator's own text; AWS has no such refusal.
Why: concurrent I/O fan-out inside a handler - a standard pattern - is impossible, and the error text tells the client it is talking to an emulator rather than reproducing AWS.
Fix: the threading support of the emulator's Python execution environment.
Failing checks: lambda.python-threadpool-executor-runs

Cause 10 of 10: the Python runtime reports a version that is not the configured runtime.
AWS: recorded behavior only - a function whose Runtime is `python3.12` reports `sys.version_info[:2] == (3, 12)`; the interpreter version follows the runtime identifier.
Emulator: a python3.12 function reports `sys.version_info[:2] == (3, 14)`, i.e. one fixed interpreter is used for every python runtime identifier.
Why: version-conditional handler code takes the wrong branch, and a client pinning a runtime cannot verify it got that runtime.
Fix: the runtime-identifier-to-interpreter selection in the Lambda invoke provisioner.
Failing checks: lambda.python-runtime-reports-configured-version

Traces: see the per-cause failing-check lists above; all are from the behavioral suite's `lambda` section.

## Evidence
Cause 1: expected handler success; got X-Amz-Function-Error 'Unhandled', errorType 'Runtime.ImportModuleError', e.g. "Error: Cannot find module 'zlib'\nRequire stack:\n- /var/task/index.js\n- /var/runtime/index.mjs". lambda.nodejs-core-module-surface lists http, https, net, dns, tls, zlib, child_process, worker_threads, perf_hooks, async_hooks, readline, v8, vm all failing the same way.
Cause 2: expected defined global; got errorType 'ReferenceError', errorMessage 'structuredClone is not defined'.
Cause 3: expected a cipher round trip; got errorType 'TypeError', errorMessage 'not a function' (trace frames: '/var/task/index.js:7:60', 'callHandler (eval_script:1506:46)').
Cause 4: expected 'expkg/sub' to resolve; got errorType 'Error', errorMessage "Cannot find module 'expkg/sub'".
Cause 5: expected __dirname == '/var/task'; got '/var/task/'.
Cause 6: expected a served invoke; got 'Check timed out after 60.0s' for all four npm-bundle checks.
Cause 7: expected the module to import; got errorType 'Runtime.ImportModuleError' with "Unable to import module 'index': No module named '_signal'" (asyncio, subprocess), "... No module named '_ctypes'", "... No module named '_bz2'", "... No module named '_sqlite3'", "... No module named '_sysconfigdata_t_linux_aarch64-linux-gnu'".
Cause 8: expected ssl.OPENSSL_VERSION; got errorType 'AttributeError', errorMessage "module 'ssl' has no attribute 'OPENSSL_VERSION'".
Cause 9: expected ThreadPoolExecutor.map to run; got errorType 'RuntimeError', errorMessage 'a Lambda function in this emulator cannot start a thread' from threading.py:1005 in start.
Cause 10: expected sys.version_info[:2] == (3, 12) for Runtime=python3.12; got [3, 14].

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1-6: CreateFunction with a nodejs managed runtime and a handler that requires a Node core module / uses structuredClone / crypto.createCipheriv / a package.json exports subpath / reads __dirname-relative files / bundles node_modules, then Invoke it and read X-Amz-Function-Error and the response payload.
3. Cause 7-10: CreateFunction with Runtime=python3.12 and a handler that imports asyncio, ctypes, bz2, lzma, sqlite3, subprocess or zoneinfo, uses ssl.OPENSSL_VERSION, starts a ThreadPoolExecutor, or returns sys.version_info, then Invoke it.
4. Re-replay exactly the check ids in trace_ids to confirm.
