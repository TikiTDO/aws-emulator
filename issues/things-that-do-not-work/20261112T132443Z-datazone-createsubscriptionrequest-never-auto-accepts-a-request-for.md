Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.subscription-request-double-accept-conflict-rejected datazone.subscription-request-reject-transitions-status
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone.subscription-request-double-accept-conflict-rejected|run#7|AssertionError|assert request["status"] == "ACCEPTED", (
Issue-Key: datazone.subscription-request-reject-transitions-status|run#7|AssertionError|assert request["status"] == "ACCEPTED", (
Issue-Commit: 88d6810d376793fb3a66f38c34bbaf625866946f

# datazone CreateSubscriptionRequest never auto-accepts a request for a listing with no subscription target (always PENDING)

> _Note: this issue was identified against the code base at commit `88d6810d376793fb3a66f38c34bbaf625866946f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubscriptionRequest

## Description
One net-new root cause in datazone.

Cause 1 of 1: CreateSubscriptionRequest hard-codes the new request's status to PENDING, so a request whose subscribed listing has no subscription target is never auto-resolved to ACCEPTED. Failing checks: datazone.subscription-request-double-accept-conflict-rejected, datazone.subscription-request-reject-transitions-status.

AWS: Smithy: CreateSubscriptionRequest output member `status` (SubscriptionRequestStatus, one of ACCEPTED | PENDING | REJECTED) is **required**; output also declares optional `existingSubscriptionId` (SubscriptionId), `reviewerId`, `decisionComment`, `updatedAt` (required) and `updatedBy`. The model does not state WHEN each status member is produced, so for the auto-resolution rule itself this is recorded behavior only: against real DataZone, CreateSubscriptionRequest for a listing that publishes a plain custom asset type — i.e. a listing with no SubscriptionTarget behind it — returns 200 with `status` already `ACCEPTED` (no manual approval step), and the immediately following GetSubscriptionRequestDetails read confirms `ACCEPTED` with NO `existingSubscriptionId` member present (no Subscription materializes for a no-target listing). Only a request against a listing that does have a subscription target sits `PENDING` awaiting an explicit Accept/Reject decision. The two recordings then exercise the consequences of that resolved state: AcceptSubscriptionRequest on an already-resolved request is refused 400 ValidationException with message `The approved subscription is already revoked or unsubscribed.` (entirely fixed text — no request- or state-derived parts), and RejectSubscriptionRequest on a non-PENDING request is refused 400 ValidationException with message `Cannot reject a subscription request with status ACCEPTED`, whose fixed text is `Cannot reject a subscription request with status ` followed by one variable part, the request's CURRENT SubscriptionRequestStatus member taken from stored state (`ACCEPTED` in these recordings, and `REJECTED` for an already-rejected one — never hard-code `ACCEPTED`). A refused reject must leave `status` and `decisionComment` unchanged.

Emulator: services/service-com-amazonaws-datazone-datazone/src/ops/create_subscription_request.rs builds the stored record with `status: SUBSCRIPTION_REQUEST_STATUS_PENDING.to_string()` unconditionally, with `reviewer_id: None`, `decision_comment: None`, `existing_subscription_id: None`, and never inspects whether the subscribed listings have any subscription target. The 200 response therefore reports `status` = `PENDING` where AWS reports `ACCEPTED`; both checks abort at that first assertion (`expected a listing with no SubscriptionTarget to auto-resolve to ACCEPTED, got 'PENDING'` / `expected auto-accept for an asset with no subscription target, got 'PENDING'`), so the downstream refusals were never reached. Note the same file's comment asserts the auto-resolution cannot happen because AcceptSubscriptionRequest is a scaffold — that is the defect, not a justification. Related and still open for the decision operations: 20261030T112352Z-datazone-updateaccountpool-stub.md style scaffold stubs — AcceptSubscriptionRequest has a stub issue filed from the checkout, and RejectSubscriptionRequest (src/ops/reject_subscription_request.rs) still returns `Scaffolded::not_implemented("RejectSubscriptionRequest")`, so the remainder of these two checks will keep failing until both decision paths exist alongside this fix. No open issue (checked: the datazone domain-lifecycle, error-message-wording, ListAccountPools, custom-asset-publish and portalUrl issues) covers the create-time status resolution.

Why: a client that subscribes to a listing with no subscription target believes access is still awaiting review. It polls GetSubscriptionRequestDetails for ACCEPTED forever, or drives an approval workflow (AcceptSubscriptionRequest) that against AWS would be refused as redundant, and it never sees the access it already has. Conversely, code that relies on the refusal of a redundant Accept/Reject sees it succeed or 501 instead of 400 ValidationException, so idempotency and state-machine guards written against AWS misbehave.

Fix: the CreateSubscriptionRequest handler's status resolution (services/service-com-amazonaws-datazone-datazone/src/ops/create_subscription_request.rs) plus the shared subscription-request state in src/state.rs: decide the created request's SubscriptionRequestStatus from whether the subscribed listings resolve to any subscription target instead of hard-coding PENDING, and leave `existingSubscriptionId` absent when no Subscription is materialized. The dependent refusals belong in the AcceptSubscriptionRequest and RejectSubscriptionRequest handlers' state-machine validation, which must refuse a non-PENDING request with 400 ValidationException and the messages quoted above rather than 501.

Traces: datazone.subscription-request-double-accept-conflict-rejected, datazone.subscription-request-reject-transitions-status.

## Evidence
Check datazone.subscription-request-reject-transitions-status, line 34:
  assert request["status"] == "ACCEPTED"
  AssertionError: expected auto-accept for an asset with no subscription target, got 'PENDING'

Check datazone.subscription-request-double-accept-conflict-rejected, line 34:
  assert request["status"] == "ACCEPTED"
  AssertionError: expected a listing with no SubscriptionTarget to auto-resolve to ACCEPTED, got 'PENDING'

Expected (AWS, recorded): CreateSubscriptionRequest -> 200, status="ACCEPTED", no existingSubscriptionId; GetSubscriptionRequestDetails -> status="ACCEPTED"; AcceptSubscriptionRequest -> 400 ValidationException "The approved subscription is already revoked or unsubscribed."; RejectSubscriptionRequest -> 400 ValidationException "Cannot reject a subscription request with status <current status>", status and decisionComment unchanged.
Actual (emulator): CreateSubscriptionRequest -> 200, status="PENDING" (hard-coded at create_subscription_request.rs line 382: `status: SUBSCRIPTION_REQUEST_STATUS_PENDING.to_string()`); the decision operations are never reached, and RejectSubscriptionRequest is still a 501 scaffold stub (reject_subscription_request.rs line 191).

## Steps to Reproduce
1. CreateDomain and wait for status AVAILABLE; CreateProject twice (producer, consumer).
2. In the producer project: CreateAssetType with formsInput={} (a plain custom type, no subscription target), CreateAsset of that type, CreateListingChangeSet(entityType=ASSET, action=PUBLISH); poll GetListing until status=ACTIVE.
3. CreateSubscriptionRequest(domainIdentifier=<domain>, subscribedPrincipals=[{project:{identifier:<consumer>}}], subscribedListings=[{identifier:<listingId>}], requestReason="...").
4. Read the response's `status`: AWS returns "ACCEPTED"; the emulator returns "PENDING". GetSubscriptionRequestDetails confirms the same divergence and the absence of existingSubscriptionId.
5. Then AcceptSubscriptionRequest(identifier=<requestId>) and RejectSubscriptionRequest(identifier=<requestId>, decisionComment="x") on that resolved request: AWS refuses each with 400 ValidationException and the messages quoted in the evidence; the emulator does not (the request is PENDING, and RejectSubscriptionRequest answers 501).
