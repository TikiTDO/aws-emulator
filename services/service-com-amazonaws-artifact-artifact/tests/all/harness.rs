// SPDX-License-Identifier: CC0-1.0

//! The one harness the report suites drive the artifact emulator through.
//!
//! Every suite goes over the router, so each case exercises routing, deserialization,
//! the handler, the persisted state and serialization — the same path a client takes.
//! The request helpers and the expected catalog live here rather than in one suite, so
//! two suites cannot disagree about what the service publishes.
//!
//! `report_workflow.rs` and `report_projection.rs` use it. The crate's other suites still
//! carry their own copies of the router bootstrap: each covers another resource and they
//! are being changed concurrently by other issues, so they are left to migrate with their
//! next edit rather than be rewritten from under their owners here. (No list of them is
//! kept here, so this comment cannot rot as suites are added or migrated — `tests/all.rs`
//! is the register.)

// Each suite uses the part of the harness its own rules need; an item no suite has
// reached yet is still part of the contract this file states.
#![allow(dead_code)]

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host every request of this harness is addressed to — and therefore the
/// host an announced document URL names, since a document URL names the endpoint the
/// caller reached the emulator on.
pub const ENDPOINT: &str = "artifact.eu-west-1.amazonaws.com";

/// A SigV4 credential scope naming this service. It is what routes a request to the
/// artifact service, and its region is what a report ARN is built for.
pub const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
pub const REGION: &str = "eu-west-1";

/// A credential scope whose region field does not have an AWS region's shape. It still
/// names the `artifact` signing name, so the request routes and the handler runs — and
/// has to choose the region a report ARN names without letting a header shape it.
pub const SIGNED_FOR_A_MALFORMED_REGION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/not_a_region/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region a report ARN names when the request's scope offers none usable.
pub const FALLBACK_REGION: &str = "us-east-1";

/// A credential scope for `region`, in the shape SigV4 declares.
pub fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// `AcceptanceType$EXPLICIT`, whose member doc in this service's Smithy model
/// (`smithy_query` shape=AcceptanceType) is "Require explicit click-through acceptance of
/// the Term associated with this Report."
pub const ACCEPTANCE_EXPLICIT: &str = "EXPLICIT";

/// `AcceptanceType$PASSTHROUGH`, whose member doc in the same model is "Do not require
/// explicit click-through acceptance of the Term associated with this Report".
pub const ACCEPTANCE_PASSTHROUGH: &str = "PASSTHROUGH";

/// One row of the service-owned catalog this emulator publishes.
pub struct Expected {
    pub id: &'static str,
    pub name: &'static str,
    pub series: &'static str,
    pub category: &'static str,
    /// One of `AcceptanceType`'s values (`EXPLICIT` | `PASSTHROUGH`).
    pub acceptance_type: &'static str,
    /// Every published version of this report, ascending.
    pub versions: &'static [i64],
}

impl Expected {
    /// The newest version this report publishes — the one `ListReports` projects.
    pub fn latest(&self) -> i64 {
        self.versions
            .iter()
            .copied()
            .next_back()
            .expect("every catalog row publishes a version")
    }

    /// Whether downloading this report requires the explicit click-through acceptance of
    /// the term governing it — what its `AcceptanceType` says
    /// ([`ACCEPTANCE_EXPLICIT`] / [`ACCEPTANCE_PASSTHROUGH`]).
    pub fn requires_explicit_acceptance(&self) -> bool {
        self.acceptance_type == ACCEPTANCE_EXPLICIT
    }
}

/// A catalog row whose download requires the explicit acceptance of its term: the reports
/// a term grant is the receipt for, which is what a suite asserting that gate names.
pub fn a_report_requiring_acceptance() -> &'static Expected {
    CATALOG
        .iter()
        .find(|row| row.requires_explicit_acceptance())
        .expect("the catalog publishes a report requiring explicit acceptance")
}

