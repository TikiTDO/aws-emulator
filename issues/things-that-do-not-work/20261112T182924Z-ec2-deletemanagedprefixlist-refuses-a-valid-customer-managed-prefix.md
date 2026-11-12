Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Step: teardown#37
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-consolidated-delete-and-deprovision-lifecycle ec2-managed-prefix-list-entries-and-associations
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DeleteManagedPrefixList|2xx->4xx|status
Issue-Commit: 9f30cc5f50e7736550fc9d928f8a80b554132b0d

# ec2 DeleteManagedPrefixList refuses a valid customer-managed prefix list delete with HTTP 400

> _Note: this issue was identified against the code base at commit `9f30cc5f50e7736550fc9d928f8a80b554132b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteManagedPrefixList

## Description
Cause 1 of 1: fixture ec2-ipam-consolidated-delete-and-deprovision-lifecycle, step teardown#37 (the same cluster also covers ec2-managed-prefix-list-entries-and-associations#teardown#5).
AWS: DeleteManagedPrefixList takes PrefixListId (Smithy: DeleteManagedPrefixListRequest.PrefixListId, required, target PrefixListResourceId) and, for a customer-managed prefix list that exists, is not AWS-managed and has no remaining associations, answers HTTP 200 with DeleteManagedPrefixListResponse carrying the prefixList element in state 'delete-in-progress'. Recorded behavior for both steps is 200.
Emulator: answers HTTP 400 for a prefix list it created earlier in the same fixture and which the fixture has already dissociated, so the delete never happens.
Why: the teardown of any flow that creates a managed prefix list cannot complete - the prefix list leaks, and a client that treats the 400 as a client error (bad id) retries forever or aborts the whole teardown. Every subsequent step that depends on the prefix list being gone is wrong too.
Fix: the DeleteManagedPrefixList handler's precondition/validation path (the lookup that decides the request is invalid) in the prefix-list operation and the prefix-list state it consults.

## Evidence
ec2-ipam-consolidated-delete-and-deprovision-lifecycle#teardown#37, DeleteManagedPrefixList: expected status 200, got 400 (divergence detail: "status: expected 200, got 400"; divergence_key service=ec2|DeleteManagedPrefixList|status=200->400|keys=status). The cluster's second member, ec2-managed-prefix-list-entries-and-associations#teardown#5, fails identically. The recorded 200 response body is the DeleteManagedPrefixListResponse prefixList element; the emulator returns a 4xx error document instead.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay fixture ec2-ipam-consolidated-delete-and-deprovision-lifecycle through teardown#37 (CreateManagedPrefixList, entries/associations work, then DeleteManagedPrefixList on that prefix list id), or ec2-managed-prefix-list-entries-and-associations through teardown#5.
3. The DeleteManagedPrefixList call answers HTTP 400 where the recording has HTTP 200 with the prefixList element in state delete-in-progress.
