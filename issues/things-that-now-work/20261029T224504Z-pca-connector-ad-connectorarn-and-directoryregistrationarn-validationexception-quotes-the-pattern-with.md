Issue-Source: trace-sweep-divergence
Issue-Trace: pca-connector-ad-connector-lifecycle-missing-prerequisites
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pca-connector-ad-connector-lifecycle-missing-prerequisites pca-connector-ad-service-principal-name-lifecycle pca-connector-ad-template-lifecycle
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad|GetConnector|4xx->4xx|$.message
Issue-Key: pca-connector-ad|DeleteConnector|4xx->4xx|$.message
Issue-Key: pca-connector-ad|CreateServicePrincipalName|4xx->4xx|$.message
Issue-Key: pca-connector-ad|GetServicePrincipalName|4xx->4xx|$.message
Issue-Key: pca-connector-ad|ListServicePrincipalNames|4xx->4xx|$.message
Issue-Key: pca-connector-ad|DeleteServicePrincipalName|4xx->4xx|$.message
Issue-Key: pca-connector-ad|ListTemplates|4xx->4xx|$.message
Issue-Commit: c2fc8676d63317349fbaa2ea39a789b7f43b5131

# connectorArn and directoryRegistrationArn ValidationException quotes the pattern with its ^ and $ anchors

> _Note: this issue was identified against the code base at commit `c2fc8676d63317349fbaa2ea39a789b7f43b5131`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnector

## Description
Seven net-new wire causes, one shared fault: wherever an operation rejects a malformed connector ARN or directory-registration ARN with the "N validation error(s) detected" ValidationException, the emulator echoes the member's @pattern EXACTLY as the Smithy model declares it, anchors included, while AWS advertises the same pattern with the leading `^` and trailing `$` removed. HTTP status (400), error code (ValidationException), the error count prefix, the member path and the rejected value are all correct in every cause; the message differs by exactly those two characters.

