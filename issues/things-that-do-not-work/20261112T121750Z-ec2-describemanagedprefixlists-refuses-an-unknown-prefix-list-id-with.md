Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-modify-managed-prefix-list
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-modify-managed-prefix-list|run#24|AssertionError|assert final in (gone, "__empty__"), (
Issue-Commit: 57366a0528cf6a22294cef262806642b58cb385a

# DescribeManagedPrefixLists refuses an unknown prefix list id with the ModifyManagedPrefixList message text ("Prefix List <id> not found.") instead of "The prefix list (<id>) does not exist."

> _Note: this issue was identified against the code base at commit `57366a0528cf6a22294cef262806642b58cb385a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedPrefixLists

## Description
One net-new root cause in the EC2 managed-prefix-list read path.

Cause 1 of 1: DescribeManagedPrefixLists reuses the ModifyManagedPrefixList not-found message when a requested PrefixListIds entry names no prefix list (including one just deleted). Failing check: ec2.op-contract-modify-managed-prefix-list.

AWS:      ec2:DescribeManagedPrefixLists takes `PrefixListIds` ("One or more prefix list IDs") and, for an id that names no prefix list the caller can see, answers HTTP 400 with error code `InvalidPrefixListID.NotFound` and the message AWS words for this read path: fixed text `The prefix list (` + `) does not exist.` wrapping the offending prefix list id taken verbatim from the request's PrefixListIds member — e.g. for a request naming a deleted id the message is "The prefix list (<prefix-list-id>) does not exist.". AWS's distinct wording for the WRITE path, `ModifyManagedPrefixList`, is "Prefix List <prefix-list-id> not found." and the suite asserts that wording separately for ModifyManagedPrefixList (both against a never-existing id and against a deleted one) — those two assertions hold against this emulator, so only the describe-path wording diverges. Smithy: DescribeManagedPrefixLists declares NO errors (EC2 query protocol errors are undeclared in the model: `"errors": []` for both DescribeManagedPrefixLists and ModifyManagedPrefixList), so the code and message here are recorded behavior only; the recording shows `InvalidPrefixListID.NotFound` plus the "The prefix list (...) does not exist." text, and the check also accepts the other AWS-consistent outcome for a deleted id — a 200 with an empty `PrefixLists` list.

Emulator: after DeleteManagedPrefixList, DescribeManagedPrefixLists(PrefixListIds=[<the deleted id>]) answers `InvalidPrefixListID.NotFound` (right code) with the message `Prefix List <prefix-list-id> not found.` — the ModifyManagedPrefixList template. Replay output: `AssertionError: expected deleted prefix list gone, got ('InvalidPrefixListID.NotFound', 'Prefix List pl-00000000000000080 not found.')` (the id is a per-run value filling the template; the requirement is the template, not that id). The emulator neither returns the describe-path message nor an empty PrefixLists page, so a caller gets a third behavior that AWS never produces.

Why:      a client that keys off the refusal text — a deletion-confirmation poll, a retry/ignore predicate, a log matcher, or a test asserting AWS's documented wording — does not recognise the emulator's message, so a prefix list that IS gone reads as an unexpected failure (or a mutation refusal), and the describe and modify refusals become indistinguishable even though AWS words them differently per operation.

Fix:      the DescribeManagedPrefixLists handler's unknown-id refusal: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_managed_prefix_lists.rs imports and calls the WRITE path's helper (`use crate::ops::modify_managed_prefix_list::managed_prefix_list_entries::prefix_list_not_found;` and `return Err(prefix_list_not_found(unknown));`). The describe path needs its own refusal message (keeping code `InvalidPrefixListID.NotFound`), worded as AWS words it for a describe, with the id interpolated from the request; the modify/get helper must keep its own wording so the ModifyManagedPrefixList assertions that currently pass continue to.

Traces:   behavioral check ec2.op-contract-modify-managed-prefix-list (its tail assertion on the post-delete DescribeManagedPrefixLists lookup). Related open issues do NOT cover this: 20261105T083215Z-ec2-4-net-new-root-causes-deletevpcendpoints-per.md owns ec2.managed-prefix-list-lifecycle and 20261101T172217Z-ec2-refusal-fidelity-3-net-new-divergences-in.md owns ec2.create-managed-prefix-list; neither carries this check's replay key nor the describe-path message.

## Evidence
Check: ec2.op-contract-modify-managed-prefix-list [mismatch]

Steps replayed: CreateManagedPrefixList (IPv4, MaxEntries=5, one seed entry) -> poll DescribeManagedPrefixLists to create-complete -> ModifyManagedPrefixList AddEntries (version bumps, modify-complete) -> ModifyManagedPrefixList on a syntactically valid but nonexistent id (passes: InvalidPrefixListID.NotFound, "Prefix List <id> not found.") -> DeleteManagedPrefixList -> DescribeManagedPrefixLists(PrefixListIds=[<deleted id>]).

Expected (AWS, either of):
  - HTTP 400, Code `InvalidPrefixListID.NotFound`, Message `The prefix list (<prefix-list-id>) does not exist.`  — fixed text "The prefix list (" / ") does not exist."; the variable part is the prefix list id copied from the request; or
  - HTTP 200 with `PrefixLists` empty.

Actual (emulator):
  HTTP 400, Code `InvalidPrefixListID.NotFound`, Message `Prefix List <prefix-list-id> not found.`  — the ModifyManagedPrefixList template.

Harness line: `AssertionError: expected deleted prefix list gone, got ('InvalidPrefixListID.NotFound', 'Prefix List pl-00000000000000080 not found.')` (pl-00000000000000080 is this run's generated id, not part of the requirement).

Source: services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_managed_prefix_lists.rs line 145 imports `prefix_list_not_found` from the modify op and line 270 returns it for an id that matches no candidate list.

Model: smithy_query(operation="DescribeManagedPrefixLists,ModifyManagedPrefixList") -> both report `"errors": []`, so the refusal code/message are recorded behavior only.

## Steps to Reproduce
1. ec2 create-managed-prefix-list --prefix-list-name <name> --max-entries 5 --address-family IPv4 --entries Cidr=10.10.0.0/32,Description=seed ; note the returned PrefixListId.
2. Poll ec2 describe-managed-prefix-lists --prefix-list-ids <id> until State == create-complete.
3. ec2 delete-managed-prefix-list --prefix-list-id <id>.
4. ec2 describe-managed-prefix-lists --prefix-list-ids <id>.
   Observed: 400 InvalidPrefixListID.NotFound with Message "Prefix List <id> not found.".
   Expected: 400 InvalidPrefixListID.NotFound with Message "The prefix list (<id>) does not exist.", or 200 with an empty PrefixLists list.
5. Control that must keep passing: ec2 modify-managed-prefix-list --prefix-list-id <id> --current-version <n> --add-entries Cidr=10.10.3.0/32 still answers InvalidPrefixListID.NotFound with "Prefix List <id> not found.".
6. Re-replay behavioral check ec2.op-contract-modify-managed-prefix-list to confirm.
