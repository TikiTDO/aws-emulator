Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.id-token-verifies-rs256-against-the-issuer-jwks cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned cognito-idp.op-contract-admin-respond-to-auth-challenge
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.id-token-verifies-rs256-against-the-issuer-jwks|_assert_issuer#15|AssertionError|assert (parts is not None and parts.scheme == "https"
Issue-Key: cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned|run#28|AssertionError|assert read_pair == ("NotAuthorizedException", refusal_message), (
Issue-Key: cognito-idp.op-contract-admin-respond-to-auth-challenge|run#11|AssertionError|assert code == "CodeMismatchException", f"expected CodeMismatchException for bogus session, got {code}"
Issue-Commit: d0a04fabcd83cf2b721ac740047fd92b0e777ff8

# cognito-idp: 3 net-new causes — the ID token's iss claim is the bare emulator endpoint, signature-verification failures are reported as "Invalid Access Token", and AdminRespondToAuthChallenge answers NotAuthorizedException for an unparseable Session

> _Note: this issue was identified against the code base at commit `d0a04fabcd83cf2b721ac740047fd92b0e777ff8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminRespondToAuthChallenge

## Description
Three net-new root causes in the cognito-idp token and challenge planes.

Cause 1 of 3: the iss claim of an issued ID token has no cognito-idp.<region> host.
AWS: recorded behavior only (the JWT body is not Smithy-modelled). The iss claim of a Cognito ID token is https://cognito-idp.<region>.<domain>/<user pool id>. Its fixed shape is: scheme https, a host whose first two labels are the literal 'cognito-idp' and the pool's region, at least one further domain label, and a path of exactly one segment equal to the pool id; the <domain>, host suffix and port are environment values, the 'cognito-idp.<region>' prefix and the pool-id path are the requirement.
Emulator: iss is https://<emulator host>:<port>/<pool id> — the bare listener address, with no cognito-idp.<region> labels at all.
Why: every OIDC/JWT verifier derives the JWKS URI and performs the issuer check from iss, and multi-provider code distinguishes identity providers by it. An issuer that is just the endpoint it was reached on fails issuer validation and cannot be told apart from any other provider served on the same host.
Fix: the cognito-idp issuer computation shared by the token-minting path and the OpenID discovery/JWKS documents (the per-pool issuer URL it stores and serves). NOTE: Cause 2 of the open issue 20261109T124858Z-cognito-idp-5-net-new-causes-tokens-are.md describes this same issuer defect, but it does not carry this check's replay key — that issue's key for this check is the alg==RS256 assertion, which now passes, so the check fails further on at the iss assertion and is unowned. Fold this in there if that cause is still open.
Traces: cognito-idp.id-token-verifies-rs256-against-the-issuer-jwks.

Cause 2 of 3: a token whose RS256 signature does not verify is refused with the wrong message.
AWS: Smithy: GetUser declares NotAuthorizedException. Cognito distinguishes two refusal messages on the access-token path — a token it cannot parse/accept at all is refused NotAuthorizedException 'Invalid Access Token', while a well-formed RS256 token whose signature does not verify against the pool's signing key (payload tampered with under the original signature, or a flipped signature byte) is refused NotAuthorizedException 'Could not verify signature for Access Token'. Both messages are fixed text with no variable part.
Emulator: both signature-verification forgeries are refused NotAuthorizedException with message 'Invalid Access Token' (observed payload: {'Error': {'Code': 'NotAuthorizedException', 'Message': 'Invalid Access Token'}}), i.e. the parse-failure message is used for a verification failure.
Why: the message is the only signal distinguishing 'this token is garbage/expired, get a new one' from 'this token was tampered with or signed by the wrong key'. Security tooling and client retry logic that key on 'Could not verify signature for Access Token' to flag forgery never see it, so token-forgery attempts are indistinguishable from ordinary token expiry in the emulator's responses.
Fix: the cognito-idp access-token validation used by GetUser (and the other access-token-bearing operations) — the signature-verification branch must emit its own message rather than falling through to the parse-failure one.
Traces: cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned.

Cause 3 of 3: AdminRespondToAuthChallenge classifies an unparseable Session as an authorization failure.
AWS: Smithy: AdminRespondToAuthChallenge declares both CodeMismatchException (httpError 400) and NotAuthorizedException (httpError 403); Session is a SessionType of length 20-4096. A Session value that is not a session this pool issued is treated as a code/response mismatch against the challenge, so the call is refused CodeMismatchException with message 'Invalid code/session provided' — fixed text, no variable part.
Emulator: the same call is refused NotAuthorizedException.
Why: the two errors are handled differently by clients — CodeMismatchException means 're-prompt the user / restart the challenge', NotAuthorizedException means 'the credentials or the principal are not allowed'. A client branching on the error code aborts the sign-in flow (or signs the user out) where AWS would have it retry the challenge, and the modelled CodeMismatchException path of this operation is never reachable.
Fix: the AdminRespondToAuthChallenge handler's Session lookup/validation in the cognito-idp service crate (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/) — an unresolvable Session must raise the modelled CodeMismatchException with that message, not NotAuthorizedException.
Traces: cognito-idp.op-contract-admin-respond-to-auth-challenge.

## Evidence
Cause 1 — AssertionError: "the ID token must be issued by https://cognito-idp.us-west-2.<domain>/<pool id>, got iss='https://127.0.0.1:44565/us-west-2_84ee59bf5'". Expected: an iss whose host starts with the labels ['cognito-idp', <region>] plus at least one domain label, and whose path is /<poolId>. Actual: the emulator's own host:port with the pool id as the path.

Cause 2 — AssertionError: "GetUser must refuse a cognito:groups claim injected under the original signature with NotAuthorizedException, got ('NotAuthorizedException', 'Invalid Access Token')"; full payload {'Error': {'Message': 'Invalid Access Token', 'Code': 'NotAuthorizedException'}, 'ResponseMetadata': {'HTTPStatusCode': 400, ...}}. Expected: NotAuthorizedException 'Could not verify signature for Access Token' for both the tampered-payload token and the flipped-signature-byte token (the alg:none variant correctly gets 'Invalid Access Token'). Actual: 'Invalid Access Token' for all three.

Cause 3 — AssertionError: "expected CodeMismatchException for bogus session, got NotAuthorizedException". Expected: CodeMismatchException (HTTP 400) with message 'Invalid code/session provided'. Actual: NotAuthorizedException.

## Steps to Reproduce
Cause 1: CreateUserPool + CreateUserPoolClient, create and confirm a user, InitiateAuth/AdminInitiateAuth with USER_PASSWORD_AUTH, base64url-decode the IdToken payload and read iss — it is https://<host>:<port>/<poolId> instead of https://cognito-idp.<region>.<domain>/<poolId>.

Cause 2: with a genuine AccessToken from the same sign-in, build two forgeries: (a) re-encode the payload with an extra "cognito:groups" claim while keeping the original header and signature, (b) flip one byte of the signature. Call GetUser with each — both answer NotAuthorizedException 'Invalid Access Token' instead of NotAuthorizedException 'Could not verify signature for Access Token'.

Cause 3: AdminCreateUser with a TemporaryPassword and MessageAction=SUPPRESS; AdminInitiateAuth with ADMIN_USER_PASSWORD_AUTH returns ChallengeName=NEW_PASSWORD_REQUIRED and a Session. Call AdminRespondToAuthChallenge with ChallengeName=NEW_PASSWORD_REQUIRED, ChallengeResponses={USERNAME, NEW_PASSWORD} and Session="not-a-real-session-token" — the refusal is NotAuthorizedException instead of CodeMismatchException 'Invalid code/session provided'.
