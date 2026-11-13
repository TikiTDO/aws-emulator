Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Accepting an agreement provisions entitlements only inside this crate: nothing appears in the Marketplace Entitlement Service

_Filed: 2026-10-24T13:53:46Z_
_Operation: AcceptAgreementRequest | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Accepting an agreement request is what grants the buyer their entitlements: this service models AgreementEntitlement with `resource` ("the resource that the entitlement is provisioned to, such as a product"), `registrationToken` ("required by acceptors to register their account with the product provider") and `licenseArn` (an AWS License Manager licence). In AWS, the entitlement created by accepting a Marketplace agreement is the same entitlement the seller/ISV then reads through the Marketplace Entitlement Service (`GetEntitlements`, keyed by ProductCode/CustomerIdentifier) and the licence that shows up in License Manager.

Expected: after AcceptAgreementRequest succeeds, a GetEntitlements call in the sibling marketplace-entitlement-service emulator (product code of the accepted resource, customer = the accepting account) returns an entitlement for the new agreement, with a dimension/value derived from the accepted term configuration and an expiration date matching the agreement's end time; cancelling the agreement (CancelAgreement / AcceptAgreementCancellationRequest) removes or expires it.

Actual: src/ops/accept_agreement_request.rs calls store.provision_entitlements(...) which writes AgreementEntitlementRecord values into THIS crate's store only (src/state.rs). The marketplace-entitlement-service crate keeps its own store that is populated exclusively through its fixture/admin seam `seed_entitlements` (services/service-com-amazonaws-marketplaceentitlementservice-awsmpentitlementservice/src/state.rs), and nothing in this crate writes to it. So the realistic cross-service workflow "accept an agreement, then query the entitlement as the ISV" returns an empty Entitlements list, and the `licenseArn` this service mints names a License Manager licence no other emulator holds.

Note the fix depends on the entitlement carrying a resource at all, which is the separately filed GetAgreementEntitlements wrong-behavior gap (every entitlement is stored with resource: None): without a product/resource there is no product code to key the propagated entitlement by.

## Evidence

Spec/well-known behavior: AgreementEntitlement$resource, $registrationToken ("only applicable for purchase agreements") and $licenseArn ("the ARN of the AWS License Manager license associated with the entitlement") in this service's model; AWS Marketplace entitlements created by an agreement are the ones marketplace-entitlement-service GetEntitlements reports for the product. Handler/state: src/ops/accept_agreement_request.rs (`let entitlements = store.provision_entitlements(...)`), src/state.rs provision_entitlements/to_entitlements write only this crate's store; services/service-com-amazonaws-marketplaceentitlementservice-awsmpentitlementservice/src/state.rs documents its store as populated "through seed_entitlements — the emulator's fixture/admin seam", and this crate's Cargo.toml declares no dependency on it (no cross-service wiring exists). Test coverage: tests/all/agreement_entitlements.rs exercises GetAgreementEntitlements/SearchAgreements only; no test crosses into the entitlement service.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P12M", dimensions:[{dimensionKey:"Users", dimensionValue:5}]}}}])
2. AcceptAgreementRequest(agreementRequestId=<id>) -> agreementId
3. GetAgreementEntitlements(agreementId=<id>) -> observe an entitlement with a licenseArn minted by this service
4. Call marketplace-entitlement-service GetEntitlements(ProductCode=<the product the agreement entitles>) against the same emulator -> observe an empty Entitlements list, although an agreement granting that entitlement was just accepted
5. Likewise, the licenseArn returned above resolves to no licence in any other emulated service