Shared AWS requirement for all seven causes: HTTP 400, code ValidationException (Smithy: ValidationException @httpError 400 on every one of these operations, Message @required; doc: "An input validation error occurred..."), message template `N validation error(s) detected: Value '<rejected value>' at '<member path, lowerCamel>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern with anchors stripped>` — with `; ` joining the clauses when more than one member fails and the count word agreeing in number. Fixed text: `validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. Variable parts: the error count, the rejected value and the member path (both from the request) and the modeled pattern minus its anchors. The anchor-stripping itself is recorded behavior only; the pattern bodies are the model's.

Shared emulator behavior: the service's constraint-violation message formatter interpolates the pattern string straight out of the compiled schema, so the rendered message always carries `^...$`.

Shared consequence (Why): any client that compares the refusal message, extracts the advertised pattern to show a user, or maps error text onto its own diagnostics sees a string AWS never emits for a @pattern violation on these two ARN shapes.

Shared Fix: the anchor-stripping step of the @pattern-constraint message renderer in the pca-connector-ad validation layer (the layer that builds `... Member must satisfy regular expression pattern: ...` before `handle` runs); not the individual handlers.

Cause 1 of 7: GetConnector with ConnectorArn='not-a-valid-arn'. AWS: pattern reported as `arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}` (Smithy: GetConnector.ConnectorArn @httpLabel @required @length 5..200 @pattern `^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$`). Emulator: same message with the anchors retained. Traces: fixture pca-connector-ad-connector-lifecycle-missing-prerequisites, step task#3.

Cause 2 of 7: DeleteConnector, same member and same divergence. Smithy: DeleteConnector.ConnectorArn @httpLabel @required, same @pattern. Traces: fixture pca-connector-ad-connector-lifecycle-missing-prerequisites, step task#4.

Cause 3 of 7: CreateServicePrincipalName with both ARNs malformed — two clauses in one message, both carrying anchors. AWS: `2 validation errors detected: Value '<v>' at 'directoryRegistrationArn' ... pattern: arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:directory-registration\/d-[0-9a-f]{10}; Value '<v>' at 'connectorArn' ... pattern: arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}`. Smithy: CreateServicePrincipalName.DirectoryRegistrationArn and .ConnectorArn, both @httpLabel @required with those anchored patterns. Traces: fixture pca-connector-ad-service-principal-name-lifecycle, step task#1.

Cause 4 of 7: GetServicePrincipalName, same two members, same two-clause divergence. Traces: fixture pca-connector-ad-service-principal-name-lifecycle, step task#2.

Cause 5 of 7: ListServicePrincipalNames with a malformed directoryRegistrationArn — single clause, directory-registration pattern. Traces: fixture pca-connector-ad-service-principal-name-lifecycle, step task#3.

Cause 6 of 7: DeleteServicePrincipalName, same two members as Cause 3. Traces: fixture pca-connector-ad-service-principal-name-lifecycle, step task#4.

Cause 7 of 7: ListTemplates with a malformed connectorArn — single clause, connector pattern. Smithy: ListTemplates.ConnectorArn @httpQuery @required with the connector @pattern. Traces: fixture pca-connector-ad-template-lifecycle, step task#2.

Related open issue (does NOT cover these wire steps): 20261029T190840Z-pca-connector-ad-diverges-on-seven-counts-validationexception.md, whose Cause 2 reports the same anchor fault from the BEHAVIORAL corpus for CreateServicePrincipalName and ListConnectors.NextToken only; it carries no wire replay key, and five of the seven operations above are not named by it at all.

## Evidence
All seven steps: expected status 400 == actual status 400; expected code ValidationException == actual; only $.message diverges, by the two anchor characters.

Cause 1 (pca-connector-ad-connector-lifecycle-missing-prerequisites#task#3, GetConnector):
  expected $.message: "1 validation error detected: Value 'not-a-valid-arn' at 'connectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}"
  actual   $.message: same text ending "... pattern: ^arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$"

Cause 2 (…#task#4, DeleteConnector): identical expected/actual pair.

Cause 3 (pca-connector-ad-service-principal-name-lifecycle#task#1, CreateServicePrincipalName):
  expected: "2 validation errors detected: Value 'not-a-valid-arn' at 'directoryRegistrationArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:directory-registration\\/d-[0-9a-f]{10}; Value 'not-a-valid-arn' at 'connectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}"
  actual:   both patterns rendered as `^...$`.

Cause 4 (…#task#2, GetServicePrincipalName): identical two-clause pair.
Cause 6 (…#task#4, DeleteServicePrincipalName): identical two-clause pair.

Cause 5 (…#task#3, ListServicePrincipalNames):
  expected tail: "... pattern: arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:directory-registration\\/d-[0-9a-f]{10}"
  actual tail:   "... pattern: ^arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:directory-registration\\/d-[0-9a-f]{10}$"

Cause 7 (pca-connector-ad-template-lifecycle#task#2, ListTemplates):
  expected tail: "... pattern: arn:[\\w-]+:pca-connector-ad:[\\w-]+:[0-9]+:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}"
  actual tail:   the same with ^ and $.

## Steps to Reproduce
Boot the emulator and replay the wire corpus simparity-wire-aws-v1, or drive it directly with an SDK client (region us-west-2, client-side validation disabled):
1. GetConnector(ConnectorArn='not-a-valid-arn') -> 400 ValidationException; inspect the message tail: the quoted pattern begins with ^ and ends with $. (pca-connector-ad-connector-lifecycle-missing-prerequisites#task#3)
2. DeleteConnector(ConnectorArn='not-a-valid-arn') -> same. (…#task#4)
3. CreateServicePrincipalName(DirectoryRegistrationArn='not-a-valid-arn', ConnectorArn='not-a-valid-arn') -> 400 with two clauses, both anchored. (pca-connector-ad-service-principal-name-lifecycle#task#1)
4. GetServicePrincipalName(same two args) -> same. (…#task#2)
5. ListServicePrincipalNames(DirectoryRegistrationArn='not-a-valid-arn') -> single anchored clause. (…#task#3)
6. DeleteServicePrincipalName(same two args as 3) -> same as 3. (…#task#4)
7. ListTemplates(ConnectorArn='not-a-valid-arn') -> single anchored clause. (pca-connector-ad-template-lifecycle#task#2)
In every case AWS emits the identical message with the leading ^ and trailing $ removed from the quoted pattern.
