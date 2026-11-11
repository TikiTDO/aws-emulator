Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty
Issue-Key: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty|_prove_executor_in_vpc#1|AssertionError|raise _no_executor(
Issue-Commit: a1d0ee0a674a3edb6363f02dafc3236b87b0de24

# kafka__logs__s3: the MSK broker-log delivery check measures nothing — the Lambda python runtime has no `select` module, so the in-VPC executor cannot import kafka-python

> _Note: this issue was identified against the code base at commit `a1d0ee0a674a3edb6363f02dafc3236b87b0de24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1 (check: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty).

Real MSK brokers resolve only inside the cluster's own VPC, so this check carries its producer into the VPC in a Lambda function with VpcConfig whose deployment package vendors kafka-python, and first proves that executor usable with a `selftest` invocation. The proof fails on the vendored import, so no broker traffic is ever produced and nothing is observed about broker logs reaching CloudWatch Logs and S3 or about the unwired log group staying empty.

AWS: recorded behavior only — the Smithy model for lambda Invoke says nothing about the contents of a runtime's standard library. A managed python3.x runtime ships a complete, unmodified CPython standard library, so `import select` (a built-in extension module, and a dependency of `socket`/`selectors` and therefore of kafka-python) succeeds. The selftest invocation answers HTTP 200 with the handler's JSON payload, no FunctionError, and reports the vendored kafka-python's `__version__`.
Emulator: the handler module imports and runs (it returns the nonce-bound sha256 proof and the VPC resolver probe), but `import kafka` inside it raises and the payload reports `IMPORT FAILED: ModuleNotFoundError: No module named 'select'`. The emulator's in-process python runtime exposes a partial standard library with no `select` module.
Why: any function whose own code or vendored dependency touches `select`, `socket` or `selectors` cannot run on the emulator, and the developer is pointed at their deployment package for a defect that lives in the emulator's runtime. Here it replaces every verdict about MSK broker-log delivery with "the instrument is unusable".
Fix: the emulator's in-process Python Lambda runtime — the sandbox/stdlib shim that backs Invoke for the python runtimes — must expose the real CPython standard library, including the `select` extension module, so a package vendoring kafka-python imports cleanly and the selftest Invoke returns the handler's payload.
Related: 20261105T101629Z-kafka-logs-s3-the-msk-broker-log-delivery.md reports the earlier `os.register_at_fork` gap of the same shim for this check and does not carry this failure's key; the kafka-heading twin of this cause is bundled as Cause 3 of the issue filed for service `kafka` in this sweep (check kafka.msk-transactional-abort-invisible-to-read-committed-visible-to-read-uncommitted). Fixing the runtime closes both.
Traces: kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty (`_prove_executor_in_vpc`, check line 995).

## Evidence
Expected (AWS): `lambda.Invoke(FunctionName=<in-VPC executor>, Payload={"op":"selftest",...})` -> HTTP 200, no FunctionError, payload field kafka_python=<the vendored version string>.
Actual (emulator): HTTP 200, payload field kafka_python="IMPORT FAILED: ModuleNotFoundError: No module named 'select'".
Suite assertion:
  AssertionError: the in-VPC executor is unusable, so this run measured nothing: the vendored kafka-python did not import inside the executor: "IMPORT FAILED: ModuleNotFoundError: No module named 'select'"
(The function name, nonce and resolver address are per-run values and are not part of the requirement; the importability of the stdlib is.)

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an IAM role with AWSLambdaVPCAccessExecutionRole attached.
3. CreateFunction with Runtime=python3.x, Handler=index.handler, VpcConfig pointing at a subnet + security group of the shared network, and a zip whose package vendors kafka-python.
4. Invoke the function with {"op":"selftest","nonce":"<hex>","resolver":"<vpc resolver>","dns_name":"amazonaws.com"}.
5. Observe the returned payload reports `IMPORT FAILED: ModuleNotFoundError: No module named 'select'` instead of the kafka-python version.
6. Re-replay kafka__logs__s3.msk-broker-logs-reach-cloudwatch-logs-and-s3-while-an-unwired-log-group-stays-empty.
