Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-deleteVpcOrigin cloudfront.op-contract-createVpcOrigin cloudfront.vpc-origin-create-invalid-arn-rejected cloudfront.vpc-origin-invalid-arn-fails cloudfront.vpc-origin-invalid-endpoint-arn-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-deleteVpcOrigin|run#9|AssertionError|assert code == "InvalidIfMatchVersion", f"expected InvalidIfMatchVersion, got {code}"
Issue-Key: cloudfront.op-contract-createVpcOrigin|_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: cloudfront.vpc-origin-create-invalid-arn-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.vpc-origin-invalid-arn-fails|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudfront.vpc-origin-invalid-endpoint-arn-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 87482cc5c5e409200c9575bf7c958860b805c188

# cloudfront: 3 net-new VPC-origin causes — DeleteVpcOrigin answers 501 NotImplemented, a duplicate VPC-origin Name is accepted, and the malformed-Arn refusal is emulator-worded

> _Note: this issue was identified against the code base at commit `87482cc5c5e409200c9575bf7c958860b805c188`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVpcOrigin

## Description
Three net-new root causes (3 causes) in the CloudFront VPC-origin control plane. Cause 3 is the wording of a refusal the open issue 20261111T194302Z-cloudfront-vpc-origin-and-public-key-requests-skip.md describes as a missing refusal (its Cause 3: CreateVpcOrigin accepted an invalid Arn with 202); the emulator now refuses the call, but with an emulator-authored message, and that issue carries none of these behavioral replay keys. No open or fixed issue names DeleteVpcOrigin at all.

Cause 1 of 3: DeleteVpcOrigin answers 501 NotImplemented for every call, so neither its IfMatch refusal nor the delete itself exists.
AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json): DeleteVpcOrigin input Id (@required, @httpLabel) and IfMatch (@required, @httpHeader If-Match); http {DELETE /2020-05-31/vpc-origin/{Id}, code 202}; output VpcOrigin + ETag header; modeled errors AccessDenied (403), CannotDeleteEntityWhileInUse (409), EntityNotFound (404), IllegalDelete (400), InvalidArgument (400), InvalidIfMatchVersion (400, shape doc "The If-Match version is missing or not valid."), PreconditionFailed (412), UnsupportedOperation (400). Recorded behavior: an If-Match value that is not the resource's current ETag is refused with HTTP 400 and error Code 'InvalidIfMatchVersion'; with the correct ETag the delete succeeds (202) and GetVpcOrigin and a second delete then both fail EntityNotFound. A delete attempted while the origin is still Deploying is IllegalDelete (400) until the create propagates.
Emulator: every call returns HTTP 501 with Code 'NotImplemented' and "operation DeleteVpcOrigin is scaffolded but not implemented in this emulator" — services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/delete_vpc_origin.rs still carries the SCAFFOLDED marker and returns crate::scaffolded::Scaffolded::not_implemented("DeleteVpcOrigin").
Why: 501 is not a modeled outcome of this operation, so no SDK error branch matches it; a caller cannot delete a VPC origin at all, and the ETag precondition contract (the whole point of the required If-Match header) is unobservable. Every test and teardown path that removes a VPC origin leaks the resource and then collides with it (EntityLimitExceeded, duplicate names), so the divergence compounds across a session.
Fix: implement DeleteVpcOrigin in its handler (src/ops/delete_vpc_origin.rs) and delete the SCAFFOLDED marker: validate the If-Match header against the stored ETag and refuse a non-matching value with InvalidIfMatchVersion (400), refuse a still-deploying origin with IllegalDelete (400), remove the record and answer 202 with the VpcOrigin and its ETag otherwise, and answer EntityNotFound (404) for an id the store does not hold.
Traces: cloudfront.op-contract-deleteVpcOrigin.

