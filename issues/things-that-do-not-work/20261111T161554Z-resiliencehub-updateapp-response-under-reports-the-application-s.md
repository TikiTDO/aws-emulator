Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|UpdateApp|2xx->2xx|$.app.tags.resourceArn
Issue-Commit: ece6d2295e7fd7091ecbef4c65d6af7cf635653d

# resiliencehub UpdateApp response under-reports the application's tag set

> _Note: this issue was identified against the code base at commit `ece6d2295e7fd7091ecbef4c65d6af7cf635653d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApp

## Description
One cause.

=== Cause 1 of 1: UpdateApp's App projection omits a tag entry the application carries, so the returned app.tags is an incomplete tag set ===

AWS: UpdateApp returns the whole application record — Smithy: UpdateApp output `app` (target App, required); App$tags -> TagMap (@sensitive, @length 1..50, key TagKey @length 1..128 @pattern ^[^\x00-\x1f\x22]+$). An application's tag set is service state written by CreateApp$tags and by the tagging plane (TagResource merges keys, UntagResource removes only the keys it names), and every read of the App shape — DescribeApp and UpdateApp alike — reports that state in full, consistently with ListTagsForResource. The model documents the member, not the particular keys, so the specific tag content here is recorded behavior only: in the recording the application carried, at the moment of the update, a tag whose value was that same application's ARN, and AWS's UpdateApp response reported it under app.tags. The key name and the ARN value are variable parts taken from the request/state of that run — they are NOT part of the requirement; the requirement is that UpdateApp's app.tags reproduce the application's complete tag set as the tagging plane left it.

Emulator: answers 200 with an app.tags map that is PRESENT but missing that entry (the diff reports `$.app.tags.resourceArn: missing (expected '<the app's own ARN>', got None)` — only the one tag key is absent, every other asserted member of app matches). So an UpdateApp done after the application was tagged reports fewer tags than the application actually carries. Hypothesis (neither the model nor the recording states this): the tag entry in question is never joined into the record UpdateApp projects — the handler mutates the stored App and then re-reads it (state::reported_app), so either the tag write never landed on the record UpdateApp reads, or the projection drops entries; the recording only establishes that the entry AWS reports is absent from the emulator's response.

Why: a client that reads tags off the write response — IaC/tagging tools that diff the tag map returned by an update against the map they intended, cost-allocation and ownership tooling, and any code that treats UpdateApp's app.tags as authoritative instead of issuing a second ListTagsForResource — concludes a tag it successfully applied has been lost, and will re-tag, report drift, or fail a post-update assertion that passes against AWS. Because the tag set is also the input to tag-based authorization and reporting, an under-reported map is a silent state-fidelity loss rather than a cosmetic one.

Fix: the UpdateApp handler's App projection and the application tag state it reads — the handler's final read of the stored application and the shared app projection it uses must carry the application's complete tag map (the keys CreateApp persisted plus every key the tagging operations merged, minus those explicitly untagged) into the response's app.tags, so UpdateApp, DescribeApp and ListTagsForResource cannot disagree about one application's tags.

Traces: resiliencehub-app-lifecycle#task#5.

## Evidence
Cause 1 — UpdateApp, fixture resiliencehub-app-lifecycle, step task#5:
  expected_status: 200   actual_status: 200  (status and envelope are correct)
  $.app.tags.resourceArn: missing
    expected: the application's own ARN (in the recording 'arn:aws:resiliencehub:us-west-2:000000000000:app/53967d0f-2831-4aea-ba99-dcefe8c4dd00' — a value from that run's state, not part of the requirement)
    actual:   None (the key is absent from the emulator's app.tags; the map itself is present and no other asserted app member diverges)
  Replay divergence_key: service=resiliencehub|UpdateApp|status=200->200|keys=$.app.tags.resourceArn
  affected (1): resiliencehub-app-lifecycle#task#5
  Smithy citations: UpdateApp output app -> App (required); App$tags -> TagMap (@sensitive, @length 1..50); TagKey @length 1..128, @pattern ^[^\x00-\x1f\x22]+$; TagResource/UntagResource bind the same resource by resourceArn. The tag CONTENT of this application is recorded behavior only.
  Note on neighbouring open issues: none of the open resiliencehub issues carries this step's replay key or mentions UpdateApp's tag projection — '20261109T124229Z-resiliencehub-assessment-and-app-compliance-rollup-diverge-from.md' covers assessment/compliance rollup members of App (complianceStatus, driftStatus, rpoInSecs, cost, versionName), '20261106T223808Z-resiliencehub-createapp-policy-not-found-refusal-message-diverges.md' covers a CreateApp refusal message, and '20261104T144946Z-resiliencehubv2-policy-lifecycle-responses-diverge-from-aws-empty-tags.md' is a different service (resiliencehubv2) and is about an EMPTY tags map being dropped, not about a tag entry missing from a populated map.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApp(name=<app>) to mint an application, and apply at least one tag to the returned appArn through the tagging plane (TagResource on that ARN, and/or CreateApp tags= on the create) — e.g. a tag whose value is the application's own ARN, as the recording does.
3. Confirm the application carries the tag: ListTagsForResource(resourceArn=<appArn>) reports it.
4. UpdateApp(appArn=<appArn>, description=<new description>) — any valid partial update.
5. Observe HTTP 200 whose $.app.tags does NOT contain the tag applied in step 2, while AWS's UpdateApp returns the application's complete tag set in $.app.tags.
6. Full replay: call replay_wire_corpus() and re-check fixture resiliencehub-app-lifecycle step task#5; $.app.tags must carry every tag entry the application holds at the time of the update.
