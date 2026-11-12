// SPDX-License-Identifier: CC0-1.0

//! What a permissions-policy DELETE does when the resource holds no policy, over the wire.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is pinned here is the status and the body a
//! CodeArtifact client reads.
//!
//! THE RULE: `policyRevision` is an OPTIMISTIC LOCK over the policy that is attached — "The
//! current revision of the resource policy to be deleted. This revision is used for
//! optimistic locking, which prevents others from overwriting your changes to the domain's
//! resource policy"
//! (`smithy_query(query="operation",operation="DeleteDomainPermissionsPolicy")`, member
//! `policyRevision`) — so it can only be stale against a record that exists. A resource that
//! holds NO policy has the delete's goal already met: the call succeeds (HTTP 200) and
//! answers the bare `ResourcePolicy` envelope, which names the resource and carries no
//! `document` and no `revision`, whatever revision the caller named. A resource that DOES
//! hold one is still locked: a revision other than the attached one is `ConflictException`
//! (409) and the policy survives.
//!
//! It is one rule for both resources a policy hangs off, so every case drives it through the
//! DOMAIN delete and the REPOSITORY delete alike. The rows differ along what the rule
//! generalises over: whether a revision is named at all and which one (none, a made-up one,
//! the revision of the policy that was just deleted, the modelled extremes of
//! `PolicyRevision`'s length), and along the resource the policy hangs off.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT, REGION};

/// Which resource the policy hangs off: the two policy families this service has, each with
/// its own operations and its own ARN.
#[derive(Debug, Clone, Copy)]
enum Family {
    Domain,
    Repository,
}

/// The domain and repository one row works in.
struct Resource {
    family: Family,
    domain: String,
    repository: String,
}

impl Resource {
    /// The ARN the policy of this resource is reported under.
    fn arn(&self) -> String {
        match self.family {
            Family::Domain => {
                format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}:domain/{}", self.domain)
            }
            Family::Repository => format!(
                "arn:aws:codeartifact:{REGION}:{ACCOUNT}:repository/{}/{}",
                self.domain, self.repository
            ),
        }
    }

    /// The query string that addresses it.
    fn query(&self) -> String {
        match self.family {
            Family::Domain => format!("domain={}", self.domain),
            Family::Repository => {
                format!("domain={}&repository={}", self.domain, self.repository)
            }
        }
    }

    /// `PutDomainPermissionsPolicy` / `PutRepositoryPermissionsPolicy`.
    async fn put(&self, router: &MultiServiceRouter, document: &str) -> Answer {
        let (uri, body) = match self.family {
            Family::Domain => (
                "/v1/domain/permissions/policy".to_string(),
                json!({ "domain": self.domain, "policyDocument": document }),
            ),
            Family::Repository => (
                format!("/v1/repository/permissions/policy?{}", self.query()),
                json!({ "policyDocument": document }),
            ),
        };
        call(router, "PUT", &uri, Some(body)).await
    }

    /// `GetDomainPermissionsPolicy` / `GetRepositoryPermissionsPolicy`.
    async fn get(&self, router: &MultiServiceRouter) -> Answer {
        let uri = match self.family {
            Family::Domain => format!("/v1/domain/permissions/policy?{}", self.query()),
            Family::Repository => format!("/v1/repository/permissions/policy?{}", self.query()),
        };
        call(router, "GET", &uri, None).await
    }

    /// `DeleteDomainPermissionsPolicy` / `DeleteRepositoryPermissionsPolicy` — note the
    /// PLURAL path segment the model binds the repository delete to.
    async fn delete(&self, router: &MultiServiceRouter, revision: Option<&str>) -> Answer {
        let mut uri = match self.family {
            Family::Domain => format!("/v1/domain/permissions/policy?{}", self.query()),
            Family::Repository => {
                format!("/v1/repository/permissions/policies?{}", self.query())
            }
        };
        if let Some(revision) = revision {
            // A revision is base64, so it can carry `+`, `/` and `=` — characters a query
            // string reads as a space, a separator and a delimiter. It travels encoded,
            // the way an SDK sends it.
            uri.push_str(&format!(
                "&policy-revision={}",
                crate::resource_tags::encode_query(revision)
            ));
        }
        call(router, "DELETE", &uri, None).await
    }
}

/// The `revision` a policy answer reports.
fn revision_of(answer: &Answer) -> Option<String> {
    answer.body["policy"]["revision"]
        .as_str()
        .map(str::to_string)
}

/// A policy document to attach, which the rule is indifferent to.
const DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"keep","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::111122223333:root"},"Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#;

