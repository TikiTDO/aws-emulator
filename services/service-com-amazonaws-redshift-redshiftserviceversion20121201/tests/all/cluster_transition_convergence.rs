// SPDX-License-Identifier: CC0-1.0

//! Every clock-owed cluster transition converges for a client that only POLLS.
//!
//! A mutating cluster call leaves work the service owes: the cluster reports
//! `creating` / `modifying` / `rebooting`, and an IAM role association reports
//! `adding` or `removing`. The RULE under test is one rule for all of them: each
//! such transition becomes due a bounded span after the call that started it and
//! is materialised by the next REPORTING call, so a caller that polls
//! `DescribeClusters` — and makes no second mutating call — reaches the settled
//! state in bounded time. A poll loop that cannot terminate is the defect this
//! suite exists to catch: a client reconciling a cluster (Terraform's provider
//! polls `ClusterAvailabilityStatus` and the `adding` -> `in-sync` convergence)
//! then has no completion signal at all.
//!
//! The span differs by what the call does, and the table asserts that too: a
//! change applied IN PLACE to a cluster that is already running (its IAM role set)
//! is settled by the time that client's next call arrives, so its loop ends on the
//! first or second poll, while bringing a cluster UP — or rebooting it — takes the
//! longer provisioning span and is seen in progress first.
//!
//! The table spans the mutating calls that start such a transition — the create,
//! an association, an association of an ARN naming a role that does not exist, a
//! disassociation of several at once, a default-role change and a reboot — rather
//! than the one recorded sequence. An IAM role belongs to another service, so this
//! service resolves none: a well-formed ARN naming nothing is associated like any
//! other, which is the row that pins it.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator, POLL_INTERVAL};

/// The example role ARN the model gives for `ClusterIamRole$IamRoleArn`
/// ("for example, `arn:aws:iam::123456789012:role/RedshiftCopyUnload`").
const ROLE_COPY_UNLOAD: &str = "arn:aws:iam::123456789012:role/RedshiftCopyUnload";
/// A second ARN of the same documented form.
const ROLE_SPECTRUM: &str = "arn:aws:iam::123456789012:role/RedshiftSpectrum";
/// A well-formed ARN naming a role nothing ever created.
const ROLE_NONEXISTENT: &str = "arn:aws:iam::123456789012:role/RedshiftCopyUnload-does-not-exist";

/// `call` takes borrowed pairs; the rows build owned parameters.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// The `(IamRoleArn, ApplyStatus)` of every association an answer carries, in
/// report order.
fn associations(body: &str) -> Vec<(String, String)> {
    let open = "<IamRoles>";
    let Some(start) = body.find(open) else {
        return Vec::new();
    };
    let rest = &body[start + open.len()..];
    let end = rest.find("</IamRoles>").unwrap_or(rest.len());
    let block = &rest[..end];
    all_text(block, "IamRoleArn")
        .into_iter()
        .zip(all_text(block, "ApplyStatus"))
        .collect()
}

/// The expected settled set: every ARN associated and `in-sync`.
fn settled_set(arns: &[&str]) -> Vec<(String, String)> {
    arns.iter()
        .map(|arn| {
            (
                (*arn).to_string(),
                state::IAM_ROLE_APPLY_STATUS_IN_SYNC.to_string(),
            )
        })
        .collect()
}

/// Poll `DescribeClusters` until `settled` holds of the body, answering the answer
/// it held on and how many polls that took.
///
/// The loop itself is the harness's ([`Emulator::poll_until`]), so this suite keeps
/// only what is its own: which reporting call to make, and what settled looks like
/// in its body. That loop makes NO mutating call, so only a transition the clock
/// owes can end it — which is exactly the property under test.
async fn poll_until(
    emulator: &Emulator,
    case: &str,
    cluster: &str,
    settled: impl Fn(&str) -> bool,
) -> (Answer, usize) {
    emulator
        .poll_until(
            case,
            "DescribeClusters",
            &[("ClusterIdentifier", cluster)],
            |answer| settled(&answer.body),
        )
        .await
}

