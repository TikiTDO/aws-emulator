Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__cognito-idp.obs-cognito-idp-2-auth-counters-per-client-and-plus-tier-no-risk
Issue-Key: cloudwatch__cognito-idp.obs-cognito-idp-2-auth-counters-per-client-and-plus-tier-no-risk|_poll_all_until_expected#37|Timeout|time.sleep(poll_s)
Issue-Commit: 004a13bc6b62a404600c0a09686b1d7dd87c7440

# cloudwatch__cognito-idp: Cognito user-pool traffic still publishes no AWS/Cognito CloudWatch metrics, so the per-app-client auth counters and the PLUS-tier NoRisk series never appear and the check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `004a13bc6b62a404600c0a09686b1d7dd87c7440`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One root cause. DUPLICATE-RISK NOTE FOR THE FIXER, up front: open issue 20261027T032545Z-cloudwatch-cognito-idp-cognito-publishes-no-aws-cognito.md already lists this exact check id in its Issue-Trace-Ids and describes the same missing emission path. The suite did NOT mark this failure OWNED, because that issue's recorded replay key names a harness frame (`_poll_all_until_stable#36`) the current check no longer has — the check now settles through `_poll_all_until_expected` and is killed by the harness cap before producing any frame at all, so the key cannot join. Treat this as the same work item as that issue if it is still open when you pick it up; it is filed here only because the replay key no longer binds and the failure would otherwise go unrecorded for this run.

=== Cause 1 of 1: cognito-idp has no CloudWatch metric-emission path, so no AWS/Cognito series is ever created by user-pool traffic ===

AWS:      recorded behavior only — the CloudWatch metric catalogue is not part of the cognito-idp Smithy model, and CloudWatch's own GetMetricStatistics/ListMetrics shapes say nothing about which series a user pool creates. What the recording shows: a user pool publishes, per app client, namespace AWS/Cognito series SignInSuccesses, TokenRefreshSuccesses and SignUpSuccesses keyed on the two dimensions UserPool (the pool id) and UserPoolClient (the app-client id). SignInSuccesses counts OUTCOMES, not just successes: a refused InitiateAuth is published as a zero-valued sample, so Sum counts successes while SampleCount counts every attempt (a client taking N good and M refused sign-ins settles at Sum N / SampleCount N+M, Average N/(N+M)). REFRESH_TOKEN_AUTH does not touch SignInSuccesses and lands in TokenRefreshSuccesses; SignUp contributes to SignUpSuccesses and AdminConfirmSignUp adds nothing. An app client on the same pool that is never used has NO datapoints in any of the three series. Separately, on a PLUS-tier pool with threat protection in AUDIT mode, a successful sign-in publishes the AWS/Cognito metric NoRisk keyed on Operation (value SignIn) and UserPoolId — note UserPoolId, a DIFFERENT dimension key from the UserPool key the authentication counters use; the same query keyed UserPool returns nothing, because CloudWatch dimension matching is exact. ListMetrics catalogues both dimension sets.

Emulator: every cognito-idp call in the scenario answers normally — CreateUserPool (including UserPoolTier=PLUS and UserPoolAddOns.AdvancedSecurityMode=AUDIT, both echoed back by DescribeUserPool), CreateUserPoolClient, AdminCreateUser/AdminSetUserPassword, the successful InitiateAuth calls, the wrong-password refusals (which the check asserts land as NotAuthorizedException "Incorrect username or password."), the REFRESH_TOKEN_AUTH refreshes, SignUp/AdminConfirmSignUp and the PLUS-pool sign-in. Nothing fails until the CloudWatch read. GetMetricStatistics for Namespace=AWS/Cognito then returns an empty Datapoints list for every series the traffic should have created, so the check's joint settle loop (`_poll_all_until_expected`, per-series budget 900 s, 2 s poll) sees None on every probe, never terminates, and the harness kills the check at its 60 s cap. Hypothesis: the emulator has no cognito-idp-to-CloudWatch emission path at all — not merely wrong values — which is consistent with the loop never seeing a single non-empty reading and with the earlier filing reaching the same conclusion.

Why:      a client that observes authentication health through these counters — a dashboard, an alarm on a drop in SignInSuccesses Sum, or a failure rate computed as 1 - Sum/SampleCount — sees a permanently empty series and cannot distinguish "no traffic" from "all traffic failing"; any alarm built on them stays INSUFFICIENT_DATA forever. The zero-valued-sample convention, which is the only thing that makes the failure rate computable, cannot be relied on. For threat protection, an empty namespace makes AUDIT mode indistinguishable from threat protection being off, and because ListMetrics catalogues nothing, a client has no way to discover that the risk series is keyed UserPoolId while the auth series are keyed UserPool — so it silently queries a series that will never hold data.

Fix:      the metric-emission side effect belongs in the cognito-idp operation handlers that terminate an authentication or registration attempt — InitiateAuth/AdminInitiateAuth for SignInSuccesses and TokenRefreshSuccesses, SignUp for SignUpSuccesses — which must publish one datapoint per attempt (value 1 on success, 0 on a refusal, including a refusal raised by request validation before any credential is checked) into the emulator's CloudWatch metric state under namespace AWS/Cognito with dimensions UserPool and UserPoolClient; and in the threat-protection evaluation step of the sign-in path for a PLUS-tier pool, which must publish the risk-outcome datapoint under dimensions Operation and UserPoolId. Both series must be registered so ListMetrics catalogues them. The CloudWatch side needs no change: this run's cloudwatch metric-store checks for PutMetricData-written series pass.

