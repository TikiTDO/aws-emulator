Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2env-trust-store-revocations-pagination-and-policy
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2env-trust-store-revocations-pagination-and-policy elbv2-truststore-shared-association-error
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|GetResourcePolicy|2xx->4xx|$.GetResourcePolicyResponse
Issue-Key: elasticloadbalancing|GetResourcePolicy|2xx->4xx|status
Issue-Key: elasticloadbalancing|DeleteSharedTrustStoreAssociation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: d77a58d64fb70ca743013b1e12d44799c7a2b26f

# elbv2 trust store plane diverges: GetResourcePolicy refuses an existing trust store, and the same-account association refusal is reworded

> _Note: this issue was identified against the code base at commit `d77a58d64fb70ca743013b1e12d44799c7a2b26f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy, DeleteSharedTrustStoreAssociation

## Description
Two causes, both in the ELBv2 trust-store plane: a modeled read of a trust store's resource policy is refused outright, and the one refusal the plane does produce carries the emulator's own wording instead of AWS's.

Cause 1 of 2 — GetResourcePolicy answers HTTP 400 for a trust store that exists (fixture elbv2env-trust-store-revocations-pagination-and-policy, step task#6, operation GetResourcePolicy).

AWS: GetResourcePolicy takes a single required ResourceArn and returns GetResourcePolicyOutput with a Policy member (Smithy: elasticloadbalancingv2 GetResourcePolicy; input ResourceArn required; output Policy @length min 1; the ONLY modeled error is ResourceNotFoundException, errorKind client, awsQueryError code 'ResourceNotFound', httpError 400). So the single modeled refusal class is reserved for an ARN that names no stored resource; an ARN that DOES name a trust store the account created is a success. For a trust store with no resource policy attached, the recording shows AWS answering HTTP 200 with a GetResourcePolicyResponse whose GetResourcePolicyResult.Policy is the empty JSON document '{}' (recorded behavior only for the empty-policy case — the model does not state what an unattached policy reads back as, and '{}' satisfies the Policy @length min 1 constraint). The requirement is therefore: an existing trust store ARN is a 200, and the absence of an attached policy is represented in the body, not by an error.

Emulator: the same request answers HTTP 400 and emits no GetResourcePolicyResponse at all ($.GetResourcePolicyResponse missing). The replay captured only the status for the refusal, so the error code and message the emulator chose are not in the replay record; whatever they are, 400 is wrong for this input, because the only refusal GetResourcePolicy models is ResourceNotFound and the ARN under test is a trust store the preceding steps of the same fixture created and then successfully described. Hypothesis: the resource-policy read is resolved against a policy store that only has an entry once a policy has been PUT, so 'no policy attached' is being conflated with 'resource not found'.

Why: a client relying on AWS's behavior reads a resource's policy to decide whether to attach, merge or replace it — read-then-modify is the only safe way to edit a resource policy. Against the emulator that read raises ResourceNotFound-class failure for every trust store that has not yet had a policy attached, i.e. exactly the case where the client is about to attach one. The client concludes the trust store does not exist and aborts, so the resource-policy path is unreachable: sharing a trust store cross-account can never be set up, and any describe/modify loop that first reads the policy fails on a resource that is demonstrably present.

Fix: the GetResourcePolicy handler and the trust-store/resource-policy state it reads — resolving the ResourceArn must succeed whenever the ARN names a stored ELBv2 resource, and an absent policy must serialize as an empty policy document in the 200 body rather than being turned into a 400. ResourceNotFoundException must be reserved for an ARN that names nothing.

Cause 2 of 2 — the DeleteSharedTrustStoreAssociation same-account refusal uses emulator-authored wording (fixture elbv2-truststore-shared-association-error, step task#4, operation DeleteSharedTrustStoreAssociation).

AWS: DeleteSharedTrustStoreAssociation takes required TrustStoreArn and ResourceArn and models DeleteAssociationSameAccountException (Smithy: elasticloadbalancingv2 DeleteSharedTrustStoreAssociation; errors DeleteAssociationSameAccountException [awsQueryError code 'DeleteAssociationSameAccount', httpError 400], TrustStoreAssociationNotFoundException, TrustStoreNotFoundException). The operation removes a CROSS-account association, so being handed a trust store and a resource that live in the same account is refused. The recorded response is HTTP 400, Error.Type 'Sender', Error.Code 'DeleteAssociationSameAccount', with the message: "Cannot remove the association because the trust store ARN '<TrustStoreArn>' and resource ARN '<ResourceArn>' belong to the same account". The fixed text of that template is everything outside the quoted ARNs — in particular the phrases "and resource ARN" (no article) and "belong to the same account"; the two variable parts are the TrustStoreArn and ResourceArn taken verbatim from the request.

Emulator: the status (400) and the error code are correct, but the message is rewritten to "Cannot remove the association because the trust store ARN '<TrustStoreArn>' and the resource ARN '<ResourceArn>' are in the same account" — it inserts the article 'the' before 'resource ARN' and substitutes "are in the same account" for AWS's "belong to the same account". The substituted ARN values themselves are correct; only the fixed text diverges.

Why: the error message is part of the contract. Clients, SDK error-mapping layers and test suites that match on an AWS error message to distinguish the same-account refusal from the other two modeled refusals (AssociationNotFound, TrustStoreNotFound) stop matching, and operator-facing logs and runbooks that quote AWS's sentence no longer line up with what the emulator reports, so a correctly-classified refusal is read as an unknown failure.

Fix: the DeleteSharedTrustStoreAssociation handler's refusal path — the message template for the DeleteAssociationSameAccount error must be AWS's fixed text with the request's TrustStoreArn and ResourceArn interpolated, not the emulator's own phrasing.

No open issue covers either cause: the trust-store listings hold only 20261104T094656Z-elastic-load-balancing-v2-describetruststoreassociations-stub.md (a DescribeTrustStoreAssociations scaffold stub, a different operation) and CloudFormation/CloudFront trust-store issues, and the GetResourcePolicy listings hold only other services' resource-policy issues (mpa, secretsmanager, dynamodb, kinesis, bedrock-agent, payment-cryptography). Neither replay key is carried by any open issue.

Traces: elbv2env-trust-store-revocations-pagination-and-policy#task#6 (Cause 1), elbv2-truststore-shared-association-error#task#4 (Cause 2).

## Evidence
Cause 1 — fixture elbv2env-trust-store-revocations-pagination-and-policy, step task#6, service elasticloadbalancing, operation GetResourcePolicy, verdict mismatch, divergence key 'service=elasticloadbalancing|GetResourcePolicy|status=200->400|keys=$.GetResourcePolicyResponse,status':
  status: expected 200, got 400
  $.GetResourcePolicyResponse: missing (expected {'GetResourcePolicyResult': {'Policy': '{}'}, 'ResponseMetadata': {'RequestId': '<shaped-not-asserted>'}}, got None)
Expected on AWS: HTTP 200 and a GetResourcePolicyResponse whose GetResourcePolicyResult.Policy is the empty policy document '{}' (the RequestId is a per-run value and is not asserted). Actual on the emulator: HTTP 400 and no GetResourcePolicyResponse element. Smithy basis: GetResourcePolicy input ResourceArn (required), output Policy (@length min 1), sole modeled error ResourceNotFoundException (client, awsQueryError 'ResourceNotFound', httpError 400).

Cause 2 — fixture elbv2-truststore-shared-association-error, step task#4, service elasticloadbalancing, operation DeleteSharedTrustStoreAssociation, verdict mismatch, divergence key 'service=elasticloadbalancing|DeleteSharedTrustStoreAssociation|status=400->400|keys=$.ErrorResponse.Error.Message'. Status (400) and Error.Code match; $.ErrorResponse.Error.Message diverges:
  expected: "Cannot remove the association because the trust store ARN '<trust-store-arn>' and resource ARN '<resource-arn>' belong to the same account"
  actual:   "Cannot remove the association because the trust store ARN '<trust-store-arn>' and the resource ARN '<resource-arn>' are in the same account"
Fixed text vs values: the fixed text is "Cannot remove the association because the trust store ARN '" + "' and resource ARN '" + "' belong to the same account"; the two variable parts are the TrustStoreArn and the ResourceArn, echoed from the request. The specific ARNs in the recording (a truststore/... and a listener/app/... ARN under account 000000000000 in us-west-2, carrying generated resource names and ids) are per-run values that merely fill the template and are not part of the requirement. The divergences are the inserted article 'the' before 'resource ARN' and 'are in' replacing 'belong to'. Smithy basis: DeleteAssociationSameAccountException, awsQueryError code 'DeleteAssociationSameAccount', httpError 400.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 against it.
3. Cause 1: open fixture elbv2env-trust-store-revocations-pagination-and-policy and run it to step task#6 — a trust store is created and its revocations are listed, then GetResourcePolicy is called with that trust store's ARN as ResourceArn. Observe HTTP 400 with no GetResourcePolicyResponse, where the recording has HTTP 200 and GetResourcePolicyResult.Policy = '{}'.
4. Cause 2: open fixture elbv2-truststore-shared-association-error and run it to step task#4 — DeleteSharedTrustStoreAssociation is called with a TrustStoreArn and a ResourceArn (a listener) that live in the SAME account. Observe HTTP 400 with Error.Code 'DeleteAssociationSameAccount' (correct) but Error.Message reading '... and the resource ARN '...' are in the same account' instead of AWS's '... and resource ARN '...' belong to the same account'.
