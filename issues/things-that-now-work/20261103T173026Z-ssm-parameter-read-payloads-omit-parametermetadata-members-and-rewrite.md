Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-parameter-store-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-parameter-store-lifecycle ssm-parameter-unlabel-partial-semantics ssm-alarm-firing-latency-mode-parameter-fix
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetParameterHistory|2xx->2xx|$.Parameters[*]
Issue-Key: ssm|DescribeParameters|2xx->2xx|$.Parameters[Name="sim2real-probe-ssm-param-10c56f94"].LastModifiedUser
Issue-Key: ssm|DescribeParameters|2xx->2xx|$.Parameters[Name="sim2real-probe-ssm-param-10c56f94"].Policies
Issue-Key: ssm|GetParameterHistory|2xx->2xx|$.Parameters[*].Labels
Issue-Key: ssm|GetParameterHistory|2xx->2xx|$.Parameters[*].LastModifiedUser
Issue-Key: ssm|GetParameterHistory|2xx->2xx|$.Parameters[*].Name
Issue-Key: ssm|GetParameterHistory|2xx->2xx|$.Parameters[*].Policies
Issue-Key: ssm|DescribeParameters|2xx->2xx|$.Parameters[*]
Issue-Key: ssm|GetParameter|2xx->2xx|$.Parameter.Selector
Issue-Commit: 133bbb645e9618d24e6206d20cee2b832cb9c7e9

# Parameter read payloads omit ParameterMetadata members and rewrite parameter names

> _Note: this issue was identified against the code base at commit `133bbb645e9618d24e6206d20cee2b832cb9c7e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameterHistory

## Description
Successful (200) Parameter Store reads return structurally incomplete records: three modelled members are never serialized, the parameter Name is rewritten with a leading '/', and GetParameter's Selector drops its separator. 5 CAUSES. All are payload-shape defects, not per-run value differences (the replay's own timestamp and ARN differences are ignored here).

Cause 1 of 5 - GetParameterHistory omits Labels, LastModifiedUser and Policies. AWS: every ParameterHistory entry carries LastModifiedUser (Smithy: ParameterHistory.LastModifiedUser, 'Amazon Resource Name (ARN) of the Amazon Web Services user who last changed the parameter'), Labels (ParameterHistory.Labels) and Policies (ParameterHistory.Policies); for a parameter written with no labels and no policies AWS returns them as empty lists, and LastModifiedUser as the ARN of the identity that performed the write. Emulator: the three members are absent from every history entry (it emits only Name, Type, LastModifiedDate, Value, Version, Tier, DataType). Why: a client auditing who last changed a parameter, or enumerating a version's labels/policies before an unlabel or policy edit, reads null where AWS always returns a value, and treats an unlabelled version as indistinguishable from one it could not read. Fix: GetParameterHistory serializer plus the parameter-version record in the SSM state (it must retain the writer identity, label set and policy list). Traces: ssm-parameter-store-lifecycle#task#6.

Cause 2 of 5 - DescribeParameters omits LastModifiedUser and Policies. AWS: ParameterMetadata.LastModifiedUser and ParameterMetadata.Policies are returned for every parameter (Smithy: ParameterMetadata), with Policies an empty list when none are attached. Emulator: both absent. Why: as cause 1, for the list/describe path. Related open issues 20261024T065715Z-ssm-sts-ssm-describeparameters-is-an-unimplemented-scaffold-stub-so.md and 20261025T053713Z-ssm-addtagstoresource-is-an-unimplemented-501-stub-and.md mention this member, but the replay reports no owning issue for this cluster, so it is filed here. Fix: DescribeParameters serializer + state. Traces: ssm-parameter-store-lifecycle#task#9.

Cause 3 of 5 - GetParameterHistory rewrites the parameter Name with a leading '/'. AWS: Parameter/ParameterHistory.Name is the name as created - a parameter created without a leading slash is returned without one (Smithy: ParameterHistory.Name target PSParameterName; the emulator's own ARN for the same record, arn:aws:ssm:<region>:<account>:parameter/<name>, already shows the unprefixed name). Emulator: Name comes back as '/<name>', i.e. the storage key with a synthetic slash, while the ARN keeps the original. Why: a client that round-trips the returned Name into GetParameter/PutParameter/DeleteParameter addresses a different name than it created, and name-equality checks against its own input fail. Fix: the name-normalization step shared by the parameter store's read path - normalize for lookup only, serialize the stored name. Traces: ssm-parameter-unlabel-partial-semantics#task#6 (also omits the members of cause 1).

Cause 4 of 5 - DescribeParameters rewrites the parameter Name the same way. AWS: ParameterMetadata.Name is the created name. Emulator: '/<name>'. Fix: as cause 3, in the DescribeParameters read path. Traces: ssm-parameter-unlabel-partial-semantics#task#7.

Cause 5 of 5 - GetParameter returns Selector without its ':' separator. AWS: Parameter.Selector echoes the selector exactly as used to retrieve the value, in one of the documented 'parameter_name:version' / 'parameter_name:label' forms, so a request for <name>:1 returns Selector ':1' (Smithy: Parameter.Selector, 'Either the version number or the label used to retrieve the parameter value. Specify selectors by using one of the following formats: parameter_name:version ...'). Emulator: Selector '1' - the separator is stripped. Why: a client that rebuilds the parameter reference as Name + Selector produces '<name>1' instead of '<name>:1' and then fails to resolve it. Fix: GetParameter's selector echo in the response serializer. Traces: ssm-alarm-firing-latency-mode-parameter-fix#task#11, ssm-parameter-store-lifecycle#task#8.

## Evidence
Replay of simparity-wire-aws-v1. Representative (ssm-parameter-store-lifecycle#task#6, GetParameterHistory): expected 200 / actual 200; each entry expected {..., 'Labels': [], 'LastModifiedUser': 'arn:aws:iam::<account>:user/<identity>', 'Policies': [], ...} but the emulator returned only {'Name','Type','LastModifiedDate','Value','Version','Tier','DataType'}. ssm-parameter-store-lifecycle#task#9 (DescribeParameters): $.Parameters[...].LastModifiedUser missing, $.Parameters[...].Policies missing (expected []). ssm-parameter-unlabel-partial-semantics#task#6 (GetParameterHistory): $.Parameters[0].Name expected 'sim2real-ssm-unlabel-d8587bc2', got '/sim2real-ssm-unlabel-d8587bc2', plus the same three missing members; #task#7 (DescribeParameters) shows the same Name rewrite while its ARN keeps the unprefixed name. ssm-alarm-firing-latency-mode-parameter-fix#task#11 (GetParameter): $.Parameter.Selector expected ':1', got '1'. Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (ParameterHistory, ParameterMetadata, Parameter.Selector).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1. 2. Representative: fixture ssm-parameter-store-lifecycle step task#6 - PutParameter twice then GetParameterHistory; check each entry for Labels, LastModifiedUser and Policies. 3. Step task#9 of the same fixture - DescribeParameters; check LastModifiedUser and Policies. 4. Fixture ssm-parameter-unlabel-partial-semantics steps task#6 and task#7 - compare the returned Name with the name used in PutParameter. 5. Fixture ssm-alarm-firing-latency-mode-parameter-fix step task#11 - GetParameter with Name='<name>:1' and compare $.Parameter.Selector.