/// One row: the case name, the action, the parameters it carries, the
/// association set the cluster must hold once the row has settled, and the most
/// polls a client may need to SEE it settled.
///
/// That last column is the rule's other half. Bringing a cluster UP is the slow
/// transition, so a poll loop sees `creating` at least once before it settles
/// (`None` — many polls, bounded only by the deadline). A change applied IN PLACE
/// to a cluster that is already running settles inside the gap between one call of
/// a client's loop and the next, so that loop is done on its first or second poll
/// (`Some(2)`) rather than paying another whole poll interval.
type TransitionCase = (
    &'static str,
    &'static str,
    Vec<(String, String)>,
    &'static [&'static str],
    Option<usize>,
);

/// `AddIamRoles` / `RemoveIamRoles` as the awsQuery form carries them: the list
/// member's own element name (`IamRoleArn`), which is what an SDK sends.
fn role_params(cluster: &str, add: &[&str], remove: &[&str]) -> Vec<(String, String)> {
    let mut params = vec![("ClusterIdentifier".to_string(), cluster.to_string())];
    for (position, arn) in add.iter().enumerate() {
        params.push((
            format!("AddIamRoles.IamRoleArn.{}", position + 1),
            (*arn).to_string(),
        ));
    }
    for (position, arn) in remove.iter().enumerate() {
        params.push((
            format!("RemoveIamRoles.IamRoleArn.{}", position + 1),
            (*arn).to_string(),
        ));
    }
    params
}

