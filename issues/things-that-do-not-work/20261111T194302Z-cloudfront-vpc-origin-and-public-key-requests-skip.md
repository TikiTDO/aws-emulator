Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-get-public-key-wrong-resource-type-id-rejected
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-get-public-key-wrong-resource-type-id-rejected cloudfront-public-key-and-key-group-lifecycle cloudfront-vpc-origin-error-paths cloudfront-vpc-origin-get-invalid-id-format cloudfront-vpc-origin-invalid-argument
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetPublicKey|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetVpcOrigin|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|GetVpcOrigin|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetVpcOrigin|4xx->4xx|status
Issue-Key: cloudfront|CreateVpcOrigin|4xx->2xx|$.ErrorResponse
Issue-Key: cloudfront|CreateVpcOrigin|4xx->2xx|$headers.location
Issue-Key: cloudfront|CreateVpcOrigin|4xx->2xx|status
Issue-Commit: 14c2f807276b097453c7eebbf32725a3995b3836

# CloudFront VPC origin and public key requests skip argument validation before resource lookup

> _Note: this issue was identified against the code base at commit `14c2f807276b097453c7eebbf32725a3995b3836`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPublicKey

## Description
Three net-new wire divergence clusters (3 causes) share one fault: CloudFront does not validate the request's arguments before using them. A path identifier that is not in the resource's id format is sent straight to a state lookup and reported as a missing resource, and a VpcOriginEndpointConfig whose Arn does not validate is accepted and materialized instead of being refused. In every case AWS's argument-level refusal (its wording, and for the VPC-origin operations its 400 InvalidArgument status and code) is replaced by a lookup result or by a success. All three clusters report owned_by [] and stub_path "".

Two open issues describe the same family but carry neither of these replay keys: 20261105T231732Z-cloudfront-path-identifiers-are-not-format-validated-before.md and 20261109T120741Z-cloudfront-field-level-encryption-lookups-report-the-resource.md (field-level-encryption lookups). Neither covers GetPublicKey, GetVpcOrigin or CreateVpcOrigin, so the validation step those issues add for their own operations will not fix these three.

Cause 1 of 3: GetPublicKey with an Id that is not a public-key id reports the key missing instead of AWS's invalid-id-format message.
AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json): GetPublicKey input member Id is @required and @httpLabel; http {GET /2020-05-31/public-key/{Id}, code 200}; modeled errors AccessDenied (403) and NoSuchPublicKey (404, doc "The specified public key doesn't exist."). When the {Id} path label does not parse as a CloudFront public-key identifier — for example an identifier belonging to another resource type — AWS refuses on the identifier's FORM, and the refusal message is "Id format is invalid": fixed text in full, with no part taken from the request (the offending id is not echoed) or from state. The recorded status is 404 and the recorded Error.Code already matches what the emulator sends, so only the message is at issue here.
Emulator: answers 404 with $.ErrorResponse.Error.Message "The specified CloudFront public key does not exist." — the generic not-found wording, produced by going straight to the state lookup.
Why: the two messages mean different things to an operator and to any client that matches on message text. "Id format is invalid" says the caller passed the wrong kind of identifier (a key-group id, a distribution id, a truncated value) and must correct the request; "does not exist" says the identifier was well-formed and the key is simply absent, which sends a caller into a create-or-retry branch for a key it never actually named and hides a client-side id mix-up.
Emulator/Fix: GetPublicKey's input validation must check the {Id} label against the public-key id format before any state lookup, and refuse a malformed id with AWS's "Id format is invalid" message; the "does not exist" wording must be reserved for a well-formed public-key id that state does not hold.
Traces: cloudfront-get-public-key-wrong-resource-type-id-rejected#task#3, cloudfront-public-key-and-key-group-lifecycle#task#10.