Traces:   cloudwatch__cognito-idp.obs-cognito-idp-2-auth-counters-per-client-and-plus-tier-no-risk

## Evidence
cloudwatch__cognito-idp.obs-cognito-idp-2-auth-counters-per-client-and-plus-tier-no-risk — verdict [mismatch], "Error: Check timed out after 60.0s". Service pair pass rate this run: cloudwatch__cognito-idp 0/1.

What the check does before the read (all of it completes without error, per the absence of any assertion failure): creates a pool with app clients A and B plus a CONFIRMED user; drives 2 successful and 3 wrong-password USER_PASSWORD_AUTH InitiateAuth calls on client A (asserting each refusal is exactly ("NotAuthorizedException", "Incorrect username or password.")), 2 REFRESH_TOKEN_AUTH refreshes, and 1 SignUp + AdminConfirmSignUp; then creates a second pool with UserPoolTier=PLUS and UserPoolAddOns={AdvancedSecurityMode: AUDIT}, asserts DescribeUserPool echoes both, and takes one successful sign-in on it.

The graded read, and where it stalls:
  cw.get_metric_statistics(Namespace="AWS/Cognito", MetricName=<series>, Dimensions=[...], StartTime=now-20m, EndTime=now+5m, Period=3600, Statistics=["Sum","SampleCount"])
  driven by _poll_all_until_expected(...) over four probes at once, which returns only when EVERY probe matches its expected (Sum, SampleCount) and a re-read is identical.
  Expected settled readings: A.SignInSuccesses (Sum 2.0, SampleCount 5.0) under [{UserPool: pool},{UserPoolClient: clientA}] with Average exactly 0.4; A.TokenRefreshSuccesses Sum 2.0; A.SignUpSuccesses Sum 1.0; PLUS.NoRisk Sum 1.0 under [{Operation: SignIn},{UserPoolId: plusPool}].
  Actual: each probe returns None (helper returns None when Datapoints is empty) on every poll, so the loop cannot match and cannot reach its own 900 s diagnostic; the harness cap fires first at 60 s.

Assertions that were therefore never reached (all downstream of the settle loop, and all still unverified): client B empty in all three series; NoRisk keyed UserPool (rather than UserPoolId) empty; ListMetrics for UserPoolId=<plusPool> listing (NoRisk, (Operation, UserPoolId)); ListMetrics for UserPool=<pool> listing (SignInSuccesses, (UserPool, UserPoolClient)); and no NoRisk entry under any UserPool dimension set.

That all the cognito-idp calls — including the refusal code and message, the PLUS tier echo and the AUDIT mode echo — pass while only the CloudWatch read stalls isolates the divergence to metric emission rather than to the authentication operations.

## Steps to Reproduce
1. Boot the emulator (cognito-idp and cloudwatch both answer).
2. CreateUserPool, then CreateUserPoolClient with GenerateSecret=false and ExplicitAuthFlows=[ALLOW_USER_PASSWORD_AUTH, ALLOW_REFRESH_TOKEN_AUTH]. AdminCreateUser(MessageAction=SUPPRESS) + AdminSetUserPassword(Permanent=true) to get a CONFIRMED user. Create a second, never-used app client B on the same pool.
3. Call InitiateAuth AuthFlow=USER_PASSWORD_AUTH twice with the correct password (both return AccessToken+RefreshToken) and three times with a wrong password (each refused NotAuthorizedException "Incorrect username or password.").
4. Call InitiateAuth AuthFlow=REFRESH_TOKEN_AUTH twice with a refresh token from step 3; call SignUp for a new user and then AdminConfirmSignUp.
5. Call GetMetricStatistics with Namespace="AWS/Cognito", MetricName="SignInSuccesses", Dimensions=[{Name:"UserPool",Value:<poolId>},{Name:"UserPoolClient",Value:<clientA>}], StartTime 20 minutes back, EndTime 5 minutes ahead, Period=3600, Statistics=["Sum","SampleCount"].
   Observed: Datapoints == [] (and it stays empty on every retry).
   Expected (AWS): a settled Sum 2.0 / SampleCount 5.0, i.e. Average 0.4 — the three refusals present as zero-valued samples.
   Repeat for MetricName="TokenRefreshSuccesses" (expect Sum 2.0) and "SignUpSuccesses" (expect Sum 1.0) — both empty.
6. Repeat step 5 for app client B: AWS also returns nothing here, so this arm only becomes meaningful once step 5 produces data.
7. CreateUserPool with UserPoolTier=PLUS and UserPoolAddOns={AdvancedSecurityMode: "AUDIT"}, add an app client and a CONFIRMED user, take one successful InitiateAuth, then call GetMetricStatistics for MetricName="NoRisk" with Dimensions=[{Name:"Operation",Value:"SignIn"},{Name:"UserPoolId",Value:<plusPoolId>}].
   Observed: no datapoint under either dimension key.
   Expected (AWS): Sum 1.0 under (Operation, UserPoolId), and nothing under (Operation, UserPool).
8. Call ListMetrics for Namespace="AWS/Cognito" filtered by Dimensions=[{Name:"UserPoolId",Value:<plusPoolId>}] and again by [{Name:"UserPool",Value:<poolId>}].
   Observed: no AWS/Cognito entries at all.
   Expected (AWS): (NoRisk, (Operation, UserPoolId)) in the first and (SignInSuccesses, (UserPool, UserPoolClient)) in the second, with NoRisk never appearing under a UserPool dimension set.

Replay shortcut: re-run the check named in trace_ids.
