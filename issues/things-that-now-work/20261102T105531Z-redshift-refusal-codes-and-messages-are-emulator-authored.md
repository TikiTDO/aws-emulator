Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.parameter-group-delete-default-fails redshift.parameter-group-lifecycle redshift.parameter-group-non-boolean-value-rejected redshift.parameter-group-out-of-range-concurrency-scaling-rejected redshift.cluster-already-exists-fault redshift.snapshot-duplicate-and-ungranted-revoke-fail redshift.tags-unknown-resource-fails
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.parameter-group-delete-default-fails|run#16|AssertionError|assert code == "InvalidParameterValue", (
Issue-Key: redshift.parameter-group-lifecycle|run#32|AssertionError|assert modify_message == "Parameter group does not exist", (
Issue-Key: redshift.parameter-group-non-boolean-value-rejected|run#8|AssertionError|assert code == "InvalidParameterValue", f"a non-boolean require_ssl should be InvalidParameterValue, got {code!r}"
Issue-Key: redshift.parameter-group-out-of-range-concurrency-scaling-rejected|run#12|AssertionError|assert got == want, f"11 is outside 0-10 and should be refused with {want!r}, got {got!r}"
Issue-Key: redshift.cluster-already-exists-fault|run#7|AssertionError|assert error["Message"] == "Cluster already exists", (
Issue-Key: redshift.snapshot-duplicate-and-ungranted-revoke-fail|run#14|AssertionError|assert dup_message == (
Issue-Key: redshift.tags-unknown-resource-fails|run#14|AssertionError|assert message == f"cluster ({bogus_cluster}) was not found.", message
Issue-Commit: 276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2

# Redshift refusal codes and messages are emulator-authored: parameter-group delete/modify, parameter-value validation, duplicate cluster/snapshot and CreateTags not-found (6 causes)