Cause 2 of 3: CreateVpcOrigin accepts a second VPC origin with a Name that is already in use instead of refusing EntityAlreadyExists.
AWS: Smithy: CreateVpcOrigin input VpcOriginEndpointConfig @required (its members Name, Arn, HTTPPort, HTTPSPort, OriginProtocolPolicy all @required); http {POST /2020-05-31/vpc-origin, code 202}; modeled errors AccessDenied (403), EntityAlreadyExists (409), EntityLimitExceeded (400), InconsistentQuantities (400), InvalidArgument (400), InvalidTagging (400), UnsupportedOperation (400). Recorded behavior: re-submitting a VpcOriginEndpointConfig whose Name is already held by an existing VPC origin is refused with error Code 'EntityAlreadyExists' (HTTP 409 per the model) and no second origin is created — an independent read still shows exactly one origin under that Name.
Emulator: answers HTTP 202 and mints a SECOND VPC origin with a new id for the same Name; src/ops/create_vpc_origin.rs checks tagging, list quantities, the ARN prefix and MAX_VPC_ORIGINS, but never the uniqueness of VpcOriginEndpointConfig.Name.
Why: Name is the identity a caller uses to find or re-create an origin idempotently. With duplicates accepted, a retried create (after a timeout, or a re-run of a provisioning step) silently doubles the resource instead of being rejected, later lookups by Name are ambiguous, and the negative half of the create contract passes vacuously.
Fix: CreateVpcOrigin's validation (src/ops/create_vpc_origin.rs) — before minting an id, reject a VpcOriginEndpointConfig.Name the store already holds with EntityAlreadyExists (409) and create nothing.
Traces: cloudfront.op-contract-createVpcOrigin.

Cause 3 of 3: CreateVpcOrigin's malformed-Arn refusal carries a generic emulator-authored message instead of AWS's parameter-named wording.
AWS: Smithy: as Cause 2; InvalidArgument (400) is modeled on CreateVpcOrigin and VpcOriginEndpointConfig.Arn is @required. Recorded behavior: a VpcOriginEndpointConfig.Arn that is not an ARN at all is refused with HTTP 400, error Code 'InvalidArgument' and Message "The parameter VpcOriginEndpointConfig Arn Invalid Format." — the fixed text is "The parameter " and " Invalid Format."; the variable part is the path of the offending input member (here VpcOriginEndpointConfig Arn), taken from the request. Nothing is created, and ListVpcOrigins shows no origin under the rejected Name.
Emulator: refuses with the right code but the message "ARNs must start with 'arn:'" — crate::state::malformed_arn(), the service's one generic ARN-form sentence shared with the resource-policy surface, which names neither the operation's parameter nor AWS's error class.
Why: the message is the only part of the refusal that tells a caller WHICH member of a nested configuration was rejected; AWS names the member path, so an operator or a client that surfaces the message can fix the right field. A generic sentence about ARNs forces the caller to guess among several ARN-valued members, and any client or test matching AWS's wording breaks.
Fix: CreateVpcOrigin's validation (src/ops/create_vpc_origin.rs) — raise InvalidArgument with the operation's own parameter-named message, built from the offending member path ("The parameter VpcOriginEndpointConfig Arn Invalid Format."), rather than the shared crate::state::malformed_arn() text.
Traces: cloudfront.vpc-origin-create-invalid-arn-rejected, cloudfront.vpc-origin-invalid-arn-fails, cloudfront.vpc-origin-invalid-endpoint-arn-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 404/632 checks passed (63.9%), 160 mismatched, 68 errored, bootstrap_signal false. None of the five checks below is marked ALREADY FILED or OWNED in the report; all are listed under the '### cloudfront' heading. (The report does mark other checks as blocked by the unimplemented ListVpcOrigins stub; DeleteVpcOrigin is NOT among the stubs it names, and no issue on file names DeleteVpcOrigin.)

Cause 1 — cloudfront.op-contract-deleteVpcOrigin, at the IfMatch boundary (cf.delete_vpc_origin(Id=<id>, IfMatch="<not the current ETag>")):
  AssertionError: expected InvalidIfMatchVersion, got NotImplemented
  The emulator's error body for that call is Code 'NotImplemented', message "operation DeleteVpcOrigin is scaffolded but not implemented in this emulator" (HTTP 501). Source: src/ops/delete_vpc_origin.rs line 4 carries "//! SCAFFOLDED, NOT IMPLEMENTED" and line 169 returns Err(crate::scaffolded::Scaffolded::not_implemented("DeleteVpcOrigin")).