/// Every mutating call's clock-owed transition is reached by polling alone.
#[tokio::test]
async fn every_clock_owed_cluster_transition_settles_for_a_polling_client() {
    let emulator = Emulator::new();
    let cluster = "convergence-cluster";

    // The node type is read out of the catalog the service publishes rather than
    // written here, so the row creates a cluster the service calls orderable.
    let catalog = emulator
        .call("DescribeOrderableClusterOptions", &[])
        .await;
    assert_eq!(
        catalog.status, 200,
        "case the orderable catalog: expected 200, got {} with body {}",
        catalog.status, catalog.body
    );
    let node_type = catalog
        .body
        .split("<OrderableClusterOption>")
        .skip(1)
        .find(|option| option.contains("<ClusterType>single-node</ClusterType>"))
        .and_then(|option| all_text(option, "NodeType").into_iter().next())
        .expect("the catalog offers a single-node option");

    let rows: Vec<TransitionCase> = vec![
        (
            "the create",
            "CreateCluster",
            vec![
                ("ClusterIdentifier".to_string(), cluster.to_string()),
                ("NodeType".to_string(), node_type.clone()),
                ("MasterUsername".to_string(), "adminuser".to_string()),
                ("MasterUserPassword".to_string(), "Probe-Password-1".to_string()),
                ("ClusterType".to_string(), "single-node".to_string()),
                ("DBName".to_string(), "devdb".to_string()),
                ("PubliclyAccessible".to_string(), "false".to_string()),
                ("Encrypted".to_string(), "true".to_string()),
                ("AutomatedSnapshotRetentionPeriod".to_string(), "1".to_string()),
            ],
            &[],
            None,
        ),
        (
            "one role associated",
            "ModifyClusterIamRoles",
            role_params(cluster, &[ROLE_COPY_UNLOAD], &[]),
            &[ROLE_COPY_UNLOAD],
            Some(2),
        ),
        (
            "an ARN naming a role that does not exist associated",
            "ModifyClusterIamRoles",
            role_params(cluster, &[ROLE_NONEXISTENT], &[]),
            &[ROLE_COPY_UNLOAD, ROLE_NONEXISTENT],
            Some(2),
        ),
        (
            "a third role associated and a default named in one call",
            "ModifyClusterIamRoles",
            {
                let mut params = role_params(cluster, &[ROLE_SPECTRUM], &[]);
                params.push(("DefaultIamRoleArn".to_string(), ROLE_SPECTRUM.to_string()));
                params
            },
            &[ROLE_COPY_UNLOAD, ROLE_NONEXISTENT, ROLE_SPECTRUM],
            Some(2),
        ),
        (
            "several roles disassociated at once",
            "ModifyClusterIamRoles",
            role_params(cluster, &[], &[ROLE_COPY_UNLOAD, ROLE_NONEXISTENT, ROLE_SPECTRUM]),
            &[],
            Some(2),
        ),
        (
            "a reboot",
            "RebootCluster",
            vec![("ClusterIdentifier".to_string(), cluster.to_string())],
            &[],
            None,
        ),
    ];

    for (case, action, params, settled, most_polls) in rows {
        let answered = emulator.call(action, &borrowed(&params)).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected {action} 200, got {} with body {}",
            answered.status, answered.body
        );
        // A call that asked for a change leaves the service applying it, so it
        // reports work in progress rather than the settled state — the span is
        // never zero, which is why a poll loop is the only way to see the end of
        // it.
        assert_ne!(
            answered.text("ClusterStatus").as_deref(),
            Some(state::STATUS_AVAILABLE),
            "case {case:?}: expected a transient ClusterStatus while the change is applied, \
             got body {}",
            answered.body
        );

        let expected = settled_set(settled);
        let (read, polls) = poll_until(&emulator, case, cluster, |body| {
            all_text(body, "ClusterStatus").first().map(String::as_str)
                == Some(state::STATUS_AVAILABLE)
                && associations(body) == expected
        })
        .await;
        assert_eq!(
            associations(&read.body),
            expected,
            "case {case:?}: expected {expected:?} after {polls} poll(s), got {:?}",
            associations(&read.body)
        );
        // The derived availability members settle with the status, which is what a
        // reconciling client reads.
        assert_eq!(
            read.text("ClusterAvailabilityStatus").as_deref(),
            Some("Available"),
            "case {case:?}: expected ClusterAvailabilityStatus=Available once settled, got {:?}",
            read.text("ClusterAvailabilityStatus")
        );
        assert_eq!(
            read.text("AvailabilityZoneRelocationStatus").as_deref(),
            Some("enabled"),
            "case {case:?}: expected the relocation status the create resolved, got {:?}",
            read.text("AvailabilityZoneRelocationStatus")
        );
        // Nothing transient is left behind: the published `ApplyStatus` values a
        // settled association may carry are `in-sync` alone.
        for (arn, status) in associations(&read.body) {
            assert_eq!(
                status,
                state::IAM_ROLE_APPLY_STATUS_IN_SYNC,
                "case {case:?}: expected {arn} to be settled, got {status:?}"
            );
        }
        // How many polls that took is the rule's other half: an in-place change is
        // settled by the time a client's next call arrives, so its loop ends on the
        // first or second poll, while bringing the cluster up takes the longer span
        // and is seen in progress first.
        match most_polls {
            Some(limit) => assert!(
                polls <= limit,
                "case {case:?}: expected an in-place change to be settled within {limit} \
                 poll(s) at {POLL_INTERVAL:?}, took {polls}"
            ),
            None => assert!(
                polls > 1,
                "case {case:?}: expected the transient state to outlive a caller's next \
                 request, but the first poll already reported it settled"
            ),
        }
    }
}

