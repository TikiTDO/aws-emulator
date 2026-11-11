Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2-trust-store-error-conditions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2-trust-store-error-conditions elbv2-trust-store-error-paths elbv2-truststore-bundle-not-found-error elbv2-truststore-create-bundle-not-found elbv2-truststore-duplicate-and-notfound-errors elbv2-truststore-invalid-bundle-errors elbv2-truststore-lifecycle elbv2-truststore-revocations-workflow elbv2-truststore-shared-association-error elbv2err-trust-store-revocation-content-not-found elbv2env-trust-store-revocations-pagination-and-policy elbv2-trust-store-restricted-error-conditions elbv2-truststore-not-found-errors elbv2-truststore-notfound-errors elasticloadbalancing-elbv2-distinct-error-conditions elbv2-load-balancer-listener-rule-lifecycle-error-conditions
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|CreateTrustStore|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|CreateTrustStore|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateTrustStore|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|CreateTrustStore|2xx->4xx|$.CreateTrustStoreResponse
Issue-Key: elasticloadbalancing|CreateTrustStore|2xx->4xx|status
Issue-Key: elasticloadbalancing|ModifyTrustStore|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|ModifyTrustStore|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|ModifyTrustStore|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DeleteTrustStore|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DeleteTrustStore|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DeleteTrustStore|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeTrustStores|2xx->4xx|$.DescribeTrustStoresResponse
Issue-Key: elasticloadbalancing|DescribeTrustStores|2xx->4xx|status
Issue-Key: elasticloadbalancing|DescribeTrustStores|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTrustStores|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTrustStores|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeTrustStoreAssociations|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeTrustStoreAssociations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeTrustStoreAssociations|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DeleteSharedTrustStoreAssociation|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DeleteSharedTrustStoreAssociation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DeleteSharedTrustStoreAssociation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# elasticloadbalancing ELBv2 trust store create, modify, delete and describe actions answer 400 InvalidAction

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrustStore

## Description
10 causes, one mechanism: the shared 'elasticloadbalancing' front door is the Classic ELB crate, which serves only Version=2012-06-01 and refuses every 2015-12-01 envelope with 400 InvalidAction 'Could not find operation <Action> for version <Version>' (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs); the ELBv2 model answers only under its private route key 'elasticloadbalancingv2'. Related OPEN issues that do NOT cover these fixture steps and carry no wire replay key: 20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md, 20261022T220446Z-elbv2-elastic-load-balancing-v2-is-unreachable-every-request.md. Each cause needs the version dispatch fixed and then that trust-store operation's own handler/validation; the recorded not-found messages are request-derived (they echo the trust store ARN), so that rule is the requirement and the quoted text is an example.

Cause 1 of 10: CreateTrustStore with a CA bundle that is not present in S3.
AWS: Smithy CreateTrustStore(Name 1..32 @pattern ^([a-zA-Z0-9]+-)*[a-zA-Z0-9]+$, CaCertificatesBundleS3Bucket/Key required) declares CaCertificatesBundleNotFoundException, awsQueryError 'CaCertificatesBundleNotFound', HTTP 400 (fixed message, example 'Provided CA certificate bundle was not found').
Emulator: 400 Code 'InvalidAction'.
Why: mTLS setup cannot validate its bundle, and the caller cannot tell a missing object from a missing API.
Fix: version dispatch, then CreateTrustStore's S3 bundle existence check.
Traces: elbv2-trust-store-error-conditions#task#1 (cluster of 7).

Cause 2 of 10: CreateTrustStore success path.
AWS: 200 with CreateTrustStoreResult.TrustStores.member{TrustStoreArn, Name, NumberOfCaCertificates, TotalRevokedEntries, Status 'CREATING'}.
Emulator: 400 InvalidAction, no CreateTrustStoreResponse.
Why: no trust store can be created, so the whole mTLS surface (associations, revocations, bundle retrieval) is unreachable.
Fix: version dispatch, then CreateTrustStore handler/state/serializer including the CREATING->ACTIVE transition.
Traces: elbv2-truststore-duplicate-and-notfound-errors#task#3 (cluster of 5).

Cause 3 of 10: CreateTrustStore during fixture setup.
AWS: as Cause 2, 200 with the created trust store.
Emulator: 400 InvalidAction (expected 200, got 400).
Why: setup for revocation pagination/policy scenarios cannot be built.
Fix: same handler as Cause 2.
Traces: elbv2env-trust-store-revocations-pagination-and-policy#setup#5.

Cause 4 of 10: ModifyTrustStore on an absent trust store.
AWS: TrustStoreNotFoundException / awsQueryError 'TrustStoreNotFound' / 400, message naming the ARN.
Emulator: 400 Code 'InvalidAction', ErrorResponse.RequestId missing.
Why: bundle rotation is unreachable and its not-found branch never fires.
Fix: version dispatch, then ModifyTrustStore handler.
Traces: elbv2-trust-store-error-paths#task#2 (cluster of 4).