Cause 2 — cloudfront.op-contract-createVpcOrigin, at the duplicate-Name boundary (the same VpcOriginEndpointConfig submitted twice; the check asserts code EntityAlreadyExists):
  AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'Location': 'https://cloudfront.amazonaws.com/2020-05-31/vpc-origin/<second generated id>', 'ETag': '<etag>', 'VpcOrigin': {'Id': '<second generated id>', 'Status': 'Deploying', 'VpcOriginEndpointConfig': {'Name': '<the name already in use>', 'Arn': 'arn:aws:ec2:<region>:<account>:instance/<id>', 'HTTPPort': 80, 'HTTPSPort': 443, 'OriginProtocolPolicy': 'http-only'}}}
  i.e. a second origin with a new id was created for a Name already held.

Cause 3 — three checks submit VpcOriginEndpointConfig.Arn = "not-a-valid-arn" and reach the same message assertion after the code assertion (InvalidArgument) passes:
  AssertionError: expected message 'The parameter VpcOriginEndpointConfig Arn Invalid Format.', got "ARNs must start with 'arn:'"
  (cloudfront.vpc-origin-create-invalid-arn-rejected, cloudfront.vpc-origin-invalid-arn-fails, cloudfront.vpc-origin-invalid-endpoint-arn-rejected). Source: src/ops/create_vpc_origin.rs lines 194-198 — `if !config.arn.starts_with(crate::state::ARN_PREFIX) { ... InvalidArgument(crate::state::malformed_arn()) }`.

Fixed vs variable parts: in Cause 3 the message is fixed text around one variable part, the offending input member path from the request ("VpcOriginEndpointConfig Arn"); the error codes in Causes 1 and 2 are requirements, while the generated VPC-origin ids, ETags, account id, region, instance id, Location host, request ids and timestamps quoted above are per-run values and are not part of any requirement. Model grounding: smithy_query query=operation operation=CreateVpcOrigin,DeleteVpcOrigin on services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json — EntityAlreadyExists (409) and InvalidArgument (400) modeled on CreateVpcOrigin (success 202); InvalidIfMatchVersion (400), IllegalDelete (400) and EntityNotFound (404) modeled on DeleteVpcOrigin (success 202, If-Match @required header).

## Steps to Reproduce
1. git_clone the repository and run the behavioral suite (run_behavioral_suite), which builds and boots the emulator from this checkout.
2. Cause 1: create a VPC origin (CreateVpcOrigin over a real backing resource), then call DELETE /2020-05-31/vpc-origin/{Id} with If-Match set to a value that is not the origin's current ETag. The emulator answers 501 NotImplemented; AWS answers 400 InvalidIfMatchVersion. Then repeat with the correct ETag: AWS answers 202 with the VpcOrigin and its ETag, after which GetVpcOrigin and a second delete both answer EntityNotFound (and a delete while the origin is still Deploying answers IllegalDelete). Re-replay cloudfront.op-contract-deleteVpcOrigin.
3. Cause 2: submit CreateVpcOrigin twice with the identical VpcOriginEndpointConfig (same Name). The second call must be refused EntityAlreadyExists and must create nothing — an independent read must still show exactly one origin under that Name. The emulator currently answers 202 and mints a second origin. Re-replay cloudfront.op-contract-createVpcOrigin.
4. Cause 3: call CreateVpcOrigin with VpcOriginEndpointConfig.Arn = "not-a-valid-arn". The code must be InvalidArgument (it already is) and the message must be "The parameter VpcOriginEndpointConfig Arn Invalid Format." with the offending member path taken from the request; the emulator sends "ARNs must start with 'arn:'". Confirm no origin exists under the rejected Name. Re-replay cloudfront.vpc-origin-create-invalid-arn-rejected, cloudfront.vpc-origin-invalid-arn-fails, cloudfront.vpc-origin-invalid-endpoint-arn-rejected.
5. Regression guard: a valid VpcOriginEndpointConfig with a fresh Name must still create a VPC origin with 202, a Location and an ETag, and reach Deployed.
6. All three causes must pass before closing; re-replay every check id in trace_ids.
