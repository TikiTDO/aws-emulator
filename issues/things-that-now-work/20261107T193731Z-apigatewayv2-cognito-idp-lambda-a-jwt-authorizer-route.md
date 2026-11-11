Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered|_poll_http#17|AssertionError|raise AssertionError(
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigatewayv2__cognito-idp__lambda: a JWT-authorizer route refuses a token that DOES carry the required scope — 401 Unauthorized instead of serving the route

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
execute-api GET through a JWT authorizer (CreateAuthorizer AuthorizerType=JWT, AuthorizationScopes)

## Description
Cause 1 of 1: the JWT authorizer's scope check rejects a request whose access token holds the route's required scope.
AWS: an HTTP API route with AuthorizationType=JWT and AuthorizationScopes serves the request when the presented access token's 'scope' claim contains at least one of the route's scopes (the integration runs and its 200 is returned); only a token missing every required scope is refused, with 403 and body {"message":"Forbidden"}, while a missing/invalid token is 401 with body {"message":"Unauthorized"}.
Emulator: the positive leg — GET /admin-scope presenting the token that holds that scope — was answered 401 with body {"message":"Unauthorized"} on every attempt for 150 s, so the route never served and the scope-mismatch contract cannot be distinguished from a blanket refusal. Fixed text of the diverging response: the 401 status and the message 'Unauthorized'; the route path, scope string and token are request values.
Why: any client authenticating with a Cognito access token is locked out of its own API — the authorizer refuses valid credentials, and the 401-vs-403 distinction AWS draws (unauthenticated vs insufficient scope) is lost.
Fix: the execute-api data plane's JWT authorizer evaluation — token validation and the AuthorizationScopes intersection against the token's scope claim — and its refusal mapping (401 for no/invalid token, 403 for insufficient scope). Related open issues that do not carry this check's replay key: 20261029T182116Z-apigatewayv2-cognito-idp-lambda-an-http-api-route-2f3eea.md, 20261025T084658Z-apigatewayv2-cognito-idp-lambda-all-three-jwt-authorizer--r2.md.
Traces: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered.

## Evidence
"GET /admin-scope with the held scope https://<host>/_aws/execute-api/<api-id>/$default/admin-scope never reached an accepted status within 150.0s. Last: 401 '{\"message\":\"Unauthorized\"}'" — expected 200 from the Lambda integration for the token that carries the scope.

## Steps to Reproduce
Replay apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered: create a Cognito user pool + resource server scopes, an HTTP API with a JWT authorizer on that pool, two routes with different AuthorizationScopes and a Lambda integration; obtain an access token holding one scope and GET the route requiring that scope (expect 200), then GET the route requiring the other scope (expect 403).
