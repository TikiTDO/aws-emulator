Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__opensearch.xsvc-opensearch-3-disabled-cmk-create-domain-rejected kms__opensearch.xsvc-opensearch-3-cross-region-cmk-create-domain-rejected
Issue-Key: kms__opensearch.xsvc-opensearch-3-disabled-cmk-create-domain-rejected|run#11|AssertionError|raise AssertionError(
Issue-Key: kms__opensearch.xsvc-opensearch-3-cross-region-cmk-create-domain-rejected|run#11|AssertionError|raise AssertionError(
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# CreateDomain does not validate EncryptionAtRestOptions.KmsKeyId: disabled and cross-region CMKs are accepted

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One root cause with two observed facets, both of which must stop passing before this is closed.

Cause 1 of 1: CreateDomain accepts an EncryptionAtRestOptions.KmsKeyId that OpenSearch must refuse — a CMK whose key state is Disabled, and a CMK in a different region from the domain.
AWS: OpenSearch resolves the supplied KMS key before creating the domain and refuses it with a 400 ValidationException when the key cannot be used for the domain's encryption at rest. Smithy: opensearch#CreateDomain takes EncryptionAtRestOptions{Enabled, KmsKeyId} and models ValidationException (httpError 400) among its errors; a KMS CMK is a regional resource, so a key ARN whose region differs from the domain's region is not resolvable by the domain, and a Disabled key cannot be used for cryptographic operations. Recorded behavior: both requests are refused and no domain is created.
Emulator: both CreateDomain calls return 200 and a DomainStatus with an ARN — 'CreateDomain with DISABLED key arn:aws:kms:us-west-2:...:key/<id> was accepted: arn:aws:es:us-west-2:...:domain/<name>' and 'CreateDomain in us-west-2 with us-east-1 key arn:aws:kms:us-east-1:...:key/<id> was accepted'. The KmsKeyId is stored without being resolved in KMS, and without a region or key-state check.
Why: a client validating its encryption posture gets a domain that reports encryption at rest against a key that cannot decrypt it. Negative tests for key state and key locality pass silently, so misconfigurations that real AWS catches at create time are deferred to never, and the emulated estate's encryption guarantees are not real.
Fix: CreateDomain's EncryptionAtRestOptions validation in the OpenSearch service crate — resolve KmsKeyId against the KMS key store, require the key's region to equal the domain's region, require the key state to be Enabled, and return ValidationException (HTTP 400) otherwise, creating no domain.
Traces: kms__opensearch.xsvc-opensearch-3-disabled-cmk-create-domain-rejected, kms__opensearch.xsvc-opensearch-3-cross-region-cmk-create-domain-rejected.

## Evidence
Expected: CreateDomain(EncryptionAtRestOptions={'Enabled': True, 'KmsKeyId': <disabled key arn>}) -> HTTP 400 ValidationException, no domain created. Same for a key ARN whose region differs from the domain's region.
Actual (disabled key): AssertionError: CreateDomain with DISABLED key 'arn:aws:kms:us-west-2:000000000000:key/b6719b90-b895-4e12-b9aa-2d8c79ba3da7' was accepted: 'arn:aws:es:us-west-2:000000000000:domain/sim2real-xkd-f55834dbc979'
Actual (cross-region key): AssertionError: CreateDomain in us-west-2 with us-east-1 key 'arn:aws:kms:us-east-1:000000000000:key/fb1c958f-9b8f-488a-bdfa-e030bbb0daff' was accepted: 'arn:aws:es:us-west-2:000000000000:domain/sim2real-xkr-4a66ee203c5c'

## Steps to Reproduce
1. kms.create_key(), then kms.disable_key(KeyId=...).
2. opensearch.create_domain(DomainName=<name>, EngineVersion='OpenSearch_2.11', ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1}, EBSOptions={'EBSEnabled':True,'VolumeType':'gp3','VolumeSize':10}, EncryptionAtRestOptions={'Enabled':True,'KmsKeyId':<disabled key arn>}) -> accepted (expected ValidationException).
3. Repeat with a CMK created in a different region than the opensearch client's region -> also accepted.
4. Replay kms__opensearch.xsvc-opensearch-3-disabled-cmk-create-domain-rejected and kms__opensearch.xsvc-opensearch-3-cross-region-cmk-create-domain-rejected.
