Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-cluster-parameter-group-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-cluster-parameter-group-lifecycle rds-cluster-parameter-group-mutation-envelopes rds-db-parameter-group-lifecycle-and-engine-defaults rds-parameter-numeric-precision-round-trip rds-db-parameter-group-lifecycle rds-parameter-group-enforcement
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.ApplyType
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.DataType
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.IsModifiable
Issue-Key: rds|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter.AllowedValues
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter.ApplyType
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter.DataType
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter.Description
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter.IsModifiable
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult
Issue-Key: rds|DescribeDBParameters|2xx->2xx|$.DescribeDBParametersResponse.DescribeDBParametersResult.Parameters.Parameter
Issue-Commit: 9919ffd222ff0dfbfa1095fff748926e9311733c

# RDS parameter listings omit engine-default metadata and leak unknown parameters

> _Note: this issue was identified against the code base at commit `9919ffd222ff0dfbfa1095fff748926e9311733c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusterParameters

## Description
DescribeDBParameters and DescribeDBClusterParameters serve a parameter list the emulator builds from user overrides only: the engine-default rows are absent, each returned Parameter is missing the descriptive/typing members AWS always sets, the Source filter is ignored, and parameters the emulator should have refused are echoed back. 6 causes.

Cause 1 of 6: DescribeDBClusterParameters on a freshly created cluster parameter group.
AWS: recorded 200 with Parameters.Parameter listing the family's engine defaults (e.g. ApplyType 'dynamic', DataType 'boolean', ApplyMethod 'pending-reboot', Source 'engine-default', ParameterName, IsModifiable 'true').
Emulator: 200 with Parameters serialized as the empty value — no rows at all.
Why: a client that reads a group's parameters before modifying it sees nothing and cannot discover modifiable parameters or their types.
Fix: the DescribeDBClusterParameters handler plus an engine-default parameter catalog in RDS state.
Traces: rds-db-cluster-parameter-group-lifecycle#task#3. (Open issue 20261029T210641Z-rds-2-net-new-root-causes-a-freshly.md describes a related empty-parameter symptom; this cluster carried no owner, so it is filed here.)

Cause 2 of 6: DescribeDBClusterParameters after a parameter was modified.
AWS: each returned Parameter carries ApplyType, DataType and IsModifiable alongside ParameterName/ParameterValue/Source/ApplyMethod.
Emulator: ApplyType, DataType and IsModifiable are absent.
Why: a client cannot tell whether a parameter is dynamic or static, or whether it may be modified.
Fix: the Parameter serializer — populate the typing members from the engine-default catalog.
Traces: rds-db-cluster-parameter-group-lifecycle#task#7; also #task#9.

Cause 3 of 6: DescribeDBClusterParameters after a modify that named an unknown parameter.
AWS: 200 with exactly one Parameter (the real one), carrying AllowedValues, ApplyType, DataType, Description, ApplyMethod, Source, ParameterName, IsModifiable, ParameterValue and SupportedEngineModes.
Emulator: 200 with a two-element list — the real parameter (stripped of all metadata) plus the unknown name the modify should have refused.
Why: the listing contains a parameter that does not exist in the engine family.
Fix: the DescribeDBClusterParameters handler and the ModifyDBClusterParameterGroup validation that let the row in.
Traces: rds-cluster-parameter-group-mutation-envelopes#task#8.

Cause 4 of 6: DescribeDBParameters on an instance parameter group.
AWS: each Parameter carries AllowedValues (the engine's full enumeration), ApplyType, DataType, Description and IsModifiable.
Emulator: all five members absent.
Why: same — the client has no type, range or description for any parameter.
Fix: the Parameter serializer / engine-default parameter catalog.
Traces: rds-db-parameter-group-lifecycle-and-engine-defaults#task#7; also rds-parameter-numeric-precision-round-trip#task#3.

Cause 5 of 6: DescribeDBParameters with a Source filter that selects nothing.
AWS: Smithy DescribeDBParameters input has a Source member; the recording answers 200 with Parameters serialized as the empty value.
Emulator: 200 but returns the user-modified parameter anyway — the Source selector is ignored.
Why: a client filtering by source gets rows AWS excludes.
Fix: the DescribeDBParameters handler's Source filtering.
Traces: rds-db-parameter-group-lifecycle#task#9.

Cause 6 of 6: DescribeDBParameters after a modify that set a non-modifiable parameter.
AWS: 200 with one Parameter, carrying AllowedValues, ApplyType 'static', DataType, Description and IsModifiable.
Emulator: 200 with two rows — the legitimate parameter (no metadata) plus the protected parameter the modify should have refused.
Why: the listing exposes a parameter RDS never lets a client set.
Fix: the DescribeDBParameters handler and the ModifyDBParameterGroup IsModifiable enforcement that let the row in.
Traces: rds-parameter-group-enforcement#task#7.

## Evidence
Replay cluster details (status 200 matched throughout; body diverged):
1. rds-db-cluster-parameter-group-lifecycle#task#3 DescribeDBClusterParameters: $...Parameters type_mismatch — expected {'Parameter': [{'ApplyType': 'dynamic', 'DataType': 'boolean', 'ApplyMethod': 'pending-reboot', 'Source': 'engine-default', 'ParameterName': 'activate_all_roles_on_login', 'IsModifiable': 'true'}]}, got ''.
2. #task#7 DescribeDBClusterParameters: Parameter.ApplyType missing (expected 'dynamic'), Parameter.DataType missing (expected 'integer'), Parameter.IsModifiable missing (expected 'true').
3. rds-cluster-parameter-group-mutation-envelopes#task#8 DescribeDBClusterParameters: Parameters.Parameter type_mismatch — expected a single record with AllowedValues '-1-2147483647', ApplyType 'dynamic', DataType 'integer', Description, SupportedEngineModes; got a two-element list [log_min_duration_statement, rdsenv_no_such_parameter] with only ParameterName/ParameterValue/Source/ApplyMethod.
4. rds-db-parameter-group-lifecycle-and-engine-defaults#task#7 DescribeDBParameters: Parameter.AllowedValues, .ApplyType, .DataType, .Description, .IsModifiable all missing.
5. rds-db-parameter-group-lifecycle#task#9 DescribeDBParameters: DescribeDBParametersResult expected {'Parameters': ''}, got a Parameters list containing max_connections.
6. rds-parameter-group-enforcement#task#7 DescribeDBParameters: Parameters.Parameter type_mismatch — expected one record (back_log) with AllowedValues '1-65535', ApplyType 'static', DataType 'integer', Description; got [back_log, basedir] with no metadata.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run rds-db-cluster-parameter-group-lifecycle#task#3 and every step in trace_ids; each creates or modifies a parameter group and then lists its parameters.
