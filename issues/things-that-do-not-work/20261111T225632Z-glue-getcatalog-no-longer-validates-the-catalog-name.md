Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.catalog-invalid-name-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.catalog-invalid-name-rejected|run#11|AssertionError|assert get_code_invalid == "ValidationException", (
Issue-Commit: 7be3c4c4f98e3bb4fe12b293a9b02ae8c9c46178

# Glue GetCatalog no longer validates the catalog-name grammar: a CatalogId containing '.' is answered EntityNotFoundException instead of ValidationException

> _Note: this issue was identified against the code base at commit `7be3c4c4f98e3bb4fe12b293a9b02ae8c9c46178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCatalog

## Description
One net-new root cause in Glue's GetCatalog refusal path. It is NOT covered by the open 20261110T173259Z-glue-3-net-new-causes-getcatalog-s-malformed.md: that issue's cause 1 carries this check under the replay key `assert get_msg_invalid == (` (the emulator then answered ValidationException with the wrong member name and an unanchored pattern). In this replay the check fails EARLIER, on the preceding `assert get_code_invalid == "ValidationException"` assertion, because the emulator no longer produces a ValidationException at all for this input — the malformed-id guard is gone and the request falls through to the store lookup. Different code path, different failing assertion, so it is filed as its own cause and that open issue is named here for the fixer. It is also not a reopening of a fixed issue: no fixed issue claims GetCatalog's name-grammar refusal (the fixed 20261027T124611Z-glue-3-net-new-root-causes-createcatalog-accepts.md covers the account ROOT catalog read and the 'Catalog not found.' text, and the fixed 20261102T132146Z-glue-catalog-input-validation-returns-validationexception-instead-of.md covers GetTags/SearchTables/CreateDatabase/CreateTable/GetTable/GetTables, not GetCatalog).

Cause 1 of 1: GetCatalog accepts a CatalogId whose catalog-name part violates the catalog-name grammar and reports the result as a not-found entity, where AWS refuses the input with a generic-constraint ValidationException before any lookup.
AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models GetCatalog input {CatalogId: CatalogIdString @required @length(1,255) @pattern ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$} and errors AccessDeniedException, EntityNotFoundException, FederationSourceException, FederationSourceRetryableException, GlueEncryptionException, InvalidInputException, OperationTimeoutException (client, httpError 400) plus InternalServiceException (server, 500). The dot rule is NOT on CatalogIdString; it is on CatalogNameString (@length(1,64) @pattern ^(?!(.*[.\/\\]|aws:)).*$), the shape CreateCatalog.Name targets. Recorded behavior: AWS applies that catalog-name pattern to the name component of GetCatalog's CatalogId and refuses a value containing '.' with HTTP 400, error code ValidationException and the message '1 validation error detected: Value '<the CatalogId from the request>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!(.*[.\/\\]|aws:)).*$'. Fixed text of this error class: the '1 validation error detected: Value ... at ... failed to satisfy constraint: Member must satisfy regular expression pattern: ...' template, the member path 'name', and the anchored pattern literal ^(?!(.*[.\/\\]|aws:)).*$ copied from the model. The ONLY variable part is the rejected value, taken verbatim from the request's CatalogId. The refusal is a constraint verdict, so it is independent of store contents: it is returned whether or not a catalog of that name could exist.
Emulator: the same call is refused HTTP 400 with error code EntityNotFoundException and the message 'Catalog not found.'. The handler deliberately skips the name grammar — its comment reads 'An id is RESOLVED against the store rather than measured against the catalog NAME grammar' — so a dotted id is parsed as a catalog reference, misses the store, and is reported as a nonexistent entity.
Why: the error class is wrong for the fault. A client that catches EntityNotFoundException concludes the name is well formed and merely unused, so it will keep offering a name AWS can never accept (CreateCatalog already refuses the same name with InvalidInputException 'Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name'); a client that catches ValidationException to surface 'this identifier is malformed' never fires, and no part of AWS's message — the member path 'name' and the anchored pattern that states the rule — reaches the caller. Validation-versus-not-found is also how callers distinguish a typo from a missing resource, so probe-then-create flows take the wrong branch.
Fix: the input-validation stage of the GetCatalog handler (services/service-com-amazonaws-glue-awsglue/src/ops/get_catalog.rs, around the `crate::state::parse_catalog_reference(&input.catalog_id)` call at line 170, ahead of the AccessDenied and store-lookup branches at lines 176 and 205). Re-apply the catalog-name grammar to the NAME component of the parsed reference and raise the generic-constraint ValidationException with AWS's member path and anchored pattern, leaving the nested '<account-id>/<name>' form resolving as a reference (its separator must not be measured against the name pattern) and leaving the account-root and genuine-miss branches as they are.
Traces: glue.catalog-invalid-name-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout, service heading 'glue' (229/324 passed). glue.catalog-invalid-name-rejected is the only failing glue check reported as neither OWNED by an open issue nor blocked by an already-filed unimplemented-operation issue.

[mismatch] glue.catalog-invalid-name-rejected
AssertionError raised at check_glue_catalog_invalid_name_rejected.py line 31: `assert get_code_invalid == "ValidationException"` — 'expected a not-found-shaped error for the never-created invalid-name catalog, got 'EntityNotFoundException''.

Assertions that PASS before it (so those behaviors must keep working):
  - CreateCatalog(Name='<lowercase-name>.invalid', CatalogInput={}) -> InvalidInputException, message 'Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name'.
  - CreateCatalog(Name='<valid lowercase name>', CatalogInput={}) -> InvalidInputException, message 'Create glue native catalog is not supported.'.

The failing call:
  call:     glue.get_catalog(CatalogId='<the same dotted name>')
  expected: HTTP 400, Code=ValidationException, Message="1 validation error detected: Value '<CatalogId>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!(.*[.\/\\]|aws:)).*$"
  actual:   HTTP 400, Code=EntityNotFoundException, Message='Catalog not found.' (crate::state::messages::CATALOG_NOT_FOUND, raised at services/service-com-amazonaws-glue-awsglue/src/ops/get_catalog.rs lines 205-213)
  Message anatomy: the catalog id in the expected message is a per-run generated name copied straight from the request and is NOT part of the requirement; the requirement is the fixed template, the member path 'name', and the anchored pattern ^(?!(.*[.\/\\]|aws:)).*$ as CatalogNameString spells it.

The check's trailing control — GetCatalog(CatalogId='<valid, never-created name>') -> EntityNotFoundException 'Catalog not found.' — is the behavior the emulator now gives for BOTH ids, which is why the malformed id is indistinguishable from an unused one.

## Steps to Reproduce
Boot the emulator from this checkout and point a Glue client at it.
1. CreateCatalog(Name='sim2real-behav-catbad-<suffix>.invalid', CatalogInput={}) — refused InvalidInputException 'Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name' (correct today; must stay).
2. GetCatalog(CatalogId='sim2real-behav-catbad-<suffix>.invalid'). Expect HTTP 400 Code=ValidationException with Message "1 validation error detected: Value 'sim2real-behav-catbad-<suffix>.invalid' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!(.*[.\/\\]|aws:)).*$"; observe HTTP 400 Code=EntityNotFoundException Message='Catalog not found.'.
3. Controls that must keep working after the fix: GetCatalog(CatalogId='<a valid lowercase name no CreateCatalog ever stored>') stays EntityNotFoundException 'Catalog not found.'; GetCatalog(CatalogId=<the served account id>) still returns the account's Data Catalog; GetCatalog(CatalogId='<another account id>/<name>') still answers AccessDeniedException.
4. Re-replay glue.catalog-invalid-name-rejected to confirm the cause is closed.