/// One resource of each family, in its own domain so the rows cannot see each other's state.
async fn resources(router: &MultiServiceRouter, slug: &str) -> Vec<Resource> {
    let mut held = Vec::new();
    for (index, family) in [Family::Domain, Family::Repository].into_iter().enumerate() {
        let domain = format!("{slug}-{index}");
        assert_eq!(
            create_domain(router, &domain, None).await.status,
            200,
            "CreateDomain {domain} must succeed"
        );
        let repository = "Policy.repo".to_string();
        assert_eq!(
            create_repository(router, &domain, &repository, None, &[])
                .await
                .status,
            200,
            "CreateRepository in {domain} must succeed"
        );
        held.push(Resource {
            family,
            domain,
            repository,
        });
    }
    held
}

/// THE RULE: a delete on a resource holding no policy succeeds with the bare envelope,
/// whatever revision it names — including the revision of the policy it just deleted, which
/// is the re-delete an idempotent teardown makes.
#[tokio::test]
async fn deleting_a_policy_that_is_not_attached_succeeds_whatever_revision_is_named() {
    let router = emulator();

    for resource in resources(&router, "idem").await {
        let family = resource.family;
        let bare = json!({ "policy": { "resourceArn": resource.arn() } });

        // Nothing attached yet: the delete has nothing to remove.
        let first = resource.delete(&router, None).await;
        assert_eq!(
            (first.status, first.body.clone()),
            (200, bare.clone()),
            "case {family:?}: deleting an unset policy must answer the bare envelope, got \
             {first:?}"
        );

        // Attach one, delete it with its own revision — and keep that revision, which is
        // what the re-delete below names.
        let attached = resource.put(&router, DOCUMENT).await;
        assert_eq!(
            attached.status, 200,
            "case {family:?}: the put must succeed, got {attached:?}"
        );
        let revision = revision_of(&attached).expect("the write mints a revision");
        let removed = resource.delete(&router, Some(&revision)).await;
        assert_eq!(
            (removed.status, removed.body["policy"]["document"].as_str()),
            (200, Some(DOCUMENT)),
            "case {family:?}: the delete must answer with the policy it removed, got {removed:?}"
        );

        // Rows differ along the revision the repeated delete names: none, the revision of
        // the policy that was just deleted (the idempotent re-delete), a revision this
        // resource never minted, and the modelled extremes of `PolicyRevision` (len 1..100,
        // pattern `^\S+$`).
        let longest = format!("r{}", "v".repeat(99));
        let rows: Vec<(String, Option<&str>)> = vec![
            ("no revision at all".to_string(), None),
            (
                "the revision of the policy just deleted".to_string(),
                Some(revision.as_str()),
            ),
            ("a revision never minted".to_string(), Some("not-a-revision")),
            ("the shortest PolicyRevision".to_string(), Some("1")),
            ("the longest PolicyRevision".to_string(), Some(longest.as_str())),
        ];
        for (case, named) in rows {
            let again = resource.delete(&router, named).await;
            assert_eq!(
                (again.status, again.body.clone()),
                (200, bare.clone()),
                "case {family:?}/{case:?}: a delete with nothing to remove must answer the bare \
                 envelope, got {again:?}"
            );
            // ...and it removed nothing that was not already gone: the read still reports no
            // policy.
            let read = resource.get(&router).await;
            assert_eq!(
                (read.status, read.code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {family:?}/{case:?}: the resource must still hold no policy, got {read:?}"
            );
        }
    }
}

/// THE OTHER HALF: the lock still holds over a policy that IS attached — a revision other
/// than the attached one is refused and the policy survives, and the attached one removes it.
#[tokio::test]
async fn the_lock_still_refuses_a_stale_revision_while_a_policy_is_attached() {
    let router = emulator();

    for resource in resources(&router, "locked").await {
        let family = resource.family;
        let attached = resource.put(&router, DOCUMENT).await;
        assert_eq!(
            attached.status, 200,
            "case {family:?}: the put must succeed, got {attached:?}"
        );
        let revision = revision_of(&attached).expect("the write mints a revision");

        let longest = format!("r{}", "v".repeat(99));
        for stale in ["1", "not-the-revision", longest.as_str()] {
            let refused = resource.delete(&router, Some(stale)).await;
            assert_eq!(
                (refused.status, refused.code().as_str()),
                (409, "ConflictException"),
                "case {family:?}/{stale:?}: a delete naming another revision must conflict, got \
                 {refused:?}"
            );
            let survived = resource.get(&router).await;
            assert_eq!(
                (
                    survived.status,
                    survived.body["policy"]["document"].as_str(),
                    revision_of(&survived)
                ),
                (200, Some(DOCUMENT), Some(revision.clone())),
                "case {family:?}/{stale:?}: a refused delete must leave the policy attached, got \
                 {survived:?}"
            );
        }

        let removed = resource.delete(&router, Some(&revision)).await;
        assert_eq!(
            removed.status, 200,
            "case {family:?}: the attached revision must unlock the delete, got {removed:?}"
        );
        let gone = resource.get(&router).await;
        assert_eq!(
            (gone.status, gone.code().as_str()),
            (404, "ResourceNotFoundException"),
            "case {family:?}: the policy must be gone from the very next read, got {gone:?}"
        );
    }
}
