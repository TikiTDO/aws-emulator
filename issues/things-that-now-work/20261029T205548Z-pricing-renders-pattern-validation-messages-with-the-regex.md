Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pricing.list-price-lists-malformed-currency-pattern-invalid-parameter-rejected pricing.get-price-list-file-url-malformed-arn-invalid-parameter-rejected
Issue-Shape-Id: com.amazonaws.pricing#AWSPriceListService
Issue-Key: pricing.list-price-lists-malformed-currency-pattern-invalid-parameter-rejected|run#5|AssertionError|assert msg == (
Issue-Key: pricing.get-price-list-file-url-malformed-arn-invalid-parameter-rejected|run#5|AssertionError|assert msg == (
Issue-Commit: a04a180e27a1be47457c995aec938403d1945997

# pricing renders @pattern validation messages with the regex anchors (^...$) included, so every pattern-constraint refusal message diverges from AWS

> _Note: this issue was identified against the code base at commit `a04a180e27a1be47457c995aec938403d1945997`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPriceLists

## Description
One net-new root cause in the Pricing (AWS Price List Service) input-validation path, observed on two different operations and two different members. The emulator classifies pattern-constraint violations correctly (HTTP 400 / InvalidParameterException) and words the whole validation sentence correctly, but when it interpolates the offending regular expression into the message it emits the Smithy @pattern trait's raw value INCLUDING its `^` start and `$` end anchors. AWS strips the anchors before rendering the message. Every pattern-constraint refusal this service can produce therefore carries a message a client cannot match against AWS's.

Cause 1 of 1: the pattern-constraint validation message includes the regex anchors.

AWS: Smithy model com.amazonaws.pricing#AWSPriceListService (protocol awsJson1_1). ListPriceLists.CurrencyCode is required with @pattern "^[A-Z]{3}$"; GetPriceListFileUrl.PriceListArn is required with @length min 18 max 2048 and @pattern "^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}$". Both operations model InvalidParameterException (errorKind client, httpError 400). AWS's standard constraint-validation message for a single pattern violation is:
  `1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern-without-anchors>`
Its fixed text is `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. Its variable parts are: <value>, the rejected value taken verbatim from the request; <memberName>, the wire (lowerCamel) member name — `currencyCode`, `priceListArn`; and <pattern>, the member's @pattern value with the leading `^` and trailing `$` anchors REMOVED. So AWS's two messages are `... at 'currencyCode' ... pattern: [A-Z]{3}` and `... at 'priceListArn' ... pattern: arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}`.

Emulator: identical status (400), identical error code (InvalidParameterException), identical fixed text, correct value and correct member name — but it prints the anchored pattern. It answers `1 validation error detected: Value 'US' at 'currencyCode' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Z]{3}$` and `1 validation error detected: Value 'arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id' at 'priceListArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}$`. The extra `^` and `$` are the entire divergence. (The quoted values above are the ones the two checks happen to send; the requirement is the template, not those literals.)

Why: the error message is part of the contract. A client, test fixture or SDK-level assertion that compares the refusal message to AWS's — or that re-compiles the advertised pattern to echo the rule back to a user or to pre-validate the next attempt — gets a string that never equals AWS's for ANY pattern-constrained member of this service. Because the anchors are added by the shared renderer rather than by one handler, the divergence silently applies to every future pattern constraint added to the crate, not just these two members, and it is invisible to code that only checks the error code.

Fix: the shared input-validation message renderer in the pricing crate (the pattern-constraint arm that formats `Member must satisfy regular expression pattern: ...`), reached from the ListPriceLists and GetPriceListFileUrl handlers' input validation. It must emit the @pattern value with a leading `^` and a trailing `$` trimmed off, while the matching itself keeps using the anchored pattern. Fixing it per-handler would leave the renderer wrong for every other member.

Traces: pricing.list-price-lists-malformed-currency-pattern-invalid-parameter-rejected, pricing.get-price-list-file-url-malformed-arn-invalid-parameter-rejected.

Related but distinct open issues (neither covers this renderer): 20261029T203828Z-pricing-catalog-seed-diverges-from-aws-wrong-list.md (seed/pagination/price-list-file content) and 20261029T203828Z-pricing-refuses-400-notfoundexception-for-lookups-that-match.md (empty-result lookups refused instead of 200). This issue is about the text of a pattern-constraint refusal, on calls that are correctly refused.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator: 19/24 passed; pricing 19/22 (86%).

[mismatch] pricing.list-price-lists-malformed-currency-pattern-invalid-parameter-rejected
  Call: ListPriceLists(ServiceCode='AmazonEC2', EffectiveDate=<now>, CurrencyCode='US') against the us-east-1 pricing endpoint.
  Error code: InvalidParameterException in both (the code assertion passed).
  Expected message: "1 validation error detected: Value 'US' at 'currencyCode' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Z]{3}"
  Actual message:   "1 validation error detected: Value 'US' at 'currencyCode' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Z]{3}$"
  AssertionError raised at the message assertion (check_pricing_list_price_lists_malformed_currency_pattern_invalid_parameter_rejected.py line 49).
  The well-formed control call (CurrencyCode='USD', MaxResults=1) succeeded and returned a non-empty PriceLists, so the operation itself is serving.

[mismatch] pricing.get-price-list-file-url-malformed-arn-invalid-parameter-rejected
  Call: GetPriceListFileUrl(PriceListArn='arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id', FileFormat='json').
  Error code: InvalidParameterException in both (the code assertion passed).
  Expected message: "1 validation error detected: Value '<arn>' at 'priceListArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}"
  Actual message:   the same sentence with the pattern rendered as "^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}$"
  AssertionError raised at the message assertion (check_pricing_get_price_list_file_url_malformed_arn_invalid_parameter_rejected.py line 45).

The only difference in both cases is the two anchor characters, and the anchored text matches the @pattern traits read from the on-disk Smithy model verbatim — evidence the trait value is interpolated unprocessed.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Point a botocore 'pricing' client at the emulator with region='us-east-1' (the Price List Query API is only served from us-east-1, eu-central-1 and ap-south-1).
3. Cause 1, member currencyCode: call list_price_lists(ServiceCode='AmazonEC2', EffectiveDate=<any datetime>, CurrencyCode='US'). Catch the ClientError and read response['Error']['Code'] and ['Message']. Code is the expected InvalidParameterException; the message's trailing pattern is '^[A-Z]{3}$' where AWS prints '[A-Z]{3}'.
4. Cause 1, member priceListArn: call get_price_list_file_url(PriceListArn='arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id', FileFormat='json') — an ARN of legal length whose shape violates the price-list pattern. Catch the ClientError; the message's trailing pattern is '^arn:...{1,1023}$' where AWS prints it without the '^' and '$'.
5. Confirm the difference is only the anchors: strip a leading '^' and a trailing '$' from the emulator's rendered pattern and the two messages become identical to AWS's.
6. Equivalently, re-replay the behavioral suite and observe the two checks named in trace_ids are the only pricing message mismatches.
