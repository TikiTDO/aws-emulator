Issue-Source: trace-sweep-divergence
Issue-Trace: s3control-access-point-create-rejections
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3control-access-point-create-rejections s3c-access-point-name-validation-splits-into-two-identities s3c-access-point-public-access-block-is-replaced-not-merged s3control-access-grants-grant-lifecycle-and-location-referential-integrity
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|CreateAccessPoint|4xx->2xx|$.ErrorResponse
Issue-Key: s3control|CreateAccessPoint|4xx->2xx|status
Issue-Key: s3control|CreateAccessPoint|4xx->4xx|$.ErrorResponse.Error.URI
Issue-Key: s3control|CreateAccessPoint|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|CreateAccessPoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetAccessGrant|2xx->2xx|$.GetAccessGrantResult.UsageLimit
Issue-Commit: c4d9e9a843977049f6a1e41e862e9a0efee7c1e6

# s3control CreateAccessPoint refusals diverge (absent bucket accepted, wrong code, wrong URI member) and GetAccessGrant omits UsageLimit

> _Note: this issue was identified against the code base at commit `c4d9e9a843977049f6a1e41e862e9a0efee7c1e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint

## Description
4 net-new root causes on the s3control (AWSS3ControlServiceV20180820, REST-XML) surface: the CreateAccessPoint request path neither enforces nor frames AWS's own service-level refusals, and GetAccessGrant's success body is short one member. Status and envelope shape are otherwise correct in these clusters. (Open issue 20261029T192817Z-s3control-xml-error-envelopes-drop-requestid-hostid-and.md covers the missing RequestId/HostId/resource-member family on OTHER operations and does not carry any of these four replay keys; 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md covers other access-point gaps and likewise does not carry them.)

--- Cause 1 of 4: CreateAccessPoint succeeds for a bucket that does not exist ---
AWS: Smithy CreateAccessPoint declares errors: [] and PUT /v20180820/accesspoint/{Name} with Bucket @required (BucketName, @length 3..255), so the referential rule is recorded behavior only: when the named Bucket does not exist in the account, AWS refuses with HTTP 400 and ErrorResponse/Error/Code 'InvalidRequest', message 'Amazon S3 AccessPoint can only be created for existing bucket' (the whole message is fixed text, no interpolated values), and creates nothing.
Emulator: answers HTTP 200 with a CreateAccessPointResult and no ErrorResponse at all, i.e. it registers an access point over a non-existent bucket.
Why: a client that provisions an access point over a mistyped or not-yet-created bucket gets a success it would never get on AWS, then every data-path read through that alias fails later with an unrelated error; infrastructure tools that treat 200 as 'bucket exists' silently drift from reality.
Fix: CreateAccessPoint's handler/validation in the s3control service crate — resolve Bucket (and BucketAccountId when supplied) against bucket state BEFORE inserting the access point and refuse with InvalidRequest when it is absent.
Traces: s3control-access-point-create-rejections#task#5.

--- Cause 2 of 4: CreateAccessPoint with Bucket omitted answers the generic validation framing instead of AWS's InvalidRequest ---
AWS: Smithy marks CreateAccessPointRequest.Bucket @required, and the recorded refusal shows s3control does NOT use the generic 'N validation error detected' framing for it: HTTP 400 with Code 'InvalidRequest' and message 'Must specify bucket to create Amazon S3 AccessPoint' — entirely fixed text, with no member name, value or limit interpolated.
Emulator: HTTP 400 but Code 'ValidationException' and message "1 validation error detected: Value null at 'bucket' failed to satisfy constraint: Member must not be null".
Why: clients and SDK error handling branch on the error code; 'ValidationException' is not a code this REST-XML service emits, so retry/branch logic keyed on InvalidRequest never fires and the refusal is unrecognised.
Fix: CreateAccessPoint's input validation — raise the operation's own InvalidRequest refusal for a missing Bucket rather than falling through to the shared required-member validator.
Traces: s3c-access-point-public-access-block-is-replaced-not-merged#task#11.

--- Cause 3 of 4: the error envelope's URI member carries the full HTTP path instead of the resource-relative URI ---
AWS: recorded behavior only (errors: [] in the model). When CreateAccessPoint is refused for a malformed access-point Name, the ErrorResponse/Error carries a <URI> member naming the resource the request addressed, relative to the service root and WITHOUT the /v20180820 API-version prefix: 'accesspoint/<Name>', where <Name> is the name taken verbatim from the request's {Name} httpLabel. The fixed part is the 'accesspoint/' prefix; the rest is a request value.
Emulator: emits the raw request target instead — '/v20180820/accesspoint/<Name>' (leading slash and version segment included). Status and Code/Message match.
Why: callers that parse <URI> to recover the offending resource identifier get an API-version-qualified path, so resource-name extraction and any comparison against an ARN resource part fail.
Fix: the s3control error serializer where the URI member is populated — derive it from the resolved resource path, stripping the '/v20180820' version prefix and the leading slash, not from the raw request line.
Traces: s3c-access-point-name-validation-splits-into-two-identities#task#5.

--- Cause 4 of 4: GetAccessGrant omits the UsageLimit member from its 200 body ---
AWS: GetAccessGrantResult in the on-disk Smithy model does not declare UsageLimit, so this is recorded behavior only: the recorded 200 response for a grant created without an explicit limit carries a <UsageLimit> element whose value is 0. It is a constant for an unlimited grant, not a per-run value.
Emulator: HTTP 200 with the modeled members correct but no UsageLimit element at all.
Why: a client that reads grant.UsageLimit to decide whether a grant is capped sees an absent/None field where AWS always supplies a number, so 'no limit' and 'field unsupported' become indistinguishable.
Fix: the GetAccessGrant response serializer (and the access-grant record in s3control state, which must carry the field) — emit UsageLimit for every grant, defaulting to 0 when none was requested.
Traces: s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#7.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up=true, bootstrap_signal=false, 327 steps, 174 passed, score 53.2%). The four clusters below carry no owning issue and no stub path:

1. CreateAccessPoint — s3control-access-point-create-rejections#task#5: 'status: expected 400, got 200' and '$.ErrorResponse: missing (expected {Error: {Code: InvalidRequest, Message: "Amazon S3 AccessPoint can only be created for existing bucket"}, RequestId: ..., HostId: ...}, got None)'. The recorded RequestId/HostId literals are per-run values and are not requirements; the 400, the code and the message are.
2. CreateAccessPoint — s3c-access-point-public-access-block-is-replaced-not-merged#task#11: '$.ErrorResponse.Error.Code: value_mismatch (expected \'InvalidRequest\', got \'ValidationException\')' and '$.ErrorResponse.Error.Message: value_mismatch (expected \'Must specify bucket to create Amazon S3 AccessPoint\', got "1 validation error detected: Value null at \'bucket\' failed to satisfy constraint: Member must not be null")'. Status 400 matched.
3. CreateAccessPoint — s3c-access-point-name-validation-splits-into-two-identities#task#5: '$.ErrorResponse.Error.URI: value_mismatch (expected \'accesspoint/<Name>\', got \'/v20180820/accesspoint/<Name>\')', where <Name> is the malformed name sent in the request. Status 400 and Code/Message matched.
4. GetAccessGrant — s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#7: '$.GetAccessGrantResult.UsageLimit: missing (expected \'0\', got None)'. Status 200 matched.

Model used: services/service-com-amazonaws-s3control-awss3controlservicev20180820/smithy-model/com-amazonaws-s3control-awss3controlservicev20180820.json — CreateAccessPoint (PUT /v20180820/accesspoint/{Name}, AccountId @required hostLabel+x-amz-account-id, Name @required httpLabel, Bucket @required, errors: []); GetAccessGrant (GET /v20180820/accessgrantsinstance/grant/{AccessGrantId}, output has no UsageLimit member, errors: []).

## Steps to Reproduce
1. git clone the repo, boot the emulator from it, and replay wire corpus simparity-wire-aws-v1.
2. Cause 1 — replay s3control-access-point-create-rejections#task#5: PUT /v20180820/accesspoint/{Name} with x-amz-account-id set and a CreateAccessPointRequest body whose <Bucket> names a bucket that was never created. AWS answers 400 InvalidRequest 'Amazon S3 AccessPoint can only be created for existing bucket'; the emulator answers 200 and creates the access point.
3. Cause 2 — replay s3c-access-point-public-access-block-is-replaced-not-merged#task#11: the same PUT with the <Bucket> member omitted entirely. Expect 400 InvalidRequest 'Must specify bucket to create Amazon S3 AccessPoint'; observe 400 ValidationException with the '1 validation error detected: ... at \'bucket\' ...' framing.
4. Cause 3 — replay s3c-access-point-name-validation-splits-into-two-identities#task#5: the same PUT with a {Name} label that violates the access-point name rules (e.g. containing uppercase). Expect the refusal's ErrorResponse/Error/URI to be 'accesspoint/<Name>'; observe '/v20180820/accesspoint/<Name>'.
5. Cause 4 — replay s3control-access-grants-grant-lifecycle-and-location-referential-integrity#task#7: create an access-grants instance, a location and a grant with no usage limit, then GET /v20180820/accessgrantsinstance/grant/{AccessGrantId}. Expect <UsageLimit>0</UsageLimit> in GetAccessGrantResult; observe the element absent.