/// The catalog the emulator serves, in the ascending report-id order `ListReports` uses.
///
/// AWS Artifact has no `CreateReport`: the catalog is service-owned, so these rows are
/// the published report set. They are spelled out here, independently of the seed in
/// `src/state.rs`, so a changed or fabricated catalog fails these suites rather than
/// quietly redefining what they assert. The first four ids and names are AWS's, as the
/// issue's recording of a live `ListReports` shows them; every row after them is a report
/// AWS publishes in Artifact under an id this emulator assigns (see `SEED`'s doc comment).
/// The rows differ along everything the rules generalise over: report series, category,
/// `AcceptanceType`, and how many versions a report has (one, two, three and seven).
pub const CATALOG: &[Expected] = &[
    Expected {
        id: "report-073F0fUCdVVNKJrF",
        name: "AWS Outposts Rack Information Security FAQ",
        series: "AWS Services",
        category: "Alignment Documents",
        acceptance_type: "PASSTHROUGH",
        versions: &[1, 2, 3],
    },
    Expected {
        id: "report-0c1IXmudeXuTlHJJ",
        name: "ISO 27001 2022 Certification",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        versions: &[1, 2, 3],
    },
    Expected {
        id: "report-5eevElBv6MyRmQsD",
        name: "UAE IAR Compliance Report",
        series: "UAE TDRA IAR",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        versions: &[1, 2, 3, 4, 5, 6, 7],
    },
    Expected {
        id: "report-7g5j2DTVeNXWx7St",
        name: "Annual Compliance Report",
        series: "ACR",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        versions: &[1],
    },
    Expected {
        id: "report-9pQ2mT8xLbV4sZ1c",
        name: "SOC 2 Type 2 Report",
        series: "SOC",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        versions: &[1, 2],
    },
    Expected {
        id: "report-Af3KdN7yRw2HqS5e",
        name: "PCI DSS Attestation of Compliance",
        series: "PCI",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        versions: &[1, 2],
    },
    Expected {
        id: "report-Bq7WmF3xTk9ZpL2a",
        name: "SOC 1 Report",
        series: "SOC",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        versions: &[1, 2],
    },
    Expected {
        id: "report-C4tYhR8nJd6VxQ3b",
        name: "SOC 3 Report",
        series: "SOC",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        versions: &[1],
    },
    Expected {
        id: "report-D9kPzX2mLb7TnR4c",
        name: "ISO 27017 Certification",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        versions: &[1, 2],
    },
    Expected {
        id: "report-E2nVbK6yQw8MsZ5d",
        name: "ISO 27018 Certification",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        versions: &[1],
    },
    Expected {
        id: "report-F6sJdL9xRt3WqN7e",
        name: "ISO 27701 Certification",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        versions: &[1],
    },
    Expected {
        id: "report-G3mQfT7bXz5KpV8h",
        name: "ISO 9001 Certification",
        series: "ISO",
        category: "Certifications and Attestations",
        acceptance_type: "PASSTHROUGH",
        versions: &[1],
    },
    Expected {
        id: "report-H8wRnP4cYk2JsL6m",
        name: "FedRAMP Moderate Customer Package",
        series: "FedRAMP",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        versions: &[1],
    },
    Expected {
        id: "report-J5zXkM9dVb3QtR7n",
        name: "IRAP Assessment Report",
        series: "IRAP",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        versions: &[1],
    },
    Expected {
        id: "report-K7bNqS2fWx8LmT4p",
        name: "C5 Attestation Report",
        series: "C5",
        category: "Certifications and Attestations",
        acceptance_type: "EXPLICIT",
        versions: &[1, 2],
    },
    Expected {
        id: "report-L4dTvJ8hZn6RkQ9s",
        name: "Cloud Security Alliance CAIQ",
        series: "CSA",
        category: "Alignment Documents",
        acceptance_type: "PASSTHROUGH",
        versions: &[1],
    },
];

/// One emulator over `engine`, so a suite can reach the same store the handler reads —
/// the secret its pagination cursors are sealed under, for instance.
pub fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One emulator: a router carrying only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// What the emulator answered: the HTTP status, the `x-amzn-errortype` header a restJson1
/// client resolves an error shape from, and the JSON body.
pub struct Answer {
    pub status: u16,
    /// The `x-amzn-errortype` header, or the empty string when the answer carries none —
    /// which is every success. Carried here so a suite asserting the SHAPE a refusal
    /// travels in does not need a sender of its own.
    pub error_type: String,
    pub body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    pub fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `reports` list a paginated report operation returns.
    pub fn reports(&self) -> &[Value] {
        self.body["reports"].as_array().map_or(&[], |items| items)
    }

    /// The `nextToken` the answer carries, if any.
    pub fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

pub async fn get(router: &MultiServiceRouter, uri: &str) -> Answer {
    get_signed(router, uri, SIGNED_FOR_EU_WEST_1).await
}

/// One request under ANY method, with the body that method carries: the sender a suite
/// needs for a write, a removal or a read alike, so no suite grows a second copy of the
/// request builder. `get` above is this one specialised to a read.
pub async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Bytes) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", ENDPOINT)
        .header("content-type", "application/json")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .body(body)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = error_type_of(&response);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// The `x-amzn-errortype` header of one answer, or the empty string when it carries none.
fn error_type_of(response: &::http::Response<Bytes>) -> String {
    response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

pub async fn get_signed(router: &MultiServiceRouter, uri: &str, authorization: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", ENDPOINT)
        .header("authorization", authorization)
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = error_type_of(&response);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

pub async fn list_reports(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/list{query}")).await
}

pub async fn list_report_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/listVersions{query}")).await
}

pub async fn get_report_metadata(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/getMetadata{query}")).await
}

pub async fn get_term_for_report(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/getTermForReport{query}")).await
}

pub async fn get_report(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/get{query}")).await
}

/// What following an announced document URL served: the status, the content type and the
/// raw bytes — a document is not JSON, so it is kept as bytes rather than parsed.
pub struct Served {
    pub status: u16,
    pub content_type: String,
    pub bytes: Bytes,
}

/// Follow a `documentPresignedUrl` the service announced, exactly as a client does:
/// addressed to the endpoint that URL names ([`ENDPOINT`]) and with NO credential of any
/// kind, since the URL carries the token that authorises the fetch.
///
/// Asserts the announcement is absolute and names this service's own endpoint, so the URL a
/// suite follows is the one a client could follow; `case` names the row that announced it.
pub async fn follow_announced_url(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> Served {
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case}: an announced URL is absolute, got {url:?}"));
    let (host, path_and_query) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case}: an announced URL carries a path, got {url:?}"));
    assert_eq!(
        host, ENDPOINT,
        "case {case}: an announced URL must name the endpoint the caller reached, got {url:?}"
    );

    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("/{path_and_query}"))
        .header("host", ENDPOINT)
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Served {
        status,
        content_type,
        bytes: response.into_body(),
    }
}