Cause 5 of 10: DeleteTrustStore on an absent trust store.
AWS: TrustStoreNotFound / 400.
Emulator: 400 InvalidAction.
Why: teardown and in-use refusals are unreachable.
Fix: version dispatch, then DeleteTrustStore handler.
Traces: elbv2-trust-store-error-paths#task#3 (cluster of 4).

Cause 6 of 10: DescribeTrustStores empty-result success path.
AWS: an unfiltered DescribeTrustStores with none provisioned is 200 with an empty TrustStores element.
Emulator: 400 InvalidAction.
Why: list-then-act code cannot observe an empty account.
Fix: version dispatch, then DescribeTrustStores list serializer.
Traces: elbv2-trust-store-error-paths#task#4.

Cause 7 of 10: DescribeTrustStores for an absent ARN.
AWS: Smithy DescribeTrustStores declares exactly one error, TrustStoreNotFoundException / 'TrustStoreNotFound' / 400, message naming the ARN.
Emulator: 400 Code 'InvalidAction'.
Why: absence is indistinguishable from a dead API.
Fix: version dispatch, then DescribeTrustStores' ARN lookup.
Traces: elasticloadbalancing-elbv2-distinct-error-conditions#task#3.

Cause 8 of 10: DescribeTrustStores for an absent ARN (second recording, same code, different ARN).
AWS: as Cause 7.
Emulator: 400 InvalidAction.
Why: same.
Fix: same handler.
Traces: elbv2-load-balancer-listener-rule-lifecycle-error-conditions#task#5 (cluster of 5).

Cause 9 of 10: DescribeTrustStoreAssociations on an absent trust store.
AWS: TrustStoreNotFound / 400, message naming the ARN.
Emulator: 400 InvalidAction.
Why: the listener-association view of a trust store is unreachable.
Fix: version dispatch, then DescribeTrustStoreAssociations handler.
Traces: elbv2-trust-store-error-conditions#task#2 (cluster of 4).

Cause 10 of 10: DeleteSharedTrustStoreAssociation on an absent trust store.
AWS: TrustStoreNotFound / 400.
Emulator: 400 InvalidAction.
Why: cross-account trust-store sharing cannot be revoked or correctly refused.
Fix: version dispatch, then DeleteSharedTrustStoreAssociation handler.
Traces: elbv2-trust-store-error-paths#task#7 (cluster of 4).

## Evidence
All actual responses are HTTP 400 with $.ErrorResponse.Error.Code 'InvalidAction' and Message 'Could not find operation <Action> for version 2015-12-01'; where an error was expected, $.ErrorResponse.RequestId is also missing.
- elbv2-trust-store-error-conditions#task#1 CreateTrustStore: expected 'CaCertificatesBundleNotFound' (message example 'Provided CA certificate bundle was not found'); got 'InvalidAction'.
- elbv2-truststore-duplicate-and-notfound-errors#task#3 CreateTrustStore: expected 200 + CreateTrustStoreResult.TrustStores.member{Status 'CREATING', NumberOfCaCertificates '1', TotalRevokedEntries '0'}; got 400. elbv2env-trust-store-revocations-pagination-and-policy#setup#5: expected 200, got 400.
- elbv2-trust-store-error-paths#task#2 ModifyTrustStore, #task#3 DeleteTrustStore, #task#7 DeleteSharedTrustStoreAssociation and elbv2-trust-store-error-conditions#task#2 DescribeTrustStoreAssociations: expected 'TrustStoreNotFound'; got 'InvalidAction'.
- elbv2-trust-store-error-paths#task#4 DescribeTrustStores: expected 200 + empty TrustStores; got 400.
- elasticloadbalancing-elbv2-distinct-error-conditions#task#3 and elbv2-load-balancer-listener-rule-lifecycle-error-conditions#task#5 DescribeTrustStores: expected 'TrustStoreNotFound' naming the ARN; got 'InvalidAction'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Signing for service 'elasticloadbalancing', POST 'Action=CreateTrustStore&Version=2015-12-01&Name=probe-ts&CaCertificatesBundleS3Bucket=<bucket>&CaCertificatesBundleS3Key=absent.pem'. Observe 400 InvalidAction instead of 400 CaCertificatesBundleNotFound; with a real bundle, instead of 200 + TrustStores{Status CREATING}.
3. Repeat with Action = ModifyTrustStore / DeleteTrustStore / DescribeTrustStores (unfiltered and with a fabricated ARN) / DescribeTrustStoreAssociations / DeleteSharedTrustStoreAssociation at Version=2015-12-01.
4. Re-replay the fixture steps named in issue_step and trace_ids.