> _Note: this issue was identified against the code base at commit `276fdc9b01f0fecfb4ec23b6285a10289d7fb0d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteClusterParameterGroup, ModifyClusterParameterGroup, CreateCluster, CreateClusterSnapshot, CreateTags

## Description
Six net-new root causes, all in the redshift (query-protocol) control plane. Each is a refusal whose error CODE and/or MESSAGE is authored by the emulator instead of reproducing the wire behavior AWS returns for that error class. Positive controls in every one of these checks passed, so the surrounding operations work; only the refusal payloads diverge.

=== Cause 1 of 6: DeleteClusterParameterGroup on a service-owned default.* group answers the wrong error code and message ===
Failing check: redshift.parameter-group-delete-default-fails
AWS: deleting a service-owned default parameter group (name beginning `default.`, e.g. `default.redshift-1.0`) is permanently refused with HTTP 400, error code `InvalidParameterValue`, message `The Cluster Parameter Group cannot be deleted: <ParameterGroupName>` — fixed text `The Cluster Parameter Group cannot be deleted: ` plus the requested ParameterGroupName taken from the request. Smithy (redshift): DeleteClusterParameterGroup models only ClusterParameterGroupNotFoundFault (awsQueryError ClusterParameterGroupNotFound, 404) and InvalidClusterParameterGroupStateFault (awsQueryError InvalidClusterParameterGroupState, 400) — InvalidParameterValue is the service-common query-protocol refusal, so this is recorded behavior only for the code/message pair, and the recording shows `InvalidParameterValue`.
Emulator: answers error code `InvalidClusterParameterGroupState`. Per its own Smithy documentation that code means "another task is in progress that involves the parameter group. Wait a few moments and try the operation again."
Why: a client cannot tell a permanent refusal from a transient one. InvalidClusterParameterGroupState is documented as retryable, so SDK/retry and IaC code that backs off on it will loop until its own timeout instead of surfacing a hard configuration error; code-driven branches ("is this group deletable?") take the wrong path.
Fix: the DeleteClusterParameterGroup handler's validation — detect the service-owned `default.` prefix before the state check and raise the query-protocol InvalidParameterValue with the message template above.
Traces: redshift.parameter-group-delete-default-fails (the same check's control — deleting an ordinary group, and the ClusterParameterGroupNotFound message `ClusterParameterGroup <name> not found.` — both passed).

=== Cause 2 of 6: ModifyClusterParameterGroup's not-found message is emulator-authored ===
Failing check: redshift.parameter-group-lifecycle
AWS: ModifyClusterParameterGroup naming a group that does not exist is refused with ClusterParameterGroupNotFound (Smithy: ClusterParameterGroupNotFoundFault, awsQueryError ClusterParameterGroupNotFound, httpError 404) and the message is the fixed string `Parameter group does not exist` — it carries no variable part, and in particular does not name the group. Note that AWS's wording for this code differs per operation: DescribeClusterParameterGroups answers `ClusterParameterGroup <name> not found.` for the same group, and the emulator already gets that one right in the same check.
Emulator: answers the correct code but the message `ParameterGroup not found: <group-name>` — wrong fixed text, and it appends an identifier AWS omits here.
Why: operators and tests that match on the refusal text (log scrapers, error-mapping layers, snapshot tests of CLI output) diverge; the per-operation wording difference that AWS actually exhibits is lost.
Fix: the ModifyClusterParameterGroup handler's not-found path — emit the operation-specific message rather than a shared "<Type> not found: <id>" formatter.
Traces: redshift.parameter-group-lifecycle (create/modify/reset/delete and the DescribeClusterParameterGroups not-found message all passed first).

=== Cause 3 of 6: a non-boolean parameter value is refused as ValidationException instead of InvalidParameterValue ===
Failing check: redshift.parameter-group-non-boolean-value-rejected
AWS: ModifyClusterParameterGroup with a boolean parameter set to a non-boolean value (require_ssl=`maybe`) is refused with HTTP 400, error code `InvalidParameterValue` and message `parameter value is not a valid boolean:<value>` — fixed text `parameter value is not a valid boolean:` plus the rejected value from the request. The whole batch is refused atomically (the valid auto_mv in the same Parameters list is not applied, and a pre-existing user override survives). Recorded behavior only for the code/message pair: Smithy models only ClusterParameterGroupNotFoundFault and InvalidClusterParameterGroupStateFault on this operation, so the value check is the service-common query-protocol InvalidParameterValue.
Emulator: answers error code `ValidationException` — a generic, non-query-protocol code that is not among the operation's modeled errors nor a redshift query-protocol error code.
Why: botocore surfaces the code verbatim, so every client that branches on `InvalidParameterValue` (retry classification, user-facing validation messages, IaC drift handling) sees an unknown code and treats a well-defined parameter-validation refusal as an unexpected failure.
Fix: redshift request/parameter validation and its error serializer — parameter-value checks must raise the query-protocol InvalidParameterValue with AWS's message template, not a generic ValidationException.
Traces: redshift.parameter-group-non-boolean-value-rejected. Related open issue 20261029T200555Z-redshift-request-validation-answers-a-generic-validationexception-or.md describes the same generic-ValidationException family for OTHER parameter-group checks; it does not carry this check's replay key and the suite did not mark this failure owned, so it is filed here and that issue is named for context.

=== Cause 4 of 6: an out-of-range parameter value is refused as ValidationException with emulator-authored text ===
Failing check: redshift.parameter-group-out-of-range-concurrency-scaling-rejected
AWS: ModifyClusterParameterGroup with max_concurrency_scaling_clusters outside the advertised 0-10 range is refused with HTTP 400, error code `InvalidParameterValue` and message `The max_concurrency_scaling_clusters of <value> is outside of the acceptable range of <min>-<max>` — fixed text `The `, ` of `, ` is outside of the acceptable range of `, with the parameter name, the rejected value from the request and the range the catalogue advertises as the variable parts.
Emulator: answers code `ValidationException` and the message `Invalid value for Parameters.Parameter.1.ParameterValue: '<value>' is not a value parameter '<name>' allows. Allowed values: <min>-<max>` — a wire-location-shaped message AWS does not use for this refusal.
Why: same as cause 3 for the code; additionally the message leaks the query serialization index (`Parameters.Parameter.1.ParameterValue`) instead of naming the parameter the operator set, so the refusal is not actionable in the vocabulary the request used.
Fix: same component as cause 3 — the parameter range check in redshift parameter validation must raise InvalidParameterValue with AWS's range message template.
Traces: redshift.parameter-group-out-of-range-concurrency-scaling-rejected. Same relation to 20261029T200555Z-redshift-request-validation-answers-a-generic-validationexception-or.md as cause 3.

=== Cause 5 of 6: ClusterAlreadyExists and ClusterSnapshotAlreadyExists messages are emulator-authored "<Type> already exists: <id>" strings ===
Failing checks: redshift.cluster-already-exists-fault, redshift.snapshot-duplicate-and-ungranted-revoke-fail
AWS: CreateCluster re-using an existing ClusterIdentifier is refused with ClusterAlreadyExists (Smithy: ClusterAlreadyExistsFault, awsQueryError ClusterAlreadyExists, httpError 400) and the message is the fixed string `Cluster already exists` with no variable part at all. CreateClusterSnapshot re-using an existing SnapshotIdentifier is refused with ClusterSnapshotAlreadyExists (Smithy: ClusterSnapshotAlreadyExistsFault, awsQueryError ClusterSnapshotAlreadyExists, httpError 400) and the message is `Cannot create the snapshot because a snapshot with the identifier <SnapshotIdentifier> already exists.` — fixed text `Cannot create the snapshot because a snapshot with the identifier ` + the requested identifier + ` already exists.` (note the trailing period).
Emulator: emits one shared template for both — `Cluster already exists: <ClusterIdentifier>` and `Cluster snapshot already exists: <SnapshotIdentifier>`. In both cases the code is right and the text is wrong; for the cluster case the emulator adds an identifier AWS does not include, and for the snapshot case it replaces AWS's whole sentence.
Why: AWS's already-exists wording is per-resource (one message names the identifier, the other does not); a client, CLI transcript test or operator runbook matching the documented text fails against the emulator, and the two refusals become indistinguishable in logs apart from the leading noun.
Fix: the CreateCluster and CreateClusterSnapshot handlers' duplicate-identifier paths must each render their own AWS message template instead of a generic "<Type> already exists: <id>" formatter.
Traces: redshift.cluster-already-exists-fault; redshift.snapshot-duplicate-and-ungranted-revoke-fail (its duplicate-create assertion; the check's later AccessToSnapshotDenied assertions were never reached). Related open issue 20261029T192501Z-redshift-snapshot-refusal-messages-and-batch-snapshot-error.md covers other snapshot refusal messages; it does not carry this check's replay key and the suite did not mark this failure owned, so it is filed here and that issue is named for context.

=== Cause 6 of 6: CreateTags on an absent resource reports the ARN instead of AWS's "<type> (<name>) was not found." ===
Failing check: redshift.tags-unknown-resource-fails
AWS: CreateTags whose ResourceName is a well-formed redshift ARN with no backing resource is refused with ResourceNotFoundFault (Smithy: ResourceNotFoundFault, awsQueryError ResourceNotFoundFault, httpError 404) and the message names the resource TYPE and the bare resource NAME parsed out of the ARN: `cluster (<cluster-identifier>) was not found.` — fixed text `cluster (`, `) was not found.` with the identifier taken from the ARN's resource part (and the corresponding noun for other redshift resource types).
Emulator: answers the correct code but the message `The resource could not be found: <full-ARN>` — which is the Smithy doc sentence for the error shape plus the raw ARN, not AWS's wire message.
Why: the client loses the type/identifier decomposition AWS provides and instead gets an echo of its own input; text-matching callers break, and a message that is identical for every resource type cannot distinguish a missing cluster from a missing snapshot or parameter group.
Fix: the CreateTags handler's resource-resolution failure path — parse the ARN's resource type and identifier and render the per-type not-found template, instead of reusing the generic shape documentation string.
Traces: redshift.tags-unknown-resource-fails (its positive control — CreateTags/DescribeTags against a real parametergroup ARN — passed, and the rejected call left the unrelated resource's tags untouched). Related open issue 20261029T192501Z-redshift-read-payloads-omit-modeled-members-and-tagging.md covers other tagging divergences; it does not carry this check's replay key and the suite did not mark this failure owned, so it is filed here and that issue is named for context.

## Evidence
Cause 1 — DeleteClusterParameterGroup(ParameterGroupName='default.redshift-1.0'): expected code 'InvalidParameterValue' with message 'The Cluster Parameter Group cannot be deleted: default.redshift-1.0'; actual code 'InvalidClusterParameterGroupState'. Assertion: "deleting the service-owned default.redshift-1.0 must be rejected with InvalidParameterValue, got InvalidClusterParameterGroupState".

Cause 2 — ModifyClusterParameterGroup on a just-deleted group: code ClusterParameterGroupNotFound (correct); expected message 'Parameter group does not exist', actual 'ParameterGroup not found: <group-name>'. The same check's DescribeClusterParameterGroups message 'ClusterParameterGroup <group-name> not found.' matched.

Cause 3 — ModifyClusterParameterGroup(Parameters=[auto_mv=false, require_ssl='maybe']): expected ('InvalidParameterValue', 'parameter value is not a valid boolean:maybe'); actual code 'ValidationException'. Assertion: "a non-boolean require_ssl should be InvalidParameterValue, got 'ValidationException'".

Cause 4 — ModifyClusterParameterGroup(max_concurrency_scaling_clusters='11'): expected ('InvalidParameterValue', 'The max_concurrency_scaling_clusters of 11 is outside of the acceptable range of 0-10'); actual ('ValidationException', "Invalid value for Parameters.Parameter.1.ParameterValue: '11' is not a value parameter 'max_concurrency_scaling_clusters' allows. Allowed values: 0-10").

Cause 5 — CreateCluster with an in-use ClusterIdentifier: expected code ClusterAlreadyExists with message exactly 'Cluster already exists'; actual message 'Cluster already exists: <cluster-identifier>'. CreateClusterSnapshot with an in-use SnapshotIdentifier: expected code ClusterSnapshotAlreadyExists with message 'Cannot create the snapshot because a snapshot with the identifier <snapshot-id> already exists.'; actual 'Cluster snapshot already exists: <snapshot-id>'.

Cause 6 — CreateTags(ResourceName='arn:aws:redshift:<region>:<account>:cluster:<absent-cluster>'): expected code ResourceNotFoundFault with message 'cluster (<absent-cluster>) was not found.'; actual message 'The resource could not be found: arn:aws:redshift:<region>:<account>:cluster:<absent-cluster>'.

(Identifiers, regions and account numbers above are per-run values that fill the template; only the fixed text and the request-derived variable parts are the requirement.)

## Steps to Reproduce
Cause 1: redshift DescribeClusterParameterGroups(MaxRecords=100), pick a group whose name starts with 'default.', then DeleteClusterParameterGroup(ParameterGroupName=<that group>) and read Error.Code / Error.Message. Replay: redshift.parameter-group-delete-default-fails.
Cause 2: CreateClusterParameterGroup(ParameterGroupFamily='redshift-1.0'), DeleteClusterParameterGroup on it, then ModifyClusterParameterGroup(ParameterGroupName=<same name>, Parameters=[{require_ssl,true}]) and read Error.Message. Replay: redshift.parameter-group-lifecycle.
Cause 3: CreateClusterParameterGroup; ModifyClusterParameterGroup(Parameters=[{require_ssl,'true'}]); then ModifyClusterParameterGroup(Parameters=[{auto_mv,'false'},{require_ssl,'maybe'}]) and read Error.Code / Error.Message; DescribeClusterParameters(Source='user') must still be {require_ssl: 'true'}. Replay: redshift.parameter-group-non-boolean-value-rejected.
Cause 4: CreateClusterParameterGroup; ModifyClusterParameterGroup(Parameters=[{max_concurrency_scaling_clusters,'11'}]) and read Error.Code / Error.Message. Replay: redshift.parameter-group-out-of-range-concurrency-scaling-rejected.
Cause 5: CreateCluster(ClusterIdentifier=X, ...); wait available; CreateCluster again with the same ClusterIdentifier and read Error.Message. Then CreateClusterSnapshot(SnapshotIdentifier=S, ClusterIdentifier=X); wait available; CreateClusterSnapshot again with the same SnapshotIdentifier and read Error.Message. Replays: redshift.cluster-already-exists-fault, redshift.snapshot-duplicate-and-ungranted-revoke-fail.
Cause 6: sts GetCallerIdentity for the account; CreateClusterParameterGroup and CreateTags on its ARN (positive control); then CreateTags(ResourceName='arn:aws:redshift:<region>:<account>:cluster:<never-created-name>', Tags=[{k,v}]) and read Error.Code / Error.Message. Replay: redshift.tags-unknown-resource-fails.
