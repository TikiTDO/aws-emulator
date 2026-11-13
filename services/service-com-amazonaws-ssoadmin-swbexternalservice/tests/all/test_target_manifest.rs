// SPDX-License-Identifier: CC0-1.0

//! The single integration-test target this crate declares builds, links and serves.
//!
//! `tests/all.rs` is the ONE crate root of this crate's integration-test target: every
//! suite beside this one is loaded by a `mod` item there, so the manifest going wrong takes
//! every suite in the crate down with it and none of them runs. That rule — one `mod` item
//! per suite file, each carrying the `#[path]` that points it into `tests/all/` — is
//! enforced for every crate in the workspace by `core/router`'s own
//! `all_rs_declares_each_suite_once` suite and, for a suite file left undeclared, by the
//! build-time test-layout check; neither can say whether the target they describe actually
//! answers a request, because both only read files.
//!
//! This suite is that last step, asserted the way a client would see it: the account's
//! instance surface read over `MultiServiceRouter::dispatch`, through the `awsJson1_1`
//! envelope and the `X-Amz-Target` routing, out of the state the service holds.

use serde_json::json;

use crate::ssoadmin_permission_sets::{call, is_instance_arn, router};

/// The target the manifest declares serves a request: `ListInstances` over a fresh account
/// hands out the one instance such an account discovers, under an ARN of the modelled form,
/// and hands out the same one on a second read.
#[tokio::test]
async fn the_declared_test_target_serves_a_request() {
    let router = router();
    let mut served = Vec::new();
    for read in ["first read", "second read"] {
        let (status, body) = call(&router, "ListInstances", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {read:?}: ListInstances expected 200, got {status} {body}"
        );
        let instances = body["Instances"]
            .as_array()
            .unwrap_or_else(|| panic!("case {read:?}: no Instances in {body}"));
        assert_eq!(
            instances.len(),
            1,
            "case {read:?}: expected the one instance an account discovers, got {instances:?}"
        );
        let instance_arn = instances[0]["InstanceArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {read:?}: the entry reports no InstanceArn: {}",
                    instances[0]
                )
            })
            .to_string();
        assert!(
            is_instance_arn(&instance_arn),
            "case {read:?}: expected an instance ARN, got {instance_arn:?}"
        );
        served.push(instance_arn);
    }
    assert_eq!(
        served[0], served[1],
        "expected both reads to serve the same discovered instance, got {served:?}"
    );
}
