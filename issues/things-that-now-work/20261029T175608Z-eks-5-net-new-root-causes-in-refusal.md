Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.cancel-update-terminal-state-rejected eks.cluster-deletion-protection-delete-rejected eks.list-associated-access-policies-invalid-scope-rejected eks.addon-catalog-missing-vs-unknown-semantics eks.describe-addon-configuration-schema eks.list-tags-for-resource-not-found eks.untag-resource-not-found
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.cancel-update-terminal-state-rejected|run#11|AssertionError|assert unknown_code == ("ResourceNotFoundException", f"Update {unknown_id} not found"), f"expected ResourceNotFoundException for an unknown updateId, got {unkno
Issue-Key: eks.cluster-deletion-protection-delete-rejected|run#16|AssertionError|assert message == expected, f"unexpected message: {message!r}, expected {expected!r}"
Issue-Key: eks.list-associated-access-policies-invalid-scope-rejected|run#12|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException for a corrupted nextToken, got {code!r}"
Issue-Key: eks.addon-catalog-missing-vs-unknown-semantics|run#14|AssertionError|assert message == (
Issue-Key: eks.describe-addon-configuration-schema|run#15|AssertionError|assert message == expected, message
Issue-Key: eks.list-tags-for-resource-not-found|run#11|AssertionError|assert message == expected, f"message: {message!r} != {expected!r}"
Issue-Key: eks.untag-resource-not-found|run#14|AssertionError|assert msg == "Resource was not found", msg
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# eks: 5 net-new root causes in refusal codes and wording — CancelUpdate unknown id, deletion protection, ListAssociatedAccessPolicies nextToken, add-on catalog lookups and the tagging APIs

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Five further net-new root causes in the eks emulator (crate services/service-com-amazonaws-eks-awswesleyfrontend): refusals carry the right HTTP class but the wrong error code or the wrong message text, so clients and tests that key on the contract diverge. All five must be closed.

Cause 1 of 5 — an unknown updateId is reported with the wrong sentence.
AWS: Smithy: CancelUpdate/DescribeUpdate model ResourceNotFoundException (httpError 404). Recorded behavior: an updateId that does not exist is refused ('ResourceNotFoundException', 'Update <updateId> not found') — fixed text "Update " ... " not found", variable part is the updateId from the request, and there is NO trailing period.
Emulator: answers ResourceNotFoundException with 'No update found for ID: <updateId>.'.
Why: the code is right but the message is a different sentence, so an exact-match assertion (and any log-scraping client) diverges.
Fix: the update-resolution message in the CancelUpdate/DescribeUpdate handlers.
Traces: eks.cancel-update-terminal-state-rejected.

Cause 2 of 5 — DeleteCluster's deletion-protection refusal uses the wrong wording.
AWS: recorded behavior only: deleting a cluster whose DeletionProtection is enabled is refused with the message "Cluster <name> cannot be deleted while DeletionProtection is enabled." — fixed text "Cluster " ... " cannot be deleted while DeletionProtection is enabled.", variable part the cluster name from the request. (The error code the emulator returns already matched; only the sentence diverges.)
Emulator: answers 'Cluster <name> has deletion protection enabled. Disable deletion protection before deleting the cluster.'.
Why: the sentence is the user-visible contract; a client asserting AWS's wording (as this check does) fails, and operators get text real EKS never emits.
Fix: the DeleteCluster handler's deletion-protection guard message.
Traces: eks.cluster-deletion-protection-delete-rejected.

Cause 3 of 5 — a corrupted nextToken is refused with the wrong error code.
AWS: Smithy: ListAssociatedAccessPolicies models InvalidParameterException (400) and InvalidRequestException (400) separately. Recorded behavior: a nextToken that cannot be decoded is refused with error code InvalidParameterException (a malformed parameter), not InvalidRequestException (bad state).
Emulator: answers InvalidRequestException.
Why: pagination wrappers distinguish 'your token is garbage, start over' (InvalidParameterException) from 'the resource state forbids this' (InvalidRequestException); keying on the modeled code picks the wrong recovery path.
Fix: the ListAssociatedAccessPolicies handler's nextToken decoding/validation.
Traces: eks.list-associated-access-policies-invalid-scope-rejected.

