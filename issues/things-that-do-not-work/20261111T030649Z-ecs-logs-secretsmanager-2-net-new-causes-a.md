Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role ecs__logs__secretsmanager.ecs-secret-json-key-selector-injects-only-that-key
Issue-Key: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role|run#18|AssertionError|assert bad_task.get("stopCode") == "TaskFailedToStart", (
Issue-Key: ecs__logs__secretsmanager.ecs-secret-json-key-selector-injects-only-that-key|run#20|AssertionError|assert code == _OK, (
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# ecs__logs__secretsmanager: 2 net-new causes — a task whose EXECUTION role cannot read a secret is not failed at start, and a secrets JSON-key selector injects nothing

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Two net-new root causes. Neither check is marked ALREADY FILED nor OWNED.

Cause 1 of 2: Secrets Manager secret injection is not authorized against the execution role before the containers start.
AWS: containerDefinitions[].secrets are fetched by the ECS agent with the task's EXECUTION role. When the execution role may not perform secretsmanager:GetSecretValue on the referenced secret (even if the TASK role may), ECS fails the task before any container runs: DescribeTasks reports lastStatus STOPPED, stopCode TaskFailedToStart, and a stoppedReason with the fixed prefix `ResourceInitializationError: unable to pull secrets or registry auth: execution resource retrieval failed: unable to retrieve secret from asm:` followed by AWS's refusal wording naming the denied principal, the action secretsmanager:GetSecretValue and the secret arn. The principal arn and secret arn are values from the request and the emulator's IAM state; the prefix and connecting phrases are fixed. Smithy: ecs TaskStopCode enum includes TaskFailedToStart; ContainerDefinition.secrets; the ResourceInitializationError wording is AWS-documented stopped-task behavior.
Emulator: the task is admitted and stops with stopCode 'EssentialContainerExited' / reason 'Essential container in task exited'. The execution-role-vs-task-role distinction — the single most common ECS secrets mistake — is unobservable.
Why: a client that withholds the grant from the execution role is told the task started normally, so an unauthorized secret reference looks like a working deployment and the deploy gate keyed on stopCode TaskFailedToStart never fires.
Fix: the ECS task-start path must resolve containerDefinitions[].secrets through Secrets Manager under the EXECUTION role only, before launching containers, and report STOPPED / TaskFailedToStart / the ResourceInitializationError stoppedReason when refused.
Traces: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role.

Cause 2 of 2: a secret reference carrying a JSON key selector injects nothing.
AWS: a secrets[].valueFrom of the form `<secret-arn>:<json-key>::` injects ONLY that key's value from the secret's JSON document into the container's environment variable, while a bare `<secret-arn>` injects the whole document. Both forms are resolved at task start.
Emulator: the environment variable fed by the json-key form is empty/unset — nothing was injected for the selector — and the container ends with exitCode 4 and stoppedReason 'Essential container in task exited'.
Why: the json-key form is the normal way to inject a single credential field (a password, a host) without exposing the whole secret. A client using it gets an empty variable, so the application starts with no credential — and because the task is reported as having run, the misconfiguration is attributed to the application.
Fix: the ECS secrets resolution path's parsing of valueFrom — it must honour the `:<json-key>::` suffix (and the version-stage/version-id positions) and select that key out of the secret's JSON document, falling back to the whole document only for the bare arn form.
Traces: ecs__logs__secretsmanager.ecs-secret-json-key-selector-injects-only-that-key.

## Evidence
Cause 1 — Expected: stopCode 'TaskFailedToStart' with a stoppedReason beginning 'ResourceInitializationError: unable to pull secrets or registry auth: execution resource retrieval failed: unable to retrieve secret from asm:'. Actual: `AssertionError: a task whose EXECUTION role cannot read the secret was not failed at start: stopCode=EssentialContainerExited reason=Essential container in task exited`.
Cause 2 — Expected: the json-key reference injects exactly the selected key's value and the bare reference the whole document. Actual: `AssertionError: the json-key reference must inject exactly the PASSWORD value and the bare reference the whole document: JUST_KEY was empty or unset — nothing was injected for the selector (exitCode=4, reason=None, stoppedReason='Essential container in task exited')`.

## Steps to Reproduce
Cause 1: create a secret; grant secretsmanager:GetSecretValue to the TASK role only; register a task definition referencing it through containerDefinitions[].secrets; RunTask and poll DescribeTasks for stopCode/stoppedReason.
Cause 2: create a secret whose SecretString is a JSON document with a PASSWORD key; register a task definition with two secrets entries — one valueFrom '<secret-arn>:PASSWORD::' and one bare '<secret-arn>' — and have the container echo both variables; RunTask and read the awslogs output.