/// The association list is reported in the wire shape the model declares, so a
/// client's SDK can parse it at all.
///
/// `ClusterIamRoleList`'s member carries `xmlName` `ClusterIamRole`
/// (`smithy_query(query="shape", shape="ClusterIamRoleList")`), so each entry is a
/// `<ClusterIamRole>` element inside the `<IamRoles>` wrapper. An entry under any
/// other element name parses as NO association, which is indistinguishable from an
/// emulator that silently dropped the attach — and a client polling for the
/// association would then never see it.
#[tokio::test]
async fn an_association_is_reported_under_the_element_name_the_model_declares() {
    let emulator = Emulator::new();
    let cluster = "wire-shape-cluster";

    let created = emulator.create_minimal(cluster).await;
    assert_eq!(
        created.status, 200,
        "case seed cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    // An empty list is reported as the wrapper with no entry, never as a missing
    // member: a client reads "no association", not "nothing was reported".
    assert!(
        created.body.contains("<IamRoles"),
        "case the create: expected the IamRoles member to be reported even when empty, \
         got body {}",
        created.body
    );

    let (settled, _) = poll_until(&emulator, "the create", cluster, |body| {
        all_text(body, "ClusterStatus").first().map(String::as_str)
            == Some(state::STATUS_AVAILABLE)
    })
    .await;
    assert!(
        settled.body.contains("<IamRoles"),
        "case the settled read: expected the IamRoles member, got body {}",
        settled.body
    );

    let params = role_params(cluster, &[ROLE_COPY_UNLOAD, ROLE_SPECTRUM], &[]);
    let answered = emulator
        .call("ModifyClusterIamRoles", &borrowed(&params))
        .await;
    assert_eq!(
        answered.status, 200,
        "case two roles associated: expected 200, got {} with body {}",
        answered.status, answered.body
    );
    assert_eq!(
        all_text(&answered.body, "ClusterIamRole").len(),
        2,
        "case two roles associated: expected two <ClusterIamRole> entries, got body {}",
        answered.body
    );

    let (read, _) = poll_until(&emulator, "two roles associated", cluster, |body| {
        associations(body) == settled_set(&[ROLE_COPY_UNLOAD, ROLE_SPECTRUM])
    })
    .await;
    assert_eq!(
        all_text(&read.body, "ClusterIamRole").len(),
        2,
        "case two roles associated: expected a later read to report two <ClusterIamRole> \
         entries, got body {}",
        read.body
    );
}

/// A create carrying a member whose LIST is empty is accepted, and reports no
/// association.
///
/// This is the wire form an AWS SDK sends for an empty list: botocore encodes
/// `IamRoles=[]` as the bare `IamRoles=` parameter with no indexed entry. A
/// request refused there never creates the cluster at all, so a client's poll loop
/// has nothing to wait for.
#[tokio::test]
async fn a_create_carrying_an_empty_list_member_is_accepted() {
    let emulator = Emulator::new();

    // Rows: each list-typed member of `CreateClusterMessage` an SDK can be handed
    // an empty list for.
    let rows: [(&str, &str); 4] = [
        ("an empty IamRoles", "IamRoles"),
        ("an empty Tags", "Tags"),
        ("an empty VpcSecurityGroupIds", "VpcSecurityGroupIds"),
        ("an empty ClusterSecurityGroups", "ClusterSecurityGroups"),
    ];

    for (index, (case, member)) in rows.into_iter().enumerate() {
        let cluster = format!("empty-list-cluster-{index}");
        let created = emulator
            .call(
                "CreateCluster",
                &[
                    ("ClusterIdentifier", cluster.as_str()),
                    ("NodeType", "ra3.xlplus"),
                    ("MasterUsername", "adminuser"),
                    ("ClusterType", "single-node"),
                    (member, ""),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateCluster 200, got {} with body {}",
            created.status, created.body
        );
        assert!(
            associations(&created.body).is_empty(),
            "case {case:?}: expected no association, got {:?}",
            associations(&created.body)
        );
        let (read, _) = poll_until(&emulator, case, &cluster, |body| {
            all_text(body, "ClusterStatus").first().map(String::as_str)
                == Some(state::STATUS_AVAILABLE)
        })
        .await;
        assert!(
            associations(&read.body).is_empty(),
            "case {case:?}: expected a later read to report no association, got {:?}",
            associations(&read.body)
        );
    }
}
