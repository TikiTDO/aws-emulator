Issue-Source: trace-sweep-divergence
Issue-Trace: servicecatalog-portfolio-principal-association
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: servicecatalog-portfolio-principal-association
Issue-Shape-Id: com.amazonaws.servicecatalog#AWS242ServiceCatalogService
Issue-Key: servicecatalog|ListPrincipalsForPortfolio|2xx->2xx|$.Principals[*].PrincipalARN
Issue-Commit: c0ce0118fd7c38fba0f2e9d7692bcc75f2ecf7c6

# ListPrincipalsForPortfolio returns the request ARN instead of the associated IAM principal identifier

> _Note: this issue was identified against the code base at commit `c0ce0118fd7c38fba0f2e9d7692bcc75f2ecf7c6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPrincipalsForPortfolio

## Description
One net-new wire divergence in the emulated AWS Service Catalog (sdkId servicecatalog, shape com.amazonaws.servicecatalog#AWS242ServiceCatalogService). The three other servicecatalog clusters in this replay are already owned by the open issue 20261029T191224Z-servicecatalog-service-catalog-portfolio-and-product-operations-diverge-from.md (AssociatePrincipalWithPortfolio message text, CreateProduct template validation, DescribeProductAsAdmin metadata/message); that issue does NOT cover this one — its Issue-Key list has no ListPrincipalsForPortfolio entry — so it is filed separately here.

Cause 1 of 1: ListPrincipalsForPortfolio projects the principal identifier of an association as the verbatim ARN string the AssociatePrincipalWithPortfolio request carried, where AWS projects the resolved identifier of the principal that was associated.

AWS: Smithy: ListPrincipalsForPortfolio takes a required PortfolioId (target Id, @length 1..100, pattern ^[a-zA-Z0-9_\-]*$) and returns ListPrincipalsForPortfolioOutput.Principals, a list of Principal; Principal.PrincipalARN is documented as "The ARN of the principal (user, role, or group). This field allows for an ARN with no accountID, with or without wildcard characters if the PrincipalType is an IAM_PATTERN." The model therefore constrains only the member's role, not the identifier form that is echoed back for a concrete (non-pattern) IAM principal. For that, recorded behavior only: after an IAM role is associated with a portfolio, the recorded 200 response from ListPrincipalsForPortfolio carries, in Principals[0].PrincipalARN, the IAM unique principal identifier of the associated role (an AROA-prefixed role id) rather than the role ARN that was passed to AssociatePrincipalWithPortfolio. The whole value is a variable part: it is the identifier of the role in the account/state under test, so the recording's own literal (AROABGXEQES5NRCCJJWAN) is a filled-in per-run IAM-generated id and is NOT part of the requirement. What is required is the identifier FORM and its source: the identifier of the principal as held by the identity service for that association, not a copy of the request string. Hypothesis (neither the model nor the recording states the mechanism): Service Catalog resolves a concrete IAM principal ARN to that principal's unique id when it stores the association, and serves the stored value back on list.

Emulator: for the same association, the emulator returns HTTP 200 with Principals[0].PrincipalARN set to the ARN string supplied in the association request (in this environment arn:aws:iam::<account>:role/<path>/<role-name>, with the emulator's dummy account id and the fixture's generated role name — both per-run values). The status, the error-free outcome, the list arity and every other member of the Principal match; only this member's value diverges, and it diverges in form, not merely in per-run content.

Why: a client that associates a principal and then reads it back cannot correlate the two sides the way it can against AWS. Code that joins ListPrincipalsForPortfolio output against IAM (DescribeRole/ListRoles unique ids, CloudTrail userIdentity.principalId, IAM policy conditions keyed on the principal id) finds no match, and code written against the emulator that assumes the value is re-usable as an ARN breaks on AWS, where it is not an ARN at all. Portfolio-access reconciliation tooling therefore reports principals as unknown or duplicated.

Fix: the response shaping of the ListPrincipalsForPortfolio handler plus the portfolio-principal association record in the servicecatalog service state it reads from — the association must store the resolved identifier of the principal at associate time (taken from the emulator's own identity state for a concrete IAM principal, preserving the request string for IAM_PATTERN principals, which the model explicitly allows to be a wildcard/accountless ARN) and project that stored identifier into Principal.PrincipalARN, instead of echoing the raw request ARN.

Traces: servicecatalog-portfolio-principal-association#task#4 (ListPrincipalsForPortfolio, expected 200 / actual 200, cluster affected_count 1).

## Evidence
Replay of trace set simparity-wire-aws-v1 (summary: emulator_up true, "/__admin/health answered 200 and the :39959 data listener accepts"; 10 fixtures replayed, 108 steps, 78 passed, 24 mismatched, 6 blocked, score_pct 72.2, bootstrap_signal false, divergence_count 4).

Net-new cluster (verdict=mismatch, stub_path empty, owned_by empty):
  fixture_id servicecatalog-portfolio-principal-association, step_key task#4, service servicecatalog, operation ListPrincipalsForPortfolio, expected_status 200, actual_status 200
  divergence_key: service=servicecatalog|ListPrincipalsForPortfolio|status=200->200|keys=$.Principals[0].PrincipalARN
  detail: $.Principals[0].PrincipalARN: value_mismatch (expected 'AROABGXEQES5NRCCJJWAN', got 'arn:aws:iam::000000000000:role/sim2real/sim2real-sc-principal-f1f9d40f-c718-4981-8d77-e42cba137580')
  affected: [servicecatalog-portfolio-principal-association#task#4], affected_count 1

Requirement vs per-run values: the expected side is an IAM unique role id and the actual side is a role ARN — the divergence is the identifier FORM/source. Both literals shown are per-run (the AROA... id is generated by the identity service for that run; the account id 000000000000 and the role name sim2real-sc-principal-<uuid> are generated by this environment), so neither literal is part of the requirement and a fix must not hard-code either.

Model evidence (services/service-com-amazonaws-servicecatalog-aws242servicecatalogservice/smithy-model/com-amazonaws-servicecatalog-aws242servicecatalogservice.json): ListPrincipalsForPortfolio input {AcceptLanguage, PortfolioId (required, Id), PageSize (default 0, range 0..20), PageToken}; output {Principals: list<Principal>, NextPageToken}; errors InvalidParametersException, ResourceNotFoundException (both client, httpError 400 defaulted); paginated {inputToken PageToken, outputToken NextPageToken, pageSize PageSize}. AssociatePrincipalWithPortfolio input {PortfolioId required, PrincipalARN required (@length 1..1000), PrincipalType required enum}. Principal.PrincipalARN doc quoted above (IAM_PATTERN allowance included).

The other three servicecatalog clusters in this replay (AssociatePrincipalWithPortfolio#task#5, CreateProduct product-lifecycle#task#4, DescribeProductAsAdmin product-lifecycle#task#6) reported owned_by the existing open servicecatalog issue and are deliberately not re-filed here.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout. servicecatalog is bootstrapped (service-com-amazonaws-servicecatalog-aws242servicecatalogservice is in get_bootstrap_status().bootstrapped; it is not in pending).
2. Replay fixture servicecatalog-portfolio-principal-association. Its earlier steps create an IAM role and a portfolio and call AssociatePrincipalWithPortfolio with PrincipalARN = that role's ARN and PrincipalType = IAM.
3. At step task#4 call ListPrincipalsForPortfolio with that PortfolioId and inspect Principals[0].PrincipalARN.
4. Observe HTTP 200 whose Principals[0].PrincipalARN is the ARN string sent in step 2 (arn:aws:iam::<account>:role/<path>/<role-name>), whereas the recording returns the associated role's IAM unique principal id (AROA-prefixed) for the same association.
5. Confirm a fix by re-replaying servicecatalog-portfolio-principal-association#task#4: the member must carry the resolved identifier of the associated principal as held by the emulator's identity state for that role (not a hard-coded literal, and not the request ARN), while an IAM_PATTERN association still reports the pattern ARN it was given.
6. Full sweep: replay_wire_corpus() reproduces this cluster as divergence_key service=servicecatalog|ListPrincipalsForPortfolio|status=200->200|keys=$.Principals[0].PrincipalARN.
