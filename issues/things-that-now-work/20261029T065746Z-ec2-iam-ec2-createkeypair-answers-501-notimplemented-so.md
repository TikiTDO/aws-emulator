Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle
Issue-Key: ec2__iam|CreateKeyPair|NotImplemented
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# ec2__iam: EC2 CreateKeyPair answers 501 NotImplemented, so the end-to-end EC2+EBS instance lifecycle check cannot get past its solve phase (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeyPair

## Description
One net-new root cause for this corpus. The replay report did NOT mark this check as covered (its failure surfaces wrapped in the check's own `_PhaseError`, not as a bare ClientError, so the stub attribution missed it). The open issue 20261022T235108Z-ec2-createkeypair-stub.md names the same operation but belongs to corpus simparity-behavioral-aws-v1 and its Issue-Trace-Ids list six other `ec2.*` checks — not this one — so closing that issue would never re-replay this check. Close this one by making the check below pass; it shares a root cause with that issue and should be closed together with it.

Cause 1 of 1: EC2 CreateKeyPair is an unimplemented scaffold stub.

AWS: CreateKeyPair answers HTTP 200 and creates an ED25519 or RSA key pair, returning KeyName, KeyPairId, KeyFingerprint and the unencrypted PEM private key in KeyMaterial (Smithy, services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json: CreateKeyPair input {KeyName (required), KeyType, TagSpecifications, KeyFormat, DryRun}; output CreateKeyPairResult). A second call with a KeyName that already exists is refused `InvalidKeyPair.Duplicate` (HTTP 400, awsQuery error envelope) with the message AWS words as `The keypair '<KeyName>' already exists.` — `<KeyName>` is taken from the request, the rest is fixed text. The created key pair is then visible to DescribeKeyPairs and usable as RunInstances `KeyName`.

Emulator: the call is refused HTTP 501 with error code `NotImplemented` and the message `operation CreateKeyPair is scaffolded but not implemented in this emulator` — a code and message real EC2 never produces for a well-formed request. The handler still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`.

Why: no caller can ever obtain SSH key material, so nothing that needs to reach into a guest works. In this check the whole solve phase (and therefore the post_invoke, cleanup and destroy phases that depend on it) is dead: a deployed instance with a standing EBS volume can be created but never logged into, so the force-detach/terminate behaviour the check exists to assert is unobservable, and the failure arrives as a 501 with no AWS error code for the client to branch on.

Fix: the operation's handler — services/service-com-amazonaws-ec2-amazonec2/src/ops/create_key_pair.rs: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` in `CreateKeyPair::handle` with a real handler that mints the key pair into the EC2 crate's key-pair state (name uniqueness -> InvalidKeyPair.Duplicate, KeyType/KeyFormat handling, tag specifications) and delete that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

Traces: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle (verdict error, phase 3/6 `solve`). Handler citation: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_key_pair.rs.

## Evidence
Expected (AWS): ec2:CreateKeyPair(KeyName=<name>, KeyType="ed25519") -> HTTP 200 with {KeyName, KeyPairId: "key-...", KeyFingerprint, KeyMaterial: "-----BEGIN OPENSSH PRIVATE KEY-----..."}; the same name again -> HTTP 400 InvalidKeyPair.Duplicate, message `The keypair '<KeyName>' already exists.` (fixed text plus the KeyName from the request).

Actual (emulator): HTTP 501, error code `NotImplemented`, message `operation CreateKeyPair is scaffolded but not implemented in this emulator`. Replay detail: `_PhaseError: [phase 3/6 solve] ClientError: An error occurred (NotImplemented) when calling the CreateKeyPair operation: operation CreateKeyPair is scaffolded but not implemented in this emulator`. Both the code and the message text are emulator-authored and have no AWS counterpart.

Supporting: the handler carries the stub census marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of services/service-com-amazonaws-ec2-amazonec2/src/ops/create_key_pair.rs.

## Steps to Reproduce
1. Boot the emulator and call ec2:CreateKeyPair(KeyName="k1", KeyType="ed25519"). Expected: HTTP 200 with KeyPairId/KeyFingerprint/KeyMaterial. Actual: HTTP 501 NotImplemented.
2. Call it again with KeyName="k1". Expected: HTTP 400 InvalidKeyPair.Duplicate, `The keypair 'k1' already exists.`
3. Full replay: deploy the check's ec2-ebs-instance-estate stack (VPC, subnet, security group, EBS volume, IAM role + instance profile), then in the solve phase create a key pair and launch a t3.micro with it and the instance profile, attach the volume at /dev/sdf, force-detach and terminate. Re-replay check ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle to confirm the fix.
