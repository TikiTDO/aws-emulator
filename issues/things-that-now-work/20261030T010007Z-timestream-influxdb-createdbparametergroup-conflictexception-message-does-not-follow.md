Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: timestream-influxdb.create-db-parameter-group-duplicate-name-conflict-rejected timestream-influxdb.create-db-parameter-group-missing-permission-denied
Issue-Shape-Id: com.amazonaws.timestreaminfluxdb#AmazonTimestreamInfluxDB
Issue-Key: timestream-influxdb.create-db-parameter-group-duplicate-name-conflict-rejected|run#9|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: timestream-influxdb.create-db-parameter-group-missing-permission-denied|_wait_for_access_denied#11|AssertionError|raise AssertionError(
Issue-Commit: 3a6da8cf7ceece0dec1bbe7fd5ea27cdc5dd3e62

# timestream-influxdb: CreateDbParameterGroup ConflictException message does not follow the AWS already-exists rule, and an unprivileged principal's create is executed instead of denied

> _Note: this issue was identified against the code base at commit `3a6da8cf7ceece0dec1bbe7fd5ea27cdc5dd3e62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDbParameterGroup

## Description
Two independent net-new root causes in the Timestream for InfluxDB emulator, both grounded in failing behavioral checks of this suite. Causes are numbered; EACH must be closed before this issue is submitted.

Related open issue, which does NOT cover either cause as it now manifests: 20261022T105412Z-timestream-influxdb-tagresource-stub-answers-501-notimplemented-no.md. Its Cause 2 described the authorization gap as it looked when CreateDbParameterGroup was still a 501 scaffold stub (the check timed out waiting for the call to settle on AccessDeniedException). CreateDbParameterGroup now answers, so the same missing gate presents as an entirely different and worse divergence — the unauthorized call SUCCEEDS with HTTP 200 and persists a resource — which is why the replay key no longer matches that issue and the check is not owned by it. Cause 2 below is the authoritative description of the gap in its current form.

=== Cause 1 of 2: ConflictException message for a duplicate DB-parameter-group name is free text, not the AWS already-exists rule (failing check: timestream-influxdb.create-db-parameter-group-duplicate-name-conflict-rejected) ===
AWS: Smithy (services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/smithy-model/com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb.json) models CreateDbParameterGroup with required input `name` (DbParameterGroupName, @length 3..64, @pattern ^[a-zA-Z][a-zA-Z0-9]*(-[a-zA-Z0-9]+)*$) and declares ConflictException with httpError 409; ConflictException is a client error whose members are required `message`, required `resourceId` and required `resourceType`. A second CreateDbParameterGroup reusing an already-taken name is refused with HTTP 409 / error code `ConflictException`, and the message is built by this service's already-exists rule rather than being free text. The rule is: the fixed text `The DbParameterGroup with id [`, `], name [`, `], customer [`, `] already exists`, with three variable parts filled from the emulator's state and the request — the id of the EXISTING group that owns the name (the `id` returned by the original create), the requested `name` from the request, and the caller's 12-digit account id. Rendered for the recorded request that is one example of that rule, not the requirement itself: "The DbParameterGroup with id [2cf10a89b3], name [sim2real-pgdup-f274c30339aa], customer [000000000000] already exists" — here `2cf10a89b3`, `sim2real-pgdup-f274c30339aa` and `000000000000` are the per-run values and everything else is fixed text.
Emulator: status and error code are right (409 / ConflictException) but the message is a generic RDS-style sentence that drops the conflicting resource's id and the account entirely and renames the resource type: `DB parameter group sim2real-pgdup-f274c30339aa already exists` (only the requested name is interpolated).
Why: a client that identifies the conflicting group from the error — the common pattern of parsing the id out of the already-exists message to then GetDbParameterGroup / adopt the existing group instead of failing — has nothing to parse, so reconciling create-if-absent logic breaks. Error-message conformance tests and log-based alerting keyed on this service's rule mismatch on every duplicate-name conflict, and the message names a different resource type than AWS does.
Fix: the CreateDbParameterGroup handler's duplicate-name branch in services/service-com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb/src/ops/create_db_parameter_group.rs — when the name is already taken, look up the existing group, and build the ConflictException from the rule above using that group's id, the requested name and the request's account id. While there, populate ConflictException's required `resourceId` (the existing group's id) and `resourceType` members, which the model marks required.
Traces: timestream-influxdb.create-db-parameter-group-duplicate-name-conflict-rejected. The same check's follow-on assertions confirm the rejection otherwise has no effect (the original description is not overwritten and no second group is created), so this cause is the message rule alone.

=== Cause 2 of 2: no identity-based authorization gate — a principal with no policies has its CreateDbParameterGroup EXECUTED and a resource persisted, instead of being denied (failing check: timestream-influxdb.create-db-parameter-group-missing-permission-denied) ===
AWS: authorization runs BEFORE the operation executes. Smithy declares AccessDeniedException (httpError 403, member required `message`) on CreateDbParameterGroup. A freshly created IAM user with no attached policies calling the action is refused with HTTP 403 / error code `AccessDeniedException`, and nothing is created. The message is generated from the request, so it is a rule: fixed text `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`, with the variable parts taken from the caller's identity, the action and the resource — the calling user's IAM ARN (arn:aws:iam::<account>:user/<user>), the action `timestream-influxdb:CreateDbParameterGroup`, and the resource ARN arn:aws:timestream-influxdb:<region>:<account>:db-parameter-group/*. The denial is independent of whether the call would otherwise have succeeded.
Emulator: the request signed with the unprivileged user's access key is dispatched straight to the handler and answered HTTP 200 with a real created group — the check observed `{'id': '4cb86f6550', 'name': 'sim2real-denied-...', 'arn': 'arn:aws:timestream-influxdb:us-west-2:000000000000:db-parameter-group/4cb86f6550', ...}` with HTTPStatusCode 200 — and the check aborts immediately with "real AWS denies this call, but it SUCCEEDED". No 403 and no AccessDeniedException is ever produced, and the denied call leaves a persisted parameter group behind. Hypothesis: the request walk for this service crate has no authorization step in front of handler dispatch, so principal and policy are never consulted.
Why: this is overly permissive in the strongest sense — unauthorized callers are indistinguishable from authorized ones AND their writes take effect. Any least-privilege or IAM-policy regression test written against AWS passes vacuously or inverts, SDK error handling never reaches the AccessDeniedException branch, 403 (terminal) vs 200 retry/terminal classification is wrong, and state is mutated by a principal AWS would have refused.
Fix: the service's request-pipeline authorization step (the authorization middleware of the shared server runtime as mounted for the timestream-influxdb service crate), evaluated before the per-operation handler so the 403 is returned regardless of whether the target operation is implemented — not inside create_db_parameter_group.rs. The same gate governs every operation of this service, not just this one.
Traces: timestream-influxdb.create-db-parameter-group-missing-permission-denied.

## Evidence
Suite: simparity-behavioral-aws-v1 — timestream-influxdb 2/19 (11%), 17 failed. Of those, 13 errors are already-filed unimplemented-scaffold clusters (CreateInternetGateway x9, ListDbParameterGroups x2, TagResource x2) and 2 mismatches are OWNED by 20261022T105412Z-timestream-influxdb-tagresource-stub-answers-501-notimplemented-no.md; the 2 mismatches below are the survivors.

Cause 1 — [mismatch] timestream-influxdb.create-db-parameter-group-duplicate-name-conflict-rejected:
  setup:    first = CreateDbParameterGroup(name=<unique>, description="original") -> 200, id=<pg_id>
  request:  CreateDbParameterGroup(name=<same name>, description="duplicate")
  expected: HTTP 409, Error.Code "ConflictException", Error.Message built by the rule
            "The DbParameterGroup with id [<existing pg id>], name [<requested name>], customer [<12-digit account>] already exists"
            (fixed text as quoted; id/name/account are request- and state-derived)
  actual:   Error.Code "ConflictException" (correct), Error.Message "DB parameter group sim2real-pgdup-f274c30339aa already exists"
  assertion: AssertionError: expected message 'The DbParameterGroup with id [2cf10a89b3], name [sim2real-pgdup-f274c30339aa], customer [000000000000] already exists', got 'DB parameter group sim2real-pgdup-f274c30339aa already exists'
  side effects correct: GetDbParameterGroup still shows description "original", and exactly one group carries the name.

Cause 2 — [mismatch] timestream-influxdb.create-db-parameter-group-missing-permission-denied:
  setup:    iam.CreateUser(<user>) + iam.CreateAccessKey(<user>), no policies attached; timestream-influxdb client re-signed with those credentials
  request:  CreateDbParameterGroup(name=<unique>) as that principal
  expected: HTTP 403, Error.Code "AccessDeniedException", Error.Message built by the rule
            "User: arn:aws:iam::<account>:user/<user> is not authorized to perform: timestream-influxdb:CreateDbParameterGroup on resource: arn:aws:timestream-influxdb:<region>:<account>:db-parameter-group/* because no identity-based policy allows the timestream-influxdb:CreateDbParameterGroup action"
            and NO parameter group created
  actual:   HTTP 200 with a created group — {'id': '4cb86f6550', 'name': 'sim2real-denied-3af82ddf5a04', 'arn': 'arn:aws:timestream-influxdb:us-west-2:000000000000:db-parameter-group/4cb86f6550'}, ResponseMetadata.HTTPStatusCode 200. No authorization decision is made at all.
  assertion: AssertionError: real AWS denies this call, but it SUCCEEDED and returned {...}
  note: this is a changed manifestation of the gap described as Cause 2 of the open TagResource issue, which recorded HTTP 501 NotImplemented back when CreateDbParameterGroup was a stub; the handler now answers, so the unauthorized call is executed.

## Steps to Reproduce
Boot the emulator from mainline and point a botocore timestream-influxdb client at it (region/account as configured by the harness).

Cause 1 (ConflictException message rule):
  name = "sim2real-pgdup-abc123"
  first = influx.create_db_parameter_group(name=name, description="original")   # 200, note first["id"]
  influx.create_db_parameter_group(name=name, description="duplicate")
  -> observe HTTP 409 ConflictException with Message "DB parameter group <name> already exists".
     AWS answers 409 ConflictException with "The DbParameterGroup with id [<first['id']>], name [<name>], customer [<account>] already exists".
     Also inspect the serialized error body for the model-required resourceId / resourceType members.

Cause 2 (missing authorization gate):
  iam.create_user(UserName="deny-user")
  k = iam.create_access_key(UserName="deny-user")["AccessKey"]   # no policies attached
  denied = boto3 timestream-influxdb client signed with k.AccessKeyId / k.SecretAccessKey
  denied.create_db_parameter_group(name="sim2real-denied-abc123")
  -> observe HTTP 200 and a persisted group (confirm with influx.list_db_parameter_groups()).
     AWS answers HTTP 403 AccessDeniedException with the not-authorized-to-perform message, before the operation runs, and creates nothing.

Re-replay these two checks to confirm the fix: timestream-influxdb.create-db-parameter-group-duplicate-name-conflict-rejected, timestream-influxdb.create-db-parameter-group-missing-permission-denied.