Cause 4 of 5 — DescribeAddonConfiguration reports an unknown add-on/version with a generic not-found sentence.
AWS: Smithy: DescribeAddonConfiguration models ResourceNotFoundException (404) with required inputs addonName and addonVersion. Recorded behavior: when the pair is not in the catalog the refusal message is "Schema not found for requested addonName=[<addonName>] and addonVersion=[<addonVersion>]" — fixed text "Schema not found for requested addonName=[", "] and addonVersion=[", "]", with both values copied from the request. (DescribeAddonVersions with an unknown addonName must stay an EMPTY list, which the emulator already does.)
Emulator: answers ResourceNotFoundException with the generic 'The requested resource does not exist.' for both the unknown add-on name and the unknown version.
Why: the message is the only place the lookup echoes WHICH name/version failed; a client cannot tell whether the add-on or the version was wrong, and catalogue-probing tests that assert the sentence fail.
Fix: the DescribeAddonConfiguration handler's catalogue lookup (message construction from the request's addonName/addonVersion).
Traces: eks.addon-catalog-missing-vs-unknown-semantics, eks.describe-addon-configuration-schema.

Cause 5 of 5 — the tagging APIs' not-found message echoes the ARN instead of EKS's fixed sentence.
AWS: Smithy: ListTagsForResource and UntagResource model NotFoundException (httpError 404) — the REST-style error, distinct from the cluster APIs' ResourceNotFoundException. Recorded behavior: for a well-formed ARN naming a resource that does not exist the message is exactly "Resource was not found" — all fixed text, no variable parts.
Emulator: returns code NotFoundException (correct) but message 'The resource <resourceArn> could not be found'.
Why: both failing checks assert the exact sentence after the code; more generally the emulator invents a message template AWS does not use for these two operations, and leaks the request ARN into text that is supposed to be constant.
Fix: the message emitted by the tag-resource resolution shared by ListTagsForResource and UntagResource.
Traces: eks.list-tags-for-resource-not-found, eks.untag-resource-not-found.

## Evidence
Suite simparity-behavioral-aws-v1: 64/310 passed; eks 60/282. None of these checks is marked ALREADY FILED or OWNED in the report.

Cause 1: eks.cancel-update-terminal-state-rejected — AssertionError: expected ResourceNotFoundException for an unknown updateId, got ('ResourceNotFoundException', 'No update found for ID: sim2real-no-such-update-71a0e2762f4b.'); expected ('ResourceNotFoundException', 'Update <id> not found').
Cause 2: eks.cluster-deletion-protection-delete-rejected — AssertionError: unexpected message: 'Cluster sim2real-eks-cl-77454a3d897a has deletion protection enabled. Disable deletion protection before deleting the cluster.', expected 'Cluster sim2real-eks-cl-77454a3d897a cannot be deleted while DeletionProtection is enabled.'.
Cause 3: eks.list-associated-access-policies-invalid-scope-rejected — AssertionError: expected InvalidParameterException for a corrupted nextToken, got 'InvalidRequestException'.
Cause 4: eks.addon-catalog-missing-vs-unknown-semantics — AssertionError: The requested resource does not exist. at `assert message == f"Schema not found for requested addonName=[{unknown}] and addonVersion=[{unknown_version}]"`. eks.describe-addon-configuration-schema — same assertion for addonName=vpc-cni, addonVersion=v9.99.99-eksbuild.99.
Cause 5: eks.list-tags-for-resource-not-found — AssertionError: message: 'The resource arn:aws:eks:us-west-2:000000000000:cluster/sim2real-behav-no-cluster-b038a00835ea could not be found' != 'Resource was not found' (the `assert code == "NotFoundException"` line before it passed). eks.untag-resource-not-found — AssertionError: The resource arn:aws:eks:us-west-2:000000000000:cluster/sim2real-behav-no-cluster-0d3295955d31 could not be found (expected 'Resource was not found').

## Steps to Reproduce
1. Boot the emulator and point a boto3 eks client at it.
2. Cause 1: cancel_update(name=<existing cluster>, updateId='sim2real-no-such-update') -> expect 404 ResourceNotFoundException 'Update sim2real-no-such-update not found'.
3. Cause 2: create a cluster with deletion protection enabled, then delete_cluster -> expect the message 'Cluster <name> cannot be deleted while DeletionProtection is enabled.'.
4. Cause 3: list_associated_access_policies(clusterName=<cluster>, principalArn=<arn>, nextToken='!!corrupt!!') -> expect 400 InvalidParameterException; observe InvalidRequestException.
5. Cause 4: describe_addon_configuration(addonName='vpc-cni', addonVersion='v9.99.99-eksbuild.99') and describe_addon_configuration(addonName='sim2real-not-a-real-addon', addonVersion='v1.0.0-eksbuild.1') -> expect 404 ResourceNotFoundException 'Schema not found for requested addonName=[...] and addonVersion=[...]'.
6. Cause 5: list_tags_for_resource(resourceArn='arn:aws:eks:<region>:<account>:cluster/<absent>') and untag_resource on the same ARN with any tagKeys -> expect 404 NotFoundException with the exact message 'Resource was not found'.
