Issue-Source: trace-sweep-divergence
Issue-Trace: lakeformation-gtr-lf-path-role-principal-forms
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lakeformation-gtr-lf-path-role-principal-forms lakeformation-gtr-lf-role-grant-fix-on-governed-table lakeformation-data-lake-settings-and-principal
Issue-Shape-Id: com.amazonaws.lakeformation#AWSLakeFormation
Issue-Key: lakeformation|GrantPermissions|4xx->2xx|$
Issue-Key: lakeformation|GrantPermissions|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lakeformation|GrantPermissions|4xx->2xx|status
Issue-Key: lakeformation|PutDataLakeSettings|4xx->4xx|$
Issue-Commit: dedd6f8a44979b422a25c34412c44daa0d34e6e0

# Lake Formation GrantPermissions principal path unvalidated and InvalidInputException body carries __type

> _Note: this issue was identified against the code base at commit `dedd6f8a44979b422a25c34412c44daa0d34e6e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GrantPermissions, PutDataLakeSettings

## Description
Two net-new causes on the Lake Formation client-refusal path of the lakeformation crate: a GrantPermissions call whose principal ARN AWS refuses is accepted and answered 200, and the restJson1 error body the crate serializes for InvalidInputException carries an extra `__type` member AWS does not send. Both live in how this service crate validates a request and then shapes its 400 payload. 2 causes.

Cause 1 of 2 - GrantPermissions accepts a principal ARN AWS refuses, returning 200 instead of a 400 InvalidInputException.
AWS: Smithy: GrantPermissions(Principal: DataLakePrincipal required, Resource: Resource required, Permissions: PermissionList required) models InvalidInputException (errorKind client, httpError 400) alongside ConcurrentModificationException and EntityNotFoundException; restJson1, POST /GrantPermissions, success code 200 with an empty body. The specific refusal text is recorded behavior only (the model states no message template): the recorded response is HTTP 400 with an x-amzn-errortype header and body {"Message": "Invalid path for user, arn: <principalArn>"}, where the fixed text is "Invalid path for user, arn: " and the ONLY variable part is the Principal.DataLakePrincipalIdentifier ARN taken verbatim from the request. Hypothesis: Lake Formation resolves the grantee ARN against IAM and refuses an ARN whose IAM path portion does not name an existing user/role path, which is what this fixture's role-principal ARN form exercises.
Emulator: HTTP 200 with an empty success body - no 400, no $.Message, and no x-amzn-errortype header. The grant is silently recorded for an ARN AWS never accepts.
Why: a client that mis-forms a grantee ARN's path is told the grant succeeded, so it believes the principal now holds the permissions when on AWS it holds nothing; the error that would have surfaced the malformed ARN never reaches its typed InvalidInputException handler, and any later audit/revoke built on the emulator's state disagrees with AWS.
Fix: the GrantPermissions handler's Principal validation in the lakeformation service crate - validate the DataLakePrincipalIdentifier ARN (including its path portion) before writing the grant, and raise InvalidInputException with AWS's "Invalid path for user, arn: <principalArn>" template plus the x-amzn-errortype header. Related but NOT covering this: open issue 20261103T022922Z-lakeformation-lake-formation-permission-list-revoke-paths-batch-grants.md carries the ListPermissions bare-name principal cause and its replay keys are for ListPermissions/RevokePermissions/BatchGrantPermissions, not GrantPermissions.
Traces: lakeformation-gtr-lf-path-role-principal-forms#task#7 (cluster also covers lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#7).

Cause 2 of 2 - the InvalidInputException body carries an extra `__type` member on top of Message.
AWS: Smithy: PutDataLakeSettings(DataLakeSettings required) models InvalidInputException (errorKind client, httpError 400) whose only modeled member is Message; the service protocol is restJson1 (AWSLakeFormation 2017-03-31), which conveys the error SHAPE in the x-amzn-errortype header, not in the body. Recorded response: HTTP 400 with the body consisting of exactly {"Message": "Principal cannot be both readOnly and LF admin."} - one key; the sentence is fixed text with no variable part (the offending principal is not echoed).
Emulator: HTTP 400 with the right Message but an ADDITIONAL top-level key, so the body is {"__type": "InvalidInputException", "Message": "..."}. `__type` is the awsJson error discriminator and is not part of a restJson1 error body for this service.
Why: a client that compares, logs or schema-validates the error body (strict JSON decoders, recorded-response fixtures, body diffing in tests) sees an unmodeled member that AWS never sends; tooling that keys off `__type` also reads a discriminator that would be absent against AWS, so error classification is derived from the wrong place.
Fix: the lakeformation crate's restJson1 error serializer (the shared error-response construction used by these handlers) - emit only the error shape's modeled members in the body and carry the shape name in x-amzn-errortype. Related but NOT covering this: open issue 20261029T173933Z-lakeformation-lake-formation-client-error-bodies-reword-or-drop.md is about the TEXT of refusal messages (including PutDataLakeSettings#task#7) and carries no key for this body-shape cause at task#5.
Traces: lakeformation-data-lake-settings-and-principal#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, /__admin/health 200, score 71.5%, 10 divergence clusters). Both clusters below are verdict=mismatch, with empty stub_path and empty owned_by.

1. lakeformation-gtr-lf-path-role-principal-forms#task#7 GrantPermissions, expected 400, got 200: "$: empty_body (expected {'Message': 'Invalid path for user, arn: arn:aws:iam::000000000000:role/sim2real-probe-gtr-pr-role-ed44c858'}, got {})" and "$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)". The same cluster also covers lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#7. (The ARN above is one run's generated role name; the requirement is the fixed text "Invalid path for user, arn: " followed by the request's principal ARN.)

2. lakeformation-data-lake-settings-and-principal#task#5 PutDataLakeSettings 400->400: "$: value_mismatch (expected {'Message': 'Principal cannot be both readOnly and LF admin.'}, got {'__type': 'InvalidInputException', 'Message': 'Principal cannot be both readOnly and LF admin.'})" - status and Message correct, one extra top-level `__type` key in the body.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or drive the two fixtures directly:
   - lakeformation-gtr-lf-path-role-principal-forms: run the fixture up to its GrantPermissions call whose Principal.DataLakePrincipalIdentifier is the role-ARN form AWS refuses (step task#7). Expect HTTP 400 with x-amzn-errortype set and {"Message": "Invalid path for user, arn: <the request's principal ARN>"}; observe HTTP 200 with an empty body. The same call in lakeformation-gtr-lf-role-grant-fix-on-governed-table#task#7 reproduces it too.
   - lakeformation-data-lake-settings-and-principal: call PutDataLakeSettings with a DataLakeSettings whose principal is marked both readOnly and a data lake admin (step task#5). Expect HTTP 400 whose body has exactly one key, Message = "Principal cannot be both readOnly and LF admin."; observe the same status and Message plus an extra "__type": "InvalidInputException" key.
3. Diff each response's status, x-amzn-errortype header and full JSON body (key set included) against the recorded AWS response.
