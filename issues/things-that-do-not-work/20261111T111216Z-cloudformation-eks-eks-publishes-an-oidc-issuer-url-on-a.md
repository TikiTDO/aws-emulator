Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__eks.cfn-eks-private-cluster-in-place-update-serves-oidc-discovery-then-not-found
Issue-Key: cloudformation__eks.cfn-eks-private-cluster-in-place-update-serves-oidc-discovery-then-not-found|_get#25|PublishedUrlRejected|target, headers = ctx.endpoint_route(url)
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# EKS publishes an OIDC issuer URL on a real AWS host instead of an address the emulator serves

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCluster

## Description
Cause 1 of 1: the cluster's OIDC issuer URL names oidc.eks.<region>.amazonaws.com, which is not an address the emulator is reached on.

AWS: an EKS cluster's Identity.Oidc.Issuer is an HTTPS URL that actually serves the OIDC discovery document — a GET of <issuer>/.well-known/openid-configuration returns the provider metadata (issuer, jwks_uri, response_types_supported, ...). Everything an emulator publishes must therefore be an address it serves; the host is the emulator's own host and port, not an AWS endpoint.

Emulator: DescribeCluster publishes 'https://oidc.eks.us-west-2.amazonaws.com/id/<cluster-oidc-id>' and the discovery fetch is refused before it leaves: 'endpoint_url was given https://oidc.eks.us-west-2.amazonaws.com/id/<id>/.well-known/openid-configuration (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. The host is a per-run constant of real AWS, not of this deployment.

Why: any client that follows the issuer URL — IRSA setup, `aws eks describe-cluster` plus an OIDC provider registration, service-account token validation, kubectl OIDC flows — is sent to the internet instead of to the emulator, so it either hangs, fails to resolve, or silently talks to the wrong system.

Fix: the EKS CreateCluster/DescribeCluster handler's Identity.Oidc.Issuer construction (and the per-resource listener that must serve /id/<id>/.well-known/openid-configuration) — publish the emulator's own host and port.

Traces: the failing check in trace_ids.

## Evidence
Expected: the published issuer URL is on one of the emulator's own hosts (e.g. http(s)://127.0.0.1:<port>/<path>) and its /.well-known/openid-configuration is fetchable.
Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://oidc.eks.us-west-2.amazonaws.com/id/3BF06E29903A485A5413E6A344BAFF66/.well-known/openid-configuration (published by the emulator or built by the check): its host is not one of the emulator's own hosts.

## Steps to Reproduce
1. CreateStack a template with a private AWS::EKS::Cluster; wait for CREATE_COMPLETE.
2. Call eks DescribeCluster and read Identity.Oidc.Issuer.
3. GET <issuer>/.well-known/openid-configuration — the host is oidc.eks.<region>.amazonaws.com, not an address the emulator serves.
