Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-domain-and-space-singleton-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-domain-and-space-singleton-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|CreateDomain|4xx->2xx|$.__type
Issue-Key: cloudwatchomni|CreateDomain|4xx->2xx|$.domain.customEndpointUrls[*]
Issue-Key: cloudwatchomni|CreateDomain|4xx->2xx|$.domain.domainEndpointUrl
Issue-Key: cloudwatchomni|CreateDomain|4xx->2xx|$.message
Issue-Key: cloudwatchomni|CreateDomain|4xx->2xx|status
Issue-Key: cloudwatchomni|CreateDomain|2xx->4xx|$.domain
Issue-Key: cloudwatchomni|CreateDomain|2xx->4xx|status
Issue-Commit: 826b08334102dd43451716f9b2396fd89c307280

# cloudwatchomni CreateDomain ignores clientToken requirement and account singleton conflict

> _Note: this issue was identified against the code base at commit `826b08334102dd43451716f9b2396fd89c307280`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
CreateDomain enforces neither of the two admission rules the corpus records: it creates a domain for a request with no clientToken (which AWS rejects), and it then creates a second domain for an account that already has one (which AWS rejects as a conflict). Because the first unrequested domain is admitted, the legitimate create that follows is refused. This bundle has THREE causes; all three must be fixed and re-replayed. The open issue 20261111T022337Z-cloudwatchomni-account-region-singleton-rules-unenforced-and-name.md describes related singleton gaps from the static pass but does not carry these replay keys, so these steps are not covered by it.

Cause 1 of 3 - CreateDomain without clientToken is accepted (fixture cloudwatchomni-domain-and-space-singleton-lifecycle, step task#1).
AWS: Smithy CreateDomain input clientToken is target ClientToken with @idempotencyToken (@length 1..64, @pattern ^[!-~]+$); modeled errors include ValidationException (httpError 400). Recorded behavior: a CreateDomain request that omits clientToken is refused with HTTP 400, __type 'com.amazon.cloudwatchomnifrontend#ValidationException' and message 'A clientToken is required for CreateDomain'. That message is fixed text apart from the operation name 'CreateDomain'.
Emulator: answers HTTP 200 with a fully created $.domain (no __type, no message), i.e. it treats the token as optional and mints a domain.
Why: a client that relies on the service to reject a non-idempotent create gets an unintended domain; since a domain is an account singleton, that stray domain blocks every later create (Cause 2) and the account is left in a state the client never asked for.
Fix: the CreateDomain handler's input validation in the cloudwatchomni service crate - require clientToken and raise ValidationException (400) before any state mutation.
Traces: cloudwatchomni-domain-and-space-singleton-lifecycle#task#1.

Cause 2 of 3 - the legitimate CreateDomain is refused 409 (fixture cloudwatchomni-domain-and-space-singleton-lifecycle, step task#2).
AWS: the same fixture's next step sends CreateDomain WITH a clientToken and AWS answers 200 with the domain (status ACTIVE, identityProviders ['IAM'], region = the request's Region, name = the requested name, domainEndpointUrl/customEndpointUrls per the Domain shape). This is the downstream consequence of Cause 1: with the previous request correctly rejected, no domain exists yet, so this create succeeds.
Emulator: answers HTTP 409 with no $.domain, because the domain it wrongly created in Cause 1 already occupies the account singleton.
Why: the client's only valid create fails, so the whole domain-dependent flow (spaces, dashboards, deep links) cannot start.
Fix: follows from Cause 1 (reject the token-less create without mutating state); verify this step returns 200 once Cause 1 is fixed.
Traces: cloudwatchomni-domain-and-space-singleton-lifecycle#task#2.

Cause 3 of 3 - a second CreateDomain for the same account is accepted instead of ConflictException (fixture cloudwatchomni-domain-and-space-singleton-lifecycle, step task#3).
AWS: Smithy CreateDomain errors include ConflictException (httpError 409); the operation doc notes the account-scoped domain. Recorded behavior: once the account has a domain, a further CreateDomain (a different name, valid clientToken) is refused with HTTP 409, __type 'com.amazon.cloudwatchomnifrontend#ConflictException' and message 'A domain already exists for this account' - fixed text, no variable parts.
Emulator: answers HTTP 200 and creates a second domain (returns a new domainId and endpoint URLs), so the account-singleton rule is never enforced.
Why: a client can end up with multiple account domains, a state AWS cannot produce; code that assumes ListDomains yields at most one account-scoped domain, or that retries a create expecting a 409 to detect existing setup, takes the wrong branch and duplicates resources.
Fix: the CreateDomain handler's conflict check against the account-scoped domain in the cloudwatchomni state store, raising ConflictException (409) with the message above.
Traces: cloudwatchomni-domain-and-space-singleton-lifecycle#task#3.

## Evidence
Cause 1 - step task#1: expected 400, got 200. Expected body $.__type 'com.amazon.cloudwatchomnifrontend#ValidationException', $.message 'A clientToken is required for CreateDomain'; emulator returned neither and instead a created $.domain.
Cause 2 - step task#2: expected 200, got 409. Expected $.domain {name: <requested name>, region: us-west-2, status: ACTIVE, identityProviders: [IAM], identityProviderConfiguration: {}, domainId/domainArn/domainEndpointUrl/createdAt/updatedAt shaped}; emulator returned no $.domain.
Cause 3 - step task#3: expected 409, got 200. Expected $.__type 'com.amazon.cloudwatchomnifrontend#ConflictException', $.message 'A domain already exists for this account'; emulator returned 200 with a second created domain.

## Steps to Reproduce
Replay simparity-wire-aws-v1 fixture cloudwatchomni-domain-and-space-singleton-lifecycle. task#1: CreateDomain with a valid name and identityProviders but NO clientToken -> emulator 200 instead of 400 ValidationException. task#2: CreateDomain with a clientToken -> emulator 409 instead of 200. task#3: CreateDomain again with a different name -> emulator 200 instead of 409 ConflictException.
