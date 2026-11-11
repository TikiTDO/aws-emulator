// SPDX-License-Identifier: CC0-1.0

//! Amazon DynamoDB Accelerator's published default-quota catalog.

use super::*;

/// Amazon DynamoDB Accelerator's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `dax` service code, and the
/// recorded default-quota listing of that code for which `QuotaCode` names which quota
/// and for the `Description` the service publishes under each of them. DAX publishes the
/// node and subnet planes of a Region beside the planes counted inside one cluster or
/// subnet group, and words a description for the plane the quota counts on — an AWS
/// Region, an AWS account, one cluster or one subnet group.
pub(in crate::state) fn dax_quotas() -> Vec<QuotaRow> {
    vec![
        // The recording states this code's name but not its value. SOURCE for the 20:
        // the "Limits" chapter of the Amazon DynamoDB Accelerator section of the Amazon
        // DynamoDB Developer Guide, whose parameter-group row publishes 20 per account
        // per Region. AWS publishes it as a hard limit rather than as adjustable.
        quota("Parameter groups", 20.0)
            .code("L-315AFD08")
            .fixed()
            .describe("The maximum number of parameter groups in a single AWS region."),
        // The node plane of one cluster and the subnet plane of one subnet group, both
        // of which AWS publishes as hard limits rather than as adjustable.
        quota("Nodes per cluster", 11.0)
            .code("L-87AEEBB5")
            .fixed()
            .describe(
                "The maximum number of nodes per cluster, including the primary node as well as \
                 any read replica nodes.",
            ),
        quota("Total number of nodes", 50.0)
            .code("L-AB139030")
            .describe("The maximum total number of nodes per AWS account in a single AWS region."),
        quota("Subnets per subnet group", 20.0)
            .code("L-E34C284B")
            .fixed()
            .describe("The maximum number of subnets per subnet group in a single AWS region."),
        // The subnet-group plane of the Region, which AWS publishes as fixed.
        quota("Subnet groups", 50.0)
            .code("L-F55BD408")
            .fixed()
            .describe("The maximum number of subnet groups in a single AWS region."),
    ]
}
