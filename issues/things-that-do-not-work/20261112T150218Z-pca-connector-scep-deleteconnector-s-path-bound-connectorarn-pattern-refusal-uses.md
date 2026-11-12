Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-scep.delete-connector-twice-rejected
Issue-Shape-Id: com.amazonaws.pcaconnectorscep#PcaConnectorScep
Issue-Key: pca-connector-scep.delete-connector-twice-rejected|run#15|AssertionError|assert bogus_error.get("Message") == (
Issue-Commit: b674738a58d9d62ba939f6eca5e8d4760a2144d8

# DeleteConnector's path-bound ConnectorArn pattern refusal uses the body-member ECMA-262 message form instead of AWS's "1 validation error detected" sentence

> _Note: this issue was identified against the code base at commit `b674738a58d9d62ba939f6eca5e8d4760a2144d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConnector

## Description
Behavioral replay of suite simparity-behavioral-aws-v1 against the pca-connector-scep emulator failed 5 of 21 checks; four of them are already owned by open issue 20261029T213350Z-pca-connector-scep-7-net-new-causes-createconnector.md and are excluded here. One net-new root cause survives, in pca-connector-scep. A fixer must close 1 cause before submitting.

Cause 1 of 1: a @pattern violation on an httpLabel (path-bound) member of DeleteConnector is reported with the BODY-member message form, so the whole message text is the wrong template.
  AWS: DELETE /connectors/{ConnectorArn} with a path value that violates the member's @pattern is refused 400 ValidationException, and for a path/query-bound member the message is the sentence form: "1 validation error detected: Value '<value>' at '<lowerCamelMemberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where <pattern> is the member's modeled regex with its leading ^ and trailing $ removed. The fixed text is the entire sentence scaffold ("1 validation error detected: Value ", " at ", " failed to satisfy constraint: Member must satisfy regular expression pattern: "); the variable parts are the rejected value taken verbatim from the request URI, the lowerCamel member name (here 'connectorArn'), and the anchor-stripped pattern. Smithy (services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/smithy-model/com-amazonaws-pcaconnectorscep-pcaconnectorscep.json): DeleteConnector is DELETE /connectors/{ConnectorArn}, success code 202, and models ValidationException (errorKind=client, httpError=400); DeleteConnectorRequest.ConnectorArn is httpLabel, required, @length 5..200, @pattern ^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\\d*:\\d{12}:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$ — a plain Connector ARN, so a Connector ARN carrying an extra /challenge/<uuid> segment must be refused by input validation and not treated as a lookup miss. Which of AWS's two validation-message forms applies to which HTTP binding is recorded behavior only; the status (400) and code (ValidationException) are as recorded.
  Emulator: the status (400) and the error code (ValidationException) are correct, but Message is the body-member form '[ECMA 262 regex "<pattern-with-anchors>" does not match input string "<value>"]' — a structurally different template that quotes the regex (anchors included) instead of naming the offending member and reporting the anchor-stripped pattern. Observed on DeleteConnector with a path ARN of the shape <connectorArn>/challenge/<uuid>.
  Why: a client that parses or echoes the validation message — to tell the user which request member was rejected, to drive branch logic on the member name, or to compare the server-supplied pattern against the documented constraint — gets no member name at all and a pattern that still carries ^ and $. Code validated against the emulator's text diverges from real AWS on every path-bound ARN refusal.
  Fix: the shared request-validation message builder for this service, in the layer that selects the message form by the member's HTTP binding: an httpLabel (and httpQuery) member must render the "N validation error detected: Value ... at '<member>' ... pattern: <pattern>" sentence with a leading ^ and trailing $ stripped from the modeled pattern, while only BODY members use the ECMA-262 form. The DeleteConnector handler's own refusal path is otherwise correct (status, code, and the surrounding idempotency behavior all pass).
  Related, and NOT a substitute: open issue 20261029T190804Z-pca-connector-scep-pattern-constraint-validationexception-messages-use-the-wrong-form.md describes the binding-aware form selection from wire fixtures, and its cause 4 asserts that an httpLabel member keeps the sentence form (its reported defect there was only the retained ^/$ anchors). The emulator now emits the ECMA-262 form for path members instead, which that issue does not describe, and it carries no replay key for this behavioral check. Open issue 20261029T213350Z-pca-connector-scep-7-net-new-causes-createconnector.md (cause 5) likewise describes only the anchor leak inside the correct sentence, for GetChallengeMetadata.
  Traces: pca-connector-scep.delete-connector-twice-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 16/21 passed; pca-connector-scep 16/21 (76%), 5 failed, of which 4 are marked OWNED by 20261029T213350Z-pca-connector-scep-7-net-new-causes-createconnector.md.

[mismatch] pca-connector-scep.delete-connector-twice-rejected — the check creates and activates an acm-pca ROOT CA, creates a connector, waits for a terminal status, deletes it (succeeds), confirms GetConnector answers 404 ResourceNotFoundException with Message "Connector was not found: <connectorArn>." (this now passes), confirms a second DeleteConnector succeeds (passes), then calls DeleteConnector with <connectorArn> + "/challenge/<uuid>".

Expected: Code 'ValidationException' (which matched) and Message "1 validation error detected: Value '<challenge_shaped_arn>' at 'connectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\\d*:\\d{12}:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}" (pattern with ^ and $ removed).

Got: {'Code': 'ValidationException', 'Message': '[ECMA 262 regex "^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\\d*:\\d{12}:connector\\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$" does not match input string "arn:aws:pca-connector-scep:us-west-2:000000000000:connector/46cdf3d1-a3c6-4258-bcdf-3c210ef52a60/challenge/8b07633e-303a-4683-83d5-ee74199a2d9b"]'}

Fixed text of the required form: "1 validation error detected: Value " + <value> + " at " + <'member'> + " failed to satisfy constraint: Member must satisfy regular expression pattern: " + <anchor-stripped pattern>. The region us-west-2, the account 000000000000 and the two UUIDs above are this run's values filling the template, not requirements.

## Steps to Reproduce
Boot the emulator and replay the pca-connector-scep check pca-connector-scep.delete-connector-twice-rejected of suite simparity-behavioral-aws-v1.

1. Create an acm-pca ROOT CA and activate it (get CSR, IssueCertificate with the RootCACertificate/V1 template, ImportCertificateAuthorityCertificate, wait for ACTIVE).
2. CreateConnector against that CA and wait for the connector to leave CREATING; note <connectorArn>.
3. DeleteConnector(<connectorArn>) — succeeds. GetConnector(<connectorArn>) must answer 404 ResourceNotFoundException "Connector was not found: <connectorArn>." and a second DeleteConnector(<connectorArn>) must succeed.
4. Call DeleteConnector with ConnectorArn = <connectorArn> + "/challenge/<random-uuid>" (a Connector ARN with a trailing challenge segment, violating the httpLabel @pattern).
   Expect: 400 ValidationException with Message "1 validation error detected: Value '<that value>' at 'connectorArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <the modeled ConnectorArn pattern with its leading ^ and trailing $ removed>".
   Observe: 400 ValidationException with Message '[ECMA 262 regex "<pattern with ^ and $>" does not match input string "<that value>"]'.
5. Cross-check that a BODY-bound pattern member (e.g. CreateChallenge's ConnectorArn) still uses the ECMA-262 form — only path/query-bound members must switch to the sentence form.