Cause 2 of 3: GetVpcOrigin with a malformed Id answers 404 EntityNotFound instead of 400 InvalidArgument "Id format is invalid".
AWS: Smithy: GetVpcOrigin input member Id is @required and @httpLabel; http {GET /2020-05-31/vpc-origin/{Id}, code 200}; modeled errors AccessDenied (403), EntityNotFound (404), InvalidArgument (400, shape doc "An argument is invalid."), UnsupportedOperation (400). An Id that is not in the VPC-origin id format fails argument validation, so AWS refuses with HTTP 400, Error.Code 'InvalidArgument' and Message "Id format is invalid" — again fixed text in full, echoing nothing from the request or state.
Emulator: answers HTTP 404 with Error.Code 'EntityNotFound' and Message "The specified VPC origin does not exist."
Why: status, code and message all disagree, and the two classes are semantically opposite. 400 InvalidArgument is a permanent caller bug that must not be retried; 404 EntityNotFound is the documented "not created yet" signal, so a poll-until-present or create-if-absent loop treats a permanently malformed id as a transient gap and spins until its deadline, while SDK error-type dispatch (catch InvalidArgument vs EntityNotFound) takes the wrong branch.
Fix: GetVpcOrigin's input validation — validate the {Id} path label's format before resolving it, raise InvalidArgument (400) with AWS's wording for a malformed id, and reach the EntityNotFound (404) path only for a well-formed id state does not hold.
Traces: cloudfront-vpc-origin-error-paths#task#2, cloudfront-vpc-origin-get-invalid-id-format#task#1.

Cause 3 of 3: CreateVpcOrigin accepts a VpcOriginEndpointConfig whose Arn does not validate, returning 202 and a created VPC origin instead of 400 InvalidArgument.
AWS: Smithy: CreateVpcOrigin input member VpcOriginEndpointConfig is @required; VpcOriginEndpointConfig's own members Name, Arn, HTTPPort, HTTPSPort and OriginProtocolPolicy are all @required (Arn documented as "The ARN of the CloudFront VPC origin endpoint configuration"); http {POST /2020-05-31/vpc-origin, code 202}; modeled errors AccessDenied (403), EntityAlreadyExists (409), EntityLimitExceeded (400), InconsistentQuantities (400), InvalidArgument (400), InvalidTagging (400), UnsupportedOperation (400). A VpcOriginEndpointConfig.Arn that fails validation is refused before any resource is created: HTTP 400, Error.Type 'Sender', Error.Code 'InvalidArgument', Message of the form "The parameter <member path> ValidationError." — the fixed text is "The parameter " and " ValidationError.", and the variable part is the path of the offending input member, taken from the request (here the Arn member of VpcOriginEndpointConfig). No Location header is sent on a refusal.
Emulator: answers HTTP 202 with no ErrorResponse at all, materializes a VPC origin, and announces it in a Location header. (The separate defect that create Location headers name a host the emulator does not serve is already owned by 20261109T120741Z-cloudfront-create-responses-announce-a-location-uri-on.md and is not re-filed here; the cause filed here is that the request is accepted at all.)
Why: an invalid endpoint configuration is persisted, so the caller believes it has a usable VPC origin: it proceeds to attach the origin to a distribution and to put a resource policy on it, and every later step builds on a resource AWS would never have created. A negative test asserting the 400 refusal passes vacuously, and the state the emulator accumulates diverges from AWS's for the rest of the session.
Fix: CreateVpcOrigin's validation — validate VpcOriginEndpointConfig's members (starting with Arn) before any state mutation, and raise InvalidArgument (400) with AWS's "The parameter <member path> ValidationError." wording, sending no Location header and creating nothing on the refusal path.
Traces: cloudfront-vpc-origin-error-paths#task#1, cloudfront-vpc-origin-invalid-argument#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 1303 steps total, 1011 passed, 287 mismatched, 5 blocked, score 77.6%, 90 divergence clusters. All three clusters report owned_by [] and stub_path "".

