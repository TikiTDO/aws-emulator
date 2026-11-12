Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.package-association-ops-on-missing-domain
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.package-association-ops-on-missing-domain|run#15|AssertionError|assert message == expected, f'DissociatePackage(unknown domain): expected message {expected!r}, got {message!r}'
Issue-Commit: e2524048f621f2499323d2b8db3a18fb7b1c52a6

# opensearch: DissociatePackage on a nonexistent domain returns AssociatePackage's "domain does not exist" message instead of AWS's software-version refusal text

> _Note: this issue was identified against the code base at commit `e2524048f621f2499323d2b8db3a18fb7b1c52a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DissociatePackage

## Description
This issue carries 1 root cause, in the opensearch service.

Cause 1 of 1: DissociatePackage refuses an unknown DomainName with the wrong ValidationException message — it reuses the message AssociatePackage emits for the same condition, because both operations are refused by one shared decision in the package-association state.

AWS: Smithy: DissociatePackage(PackageID @pattern "^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$" required, DomainName @length 3..28 @pattern "^[a-z][a-z0-9\\-]+$" required) binds AccessDeniedException (403), BaseException (400, defaulted), ConflictException (409), InternalException (500), ResourceNotFoundException (409) and ValidationException (400). The model does not state the refusal wording, so this is recorded behavior only: for a syntactically valid DomainName that does not exist, and a PackageID that matches the service's own pattern but was never created, real AWS refuses with HTTP 400 / error code `ValidationException` and the message `The domain is not on the latest software version. Upgrade the domain to start using this feature. ` — that whole string is FIXED text of this error class, including the TRAILING SPACE, and it has NO variable parts: neither the domain name nor the package id appears in it. The sibling AssociatePackage, for the very same unknown domain, is refused `ValidationException` with a DIFFERENT, templated message whose fixed text is `The domain ` + `<DomainName from the request>` + ` does not exist.` — that one the emulator already gets right. The asymmetry between the two refusals is itself the recorded behavior. (Hypothesis: AWS routes DissociatePackage's domain lookup through a capability/software-version gate that reports the generic upgrade message when it cannot resolve the domain, rather than through the plain existence check AssociatePackage uses.)

Emulator: DissociatePackage(PackageID=<well-formed, never created>, DomainName=<syntactically valid, nonexistent>) answers the correct code `ValidationException` but with the message `The domain <DomainName> does not exist.` — i.e. AssociatePackage's templated text, with the request's domain name interpolated into it.

Why: a client that branches on the refusal text — which is the only thing distinguishing these two refusals, since the error code and HTTP status are identical for both — takes the wrong path. Code that recognises the "not on the latest software version / upgrade the domain" refusal and surfaces an "upgrade your domain" remediation to the user never fires, and conversely code that parses the "domain <name> does not exist" template will extract a domain name from a refusal where AWS never emits one. Clients and log-scrapers keyed on this message class see a message AWS never produces for DissociatePackage.

Fix: the not-found refusal class/message for DissociatePackage. The handler at services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/dissociate_package.rs calls the SHARED `plan_package_dissociation` decision (state) and maps `AssociationRefusal::Validation` straight through, so the dissociation path inherits the association path's message verbatim; the shared decision must distinguish which operation is refusing an unknown domain, or DissociatePackage must carry its own unknown-domain refusal message. Only the message text diverges — the ValidationException class and HTTP 400 are already correct, and the other assertions in this check (permissive ListPackagesForDomain returning an empty list for an unknown domain, DescribePackages returning an empty list for a non-matching filter, AssociatePackage's code AND message, and no association being left behind) all pass and must keep passing.

Traces: opensearch.package-association-ops-on-missing-domain.

Note: the open issue 20261029T182036Z-opensearch-domain-upgrade-and-maintenance-state-machines-report.md does NOT cover this. Its Cause 4 is the plural DissociatePackages(PackageList, DomainName) operation refusing a NOT-ASSOCIATED package on an EXISTING domain with ResourceNotFoundException instead of ValidationException — a different operation, a different condition, and a wrong error CLASS rather than a wrong message. This check was not marked owned by it.

## Evidence
Failing check opensearch.package-association-ops-on-missing-domain, assertion at check_opensearch_package_association_ops_on_missing_domain.py line 52:

  AssertionError: DissociatePackage(unknown domain): expected message 'The domain is not on the latest software version. Upgrade the domain to start using this feature. ', got 'The domain sim2real-behav-os-missing-2f does not exist.'

Expected (real AWS, recorded): HTTP 400, Error.Code = `ValidationException`, Error.Message = `The domain is not on the latest software version. Upgrade the domain to start using this feature. ` — entirely fixed text, trailing space included, no request- or state-derived values.
Actual (emulator): HTTP 400, Error.Code = `ValidationException` (correct), Error.Message = `The domain <DomainName> does not exist.` — fixed text `The domain ` ... ` does not exist.` with the request's DomainName interpolated. (`sim2real-behav-os-missing-2f` in the replay is the per-run generated domain name the harness minted, not part of the requirement.)

The immediately preceding assertion in the same check — AssociatePackage(unknown domain) must answer `ValidationException` with `The domain <DomainName> does not exist.` — PASSES, which is what pins this as a per-operation message divergence and not a generic domain-lookup defect.

## Steps to Reproduce
1. Pick a syntactically valid DomainName matching ^[a-z][a-z0-9\-]+$ with length 3..28 that does not exist in the account (e.g. a generated `sim2real-behav-os-missing-<suffix>` truncated to 28 chars).
2. Pick a PackageID that matches the service's own pattern ^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$ but was never created (e.g. `pkg-` + 16 hex chars), so the refusal cannot be mere format validation.
3. Call opensearch AssociatePackage(PackageID=<that id>, DomainName=<that domain>) and observe ValidationException / `The domain <DomainName> does not exist.` — this already matches AWS.
4. Call opensearch DissociatePackage(PackageID=<that id>, DomainName=<that domain>) and read Error.Code and Error.Message.
5. Observe the code is ValidationException (correct) but the message is `The domain <DomainName> does not exist.`, where AWS returns the fixed string `The domain is not on the latest software version. Upgrade the domain to start using this feature. ` (note the trailing space).
6. Call ListPackagesForDomain(DomainName=<that domain>) again and confirm DomainPackageDetailsList is still empty — the refused calls correctly leave no association.
