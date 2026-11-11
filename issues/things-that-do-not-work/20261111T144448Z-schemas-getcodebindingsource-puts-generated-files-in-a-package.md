Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: schemas.code-binding-lifecycle-generates-and-downloads
Issue-Shape-Id: com.amazonaws.schemas#schemas
Issue-Key: schemas.code-binding-lifecycle-generates-and-downloads|run#22|AssertionError|assert any(sanitized_schema_name in name for name in names), (
Issue-Commit: 6cf63a707f0639293fd85010db3109426d2e0136

# schemas GetCodeBindingSource puts generated files in a package built by splitting the schema name on hyphens instead of sanitizing them

> _Note: this issue was identified against the code base at commit `6cf63a707f0639293fd85010db3109426d2e0136`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCodeBindingSource

## Description
One net-new root cause in the schemas code-binding generator's package naming.

Cause 1 of 1: the package path of every generated source file is derived by splitting the schema name on EVERY non-alphanumeric character, so a schema name containing `-` (or `_`) is scattered across several package/directory segments instead of becoming one sanitized segment.

AWS: recorded behavior only — the Smithy model (services/service-com-amazonaws-schemas-schemas/smithy-model/com-amazonaws-schemas-schemas.json) models GetCodeBindingSource as GET /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/language/{Language}/source, code 200, output member Body (blob, @httpPayload), and says nothing about the archive's entry names. What the recording pins is that the ZIP archive AWS serves for a Java8 binding contains at least one entry whose path carries the SCHEMA NAME as a single package segment, with the characters a Java identifier cannot hold replaced in place (`-` -> `_`): for a schema named `<name>` there is an entry matching `src/main/java/schema/<name with each '-' replaced by '_'>/<Model>.java`. Only the characters AWS itself uses as schema-namespace separators in discovered schema names (`.` and `@`, as in `aws.s3@ObjectCreated` -> `schema/aws/s3/objectcreated/`) introduce a new package segment; `-` and `_` do not. The fixed part of the path is the `src/main/java/schema/` root and the `.java` extension; the variable parts are the sanitized schema name and the model class name taken from the stored document's top-level component (`Event` for the document this check stores).

Emulator: the call succeeds with 200 and a valid ZIP whose member passes `testzip()`, but the single entry is `src/main/java/schema/sim2real/schemas/sch/ade45d7f9ba6/Event.java` — each hyphen of the schema name has become a `/` (a new package segment) instead of an `_` inside one segment, so no entry contains the sanitized schema name at all. The check reports: `AssertionError: expected a package path derived from schema name 'sim2real-schemas-sch-ade45d7f9ba6' (sanitized 'sim2real_schemas_sch_ade45d7f9ba6'), got ['src/main/java/schema/sim2real/schemas/sch/ade45d7f9ba6/Event.java']`. (The individual path components here are per-run values from the unique schema name; the requirement is the mapping rule, not these literals.)

Why: a client that unpacks the binding and compiles against it imports `schema.<sanitized schema name>.<Model>`, which is the package AWS's generator declares. Against the emulator that package does not exist — the class sits in a different, longer package (`schema.sim2real.schemas.sch.ade45d7f9ba6`), and worse, two different schema names that differ only in where their separators fall collapse onto the same package, so bindings for distinct schemas can overwrite each other in one source tree. Generated code that builds against real AWS fails to compile against the emulator.

Fix: the package-naming helper of this operation's handler — `package_segments` in services/service-com-amazonaws-schemas-schemas/src/ops/get_code_binding_source.rs, which feeds both the archive entry path (`render`) and the emitted `package`/Go package declaration (`java_source`, `go_source`). It must sanitize characters that are illegal in an identifier into `_` within a single segment and only treat AWS's namespace separators as segment boundaries, rather than splitting on every non-alphanumeric character.

Note: the open issue 20261110T135450Z-schemas-response-payloads-diverge-from-aws-getcodebindingsource-serves.md also concerns GetCodeBindingSource, but its cause and replay key are the archive's SIZE/file count ($.__wire_length__ 3060 vs 400, driven by `declared_structures` enumerating only top-level named schemas). It does not carry this behavioral check and says nothing about entry paths, so emitting every implied type would still emit all of them under the wrong package; this cause would be closed unrun.

Traces: behavioral check schemas.code-binding-lifecycle-generates-and-downloads.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout (25/30 passed, bootstrap_signal=false).

[mismatch] schemas.code-binding-lifecycle-generates-and-downloads
  File "check_schemas_code_binding_lifecycle_generates_and_downloads.py", line 67, in run
    assert any(sanitized_schema_name in name for name in names), (
  AssertionError: expected a package path derived from schema name 'sim2real-schemas-sch-ade45d7f9ba6' (sanitized 'sim2real_schemas_sch_ade45d7f9ba6'), got ['src/main/java/schema/sim2real/schemas/sch/ade45d7f9ba6/Event.java']

Everything before this assertion matched AWS: DescribeCodeBinding before PutCodeBinding returned NotFoundException with the expected message, PutCodeBinding reached CREATE_COMPLETE, and GetCodeBindingSource returned 200 with a body starting 'PK' that unzips cleanly and contains a `.../Event.java` entry. The only divergence is the package path.

Smithy: GetCodeBindingSource http {method GET, uri /v1/registries/name/{RegistryName}/schemas/name/{SchemaName}/language/{Language}/source, code 200}; output member Body target Body, httpPayload, blob; errors BadRequestException 400, UnauthorizedException 401, ForbiddenException 403, NotFoundException 404, TooManyRequestsException 429, InternalServerErrorException 500. The model does not constrain archive entry names, hence 'recorded behavior only'.

Emulator source: services/service-com-amazonaws-schemas-schemas/src/ops/get_code_binding_source.rs — `package_segments` (lines ~650-659) builds the package as PACKAGE_ROOT plus `schema_name.split(|c: char| !c.is_alphanumeric())` lower-cased, i.e. one segment per run of alphanumerics; `render` (lines ~669-684) joins those with '/' into the entry path and `java_source` (line ~727) joins them with '.' into the emitted `package` declaration.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. CreateRegistry with any name, then CreateSchema with Type=OpenApi3 and a schema name that contains a hyphen (e.g. `sim2real-schemas-sch-<suffix>`) whose content declares one top-level component named `Event` under components/schemas.
3. PutCodeBinding(Language='Java8', RegistryName=<registry>, SchemaName=<schema>) and poll DescribeCodeBinding until Status is CREATE_COMPLETE.
4. GetCodeBindingSource(Language='Java8', RegistryName=<registry>, SchemaName=<schema>) and list the ZIP entries.
   Observed: ['src/main/java/schema/sim2real/schemas/sch/<suffix>/Event.java'] — each '-' became a directory separator.
   Expected (AWS): an entry whose path contains the schema name with every '-' replaced by '_' as ONE package segment, e.g. 'src/main/java/schema/sim2real_schemas_sch_<suffix>/Event.java'; the emitted file's `package` declaration matches that package.
5. Re-replay schemas.code-binding-lifecycle-generates-and-downloads to confirm the fix.
