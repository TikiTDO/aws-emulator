Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.cancel-update-unknown-update-id-not-found
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.cancel-update-unknown-update-id-not-found|run#11|AssertionError|assert describe == ("ResourceNotFoundException", f"No update found for ID: {unknown_update_id}"), (
Issue-Commit: 444741a3bf382435d77b2de94f3ea7c734469d34

# eks DescribeUpdate's unknown-updateId 404 uses CancelUpdate's sentence ("Update <id> not found") instead of AWS's "No update found for ID: <id>"

> _Note: this issue was identified against the code base at commit `444741a3bf382435d77b2de94f3ea7c734469d34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUpdate

## Description
One net-new root cause. DescribeUpdate and CancelUpdate do NOT word their unknown-update refusal the same way in Amazon EKS, but the emulator serves one shared sentence for both, so the DescribeUpdate path is now wrong. (The open issue 20261104T105252Z-eks-10-net-new-root-causes-in-not.md, Cause 2, prescribes the single sentence 'Update <updateId> not found' for DescribeUpdate AND CancelUpdate; CancelUpdate now matches AWS, but applying the same text to DescribeUpdate is what this check catches. That issue owns eks.describe-update-not-found and eks.nodegroup-update-serialization; this check is reported unowned and is filed here.)

Cause 1 of 1: DescribeUpdate reports an unresolved updateId with CancelUpdate's sentence.
  AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) — DescribeUpdate GET /clusters/{name}/updates/{updateId}, errors ClientException 400, InvalidParameterException 400, ResourceNotFoundException 404, ServerException 500; CancelUpdate POST /clusters/{name}/updates/{updateId}/cancel-update, same ResourceNotFoundException 404. The model does not state either message, so this is recorded behavior only: on an ACTIVE, EKS-managed cluster an updateId that was never issued is refused 404 ResourceNotFoundException, and the two operations word it differently — CancelUpdate answers 'Update <updateId> not found' (fixed text 'Update ' + ' not found', the only variable part being the requested updateId, no trailing period), while DescribeUpdate answers 'No update found for ID: <updateId>' (fixed text 'No update found for ID: ', the only variable part being the requested updateId, no trailing period). The same recording shows the two other refusals on this path unchanged: an unknown CLUSTER is 'No cluster found for name: <name>.' and a connected (RegisterCluster) cluster answers ClientException for any update id.
  Emulator: DescribeUpdate answers 404 ResourceNotFoundException with CancelUpdate's sentence, 'Update <updateId> not found' — the same string CancelUpdate returns for the same id on the same cluster. Both handlers call one helper, crate::state::update_not_found / update_not_found_message, which formats 'Update {update_id} not found' for every caller.
  Why: a client polling an update id cannot distinguish the two refusals it is documented to get. Teardown and absence probes, the CLI/console text, and any exact-match or regex handling keyed on DescribeUpdate's 'No update found for ID: <id>' sentence (the nodegroup and add-on variants of DescribeUpdate included) see an unrecognised string and classify a well-defined not-found as an unknown failure.
  Fix: the not-found path of DescribeUpdate's handler (services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_update.rs, the final None arm) and the shared message helper it uses in src/state.rs — the sentence must be per-operation, not one string shared with CancelUpdate. CancelUpdate's existing wording is correct and must not change.
  Traces: eks.cancel-update-unknown-update-id-not-found (the DescribeUpdate assertion of that check; its CancelUpdate assertion passes).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 230/352 passed; service eks 215/311.

[mismatch] eks.cancel-update-unknown-update-id-not-found — check_eks_cancel_update_unknown_update_id_not_found.py line 53:
  assert describe == ("ResourceNotFoundException", f"No update found for ID: {unknown_update_id}")
  AssertionError: DescribeUpdate for the same fabricated id '<uuid>' on '<cluster>': expected ResourceNotFoundException, got ('ResourceNotFoundException', 'Update <uuid> not found') (cancel_update gave ('ResourceNotFoundException', 'Update <uuid> not found')).

So for one cluster and one never-issued id the emulator returns the IDENTICAL message from both operations, where the recording has CancelUpdate -> 'Update <id> not found' and DescribeUpdate -> 'No update found for ID: <id>'. The earlier assertions of the same check pass: CancelUpdate on an unknown cluster gives ('ResourceNotFoundException', 'No cluster found for name: <name>.') and CancelUpdate on the real ACTIVE cluster with the fabricated id gives 'Update <id> not found'. The uuid, cluster name and account id in the replay output are per-run template slots, not requirements.

Source: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/describe_update.rs lines 204-216 return crate::state::update_not_found(&input.update_id); services/service-com-amazonaws-eks-awswesleyfrontend/src/state.rs lines 1062-1074 define update_not_found_message() as format!("Update {update_id} not found") and document it as the one sentence "every caller words it this way" — that shared helper is the defect.

Sibling corroboration (both OWNED by 20261104T105252Z-eks-10-net-new-root-causes-in-not.md and therefore NOT filed here): eks.describe-update-not-found and eks.nodegroup-update-serialization both assert DescribeUpdate's message is 'No update found for ID: <updateId>'.

## Steps to Reproduce
1. git clone the project repository and build/boot the emulator from this checkout.
2. Replay the behavioral suite (simparity-behavioral-aws-v1) and re-run check eks.cancel-update-unknown-update-id-not-found.
3. Manually, against the emulator's EKS endpoint:
   a. CreateCluster with accessConfig.authenticationMode=API_AND_CONFIG_MAP and two subnets in two distinct EKS-supported AZs; wait until DescribeCluster reports status ACTIVE.
   b. Pick a random UUID that was never issued as an updateId for that cluster.
   c. CancelUpdate(name=<cluster>, updateId=<uuid>) -> expect 404 ResourceNotFoundException 'Update <uuid> not found' (this already matches).
   d. DescribeUpdate(name=<cluster>, updateId=<uuid>) -> expect 404 ResourceNotFoundException 'No update found for ID: <uuid>'; the emulator answers 'Update <uuid> not found'.
   e. ListUpdates(name=<cluster>) must not list the fabricated id (this already matches).