Cause 1 — cloudfront-get-public-key-wrong-resource-type-id-rejected#task#3, GetPublicKey, expected status 404 / actual 404:
  $.ErrorResponse.Error.Message: value_mismatch (expected 'Id format is invalid', got 'The specified CloudFront public key does not exist.')
  divergence_key: service=cloudfront|GetPublicKey|status=404->404|keys=$.ErrorResponse.Error.Message (affected_count 2; other member cloudfront-public-key-and-key-group-lifecycle#task#10)

Cause 2 — cloudfront-vpc-origin-error-paths#task#2, GetVpcOrigin, expected status 400 / actual 404:
  status: expected 400, got 404
  $.ErrorResponse.Error.Code: value_mismatch (expected 'InvalidArgument', got 'EntityNotFound')
  $.ErrorResponse.Error.Message: value_mismatch (expected 'Id format is invalid', got 'The specified VPC origin does not exist.')
  divergence_key: service=cloudfront|GetVpcOrigin|status=400->404|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status (affected_count 2; other member cloudfront-vpc-origin-get-invalid-id-format#task#1)

Cause 3 — cloudfront-vpc-origin-error-paths#task#1, CreateVpcOrigin, expected status 400 / actual 202:
  status: expected 400, got 202
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidArgument', 'Message': 'The parameter VpcOriginEndpointConfig Arn ValidationError.'}, 'RequestId': '<per-run value, not a requirement>'}, got None)
  $headers.location: expected None, got a Location header for a newly created vpc-origin
  divergence_key: service=cloudfront|CreateVpcOrigin|status=400->202|keys=$.ErrorResponse,$headers.location,status (affected_count 2; other member cloudfront-vpc-origin-invalid-argument#task#1)

Fixed vs variable parts of the asserted messages: 'Id format is invalid' (Causes 1 and 2) is fixed text in full — the offending identifier is not echoed. 'The parameter <member path> ValidationError.' (Cause 3) is fixed text around one variable part, the offending input member's path, taken from the request. The recorded RequestId values and the recorded generated VPC-origin id/host in the Location header are per-run values and are not part of the requirement. Model grounding: smithy_query query=operation operation=GetPublicKey,GetVpcOrigin,CreateVpcOrigin and query=shape shape=VpcOriginEndpointConfig,InvalidArgument,EntityNotFound,NoSuchPublicKey on services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json — InvalidArgument (httpError 400) is modeled on both GetVpcOrigin and CreateVpcOrigin; VpcOriginEndpointConfig.Arn is @required.

## Steps to Reproduce
1. git_clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture cloudfront-get-public-key-wrong-resource-type-id-rejected, step task#3 — GET /2020-05-31/public-key/{Id} with an identifier that is not a public-key id (an id of another resource type). The emulator answers 404 with Message "The specified CloudFront public key does not exist."; the recording is 404 with Message "Id format is invalid". Fixture cloudfront-public-key-and-key-group-lifecycle#task#10 is the same cause.
4. Cause 2: replay fixture cloudfront-vpc-origin-error-paths, step task#2 — GET /2020-05-31/vpc-origin/{Id} with a malformed id. The emulator answers 404 EntityNotFound "The specified VPC origin does not exist."; the recording is 400 InvalidArgument "Id format is invalid". Fixture cloudfront-vpc-origin-get-invalid-id-format#task#1 is the same cause.
5. Cause 3: replay fixture cloudfront-vpc-origin-error-paths, step task#1 — POST /2020-05-31/vpc-origin with a VpcOriginEndpointConfig whose Arn does not validate. The emulator answers 202, creates the origin and sends a Location header; the recording is 400 with Error.Code InvalidArgument and Message "The parameter VpcOriginEndpointConfig Arn ValidationError.", no Location header and no resource created. Fixture cloudfront-vpc-origin-invalid-argument#task#1 is the same cause.
6. Regression guard: a well-formed public-key id and a well-formed vpc-origin id that state does not hold must still answer the not-found refusal, and a valid VpcOriginEndpointConfig must still create a VPC origin with 202.
7. All three causes must match the recordings before closing.
