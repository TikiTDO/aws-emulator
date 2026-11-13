// SPDX-License-Identifier: CC0-1.0

//! RESTART PERSISTENCE: which services' state survives the emulator restarting, and when it is
//! written.
//!
//! A service opts in by implementing [`service_common::persist::Persisted`] and being listed in
//! [`Persistence::all`]. Nothing here runs unless boot selected a [`StateDir`] (from
//! `EMULATOR_STATE_DIR`); without one the emulator is as ephemeral as it always was. Boot builds
//! the selection once and passes the same value here and to every other writer of state.
//!
//! - **Boot.** [`Persistence::restore`] reads every listed service's snapshot before the router is
//!   built and before any listener opens. A snapshot that cannot be read honestly (another
//!   namespace's, a newer version's, a damaged file) stops the boot with a named error rather
//!   than starting that service empty.
//! - **Every request.** [`PersistingRouter`] answers a request only after writing every listed
//!   service whose state the request changed, so a change a caller was told succeeded is on
//!   disk before the caller hears it, and a `kill -9` loses only unanswered requests. If a
//!   write fails, the caller gets a 500 saying the outcome is unknown and not durable instead
//!   of the success, and the next save retries it. "Every
//!   listed service", not "the service the request named": a CloudFormation deploy changes EC2
//!   and IAM through in-process calls the outer request does not name. Unchanged services are
//!   compared, not written.
//! - **Streaming.** A payload exchange the router selects (B1) is wrapped the same way: the
//!   handler runs, changed services are written, then the response head goes out, with neither
//!   body collected.
//! - **Background.** Work that changes state after its response (a container a task launched, a
//!   clock move) is written by a two-second periodic save in `main.rs`; that is the window
//!   such a change can be lost in.
//! - **Shutdown.** [`Persistence::save_changed`] once more after the listener drains.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use emulator_server::transport::{
    ExchangeContext, ExchangeFuture, PayloadExchange, TransportBody, TransportSelection,
};
use engine::InMemoryEngine;
use service_common::persist::{PersistError, Persisted, StateDir};
use smithy_server_runtime::MultiServiceRouter;

/// One persisted service, with its type erased so the list can hold them all.
struct Entry {
    service: &'static str,
    version: u32,
    restore: fn(&StateDir, &InMemoryEngine) -> Result<bool, PersistError>,
    snapshot: fn(&InMemoryEngine) -> serde_json::Value,
}

impl Entry {
    fn of<T: Persisted + Default + Send + 'static>() -> Self {
        Self {
            service: T::SERVICE,
            version: T::VERSION,
            restore: |state_dir, engine| match state_dir.load::<T>()? {
                Some(state) => {
                    *engine.state::<T>().lock() = state;
                    Ok(true)
                }
                None => Ok(false),
            },
            snapshot: |engine| engine.state::<T>().lock().snapshot(),
        }
    }
}

/// The persisted services of one engine, and what was last written for each.
pub struct Persistence {
    state_dir: StateDir,
    engine: Arc<InMemoryEngine>,
    entries: Vec<Entry>,
    /// A hash of each service's last written snapshot, so an unchanged one is not rewritten.
    /// The async lock also orders writes: two requests never interleave a service's file.
    written: tokio::sync::Mutex<HashMap<&'static str, u64>>,
}

/// What a boot restored.
#[derive(Debug, Default, PartialEq)]
pub struct Restored {
    /// Services whose snapshot was read back.
    pub restored: Vec<&'static str>,
    /// Persisted services with no snapshot yet: they start empty.
    pub empty: Vec<&'static str>,
}

impl Persistence {
    /// Every persisted service, in dependency order: a service is listed after the services
    /// its records need (Secrets Manager's values are sealed with KMS keys).
    /// Admits the namespace's directory (creating it private to this user) before anything is
    /// read; an error when it cannot be.
    pub fn all(engine: Arc<InMemoryEngine>, state_dir: StateDir) -> Result<Self, PersistError> {
        state_dir.admit()?;
        Ok(Self {
            state_dir,
            engine,
            entries: vec![
                Entry::of::<service_com_amazonaws_kms_trentservice::state::ComAmazonawsKmsTrentserviceState>(),
                Entry::of::<service_com_amazonaws_iam_awsidentitymanagementv20100508::state::ComAmazonawsIamAwsidentitymanagementv20100508State>(),
                Entry::of::<service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State>(),
            ],
            written: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Read back every snapshot. Call before the router is built and before any listener opens.
    pub fn restore(&self) -> Result<Restored, PersistError> {
        let mut outcome = Restored::default();
        let mut written = self.written.try_lock().expect("restore runs before any request");
        for entry in &self.entries {
            if (entry.restore)(&self.state_dir, &self.engine)? {
                outcome.restored.push(entry.service);
                // What was just read is what is on disk: no need to write it back unchanged.
                written.insert(entry.service, fingerprint(&(entry.snapshot)(&self.engine)));
            } else {
                outcome.empty.push(entry.service);
            }
        }
        Ok(outcome)
    }

    /// Write every service whose state differs from what was last written. Returns the
    /// services written, or, if any write failed, which ones did not reach disk. A failed
    /// service keeps its old fingerprint, so the next save tries it again.
    pub async fn save_changed(&self) -> Result<Vec<&'static str>, NotDurable> {
        let mut written = self.written.lock().await;
        let mut saved = Vec::new();
        let mut failed = Vec::new();
        for entry in &self.entries {
            let snapshot = (entry.snapshot)(&self.engine);
            let print = fingerprint(&snapshot);
            if written.get(entry.service) == Some(&print) {
                continue;
            }
            match self.state_dir.save_snapshot(entry.service, entry.version, &snapshot) {
                Ok(_) => {
                    written.insert(entry.service, print);
                    saved.push(entry.service);
                }
                Err(error) => {
                    tracing::error!(service = entry.service, %error, "snapshot not written");
                    failed.push((entry.service, error.to_string()));
                }
            }
        }
        if failed.is_empty() {
            Ok(saved)
        } else {
            Err(NotDurable { failed })
        }
    }
}

/// Some changed services could not be written: their changes are applied in memory but would
/// not survive a restart.
#[derive(Debug)]
pub struct NotDurable {
    pub failed: Vec<(&'static str, String)>,
}

impl NotDurable {
    /// What a caller is told instead of success. The change may well have been applied (it is
    /// in memory, and the next save retries it), so this claims no rollback: it says the
    /// outcome is unknown and not durable. A 500 `InternalFailure`, which SDKs surface as a
    /// server error.
    fn response(&self) -> http::Response<bytes::Bytes> {
        let services: Vec<&str> = self.failed.iter().map(|(service, _)| *service).collect();
        let message = format!(
            "The request may have been applied, but the emulator could not make it durable \
             (state not written for: {}). Its outcome after a restart is unknown.",
            services.join(", ")
        );
        http::Response::builder()
            .status(http::StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "application/json")
            .header("x-amzn-errortype", "InternalFailure")
            .body(bytes::Bytes::from(
                serde_json::json!({ "__type": "InternalFailure", "message": message }).to_string(),
            ))
            .expect("a static status and headers")
    }
}

fn fingerprint(snapshot: &serde_json::Value) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(snapshot)
        .expect("a snapshot is JSON")
        .hash(&mut hasher);
    hasher.finish()
}

/// The production router, answering each request only once its changes are on disk.
#[derive(Clone)]
pub struct PersistingRouter {
    router: MultiServiceRouter,
    persistence: Arc<Persistence>,
}

impl PersistingRouter {
    pub fn new(router: MultiServiceRouter, persistence: Arc<Persistence>) -> Self {
        Self { router, persistence }
    }
}

impl emulator_server::EmulatorService for PersistingRouter {
    /// The wrapped router's choice, unchanged, except that a selected payload exchange is
    /// wrapped so it, too, writes its changes before the caller hears success. Without this
    /// the trait's default (`Buffered`) would quietly turn streaming off in persistent mode.
    fn select_transport(&self, parts: &http::request::Parts) -> TransportSelection {
        match self.router.select_transport(parts) {
            TransportSelection::Payload(exchange) => {
                TransportSelection::Payload(Arc::new(PersistingExchange {
                    exchange,
                    persistence: Arc::clone(&self.persistence),
                }))
            }
            other => other,
        }
    }

    async fn handle_request(
        &self,
        request: http::Request<bytes::Bytes>,
    ) -> http::Response<bytes::Bytes> {
        let response = self.router.dispatch(request).await;
        match self.persistence.save_changed().await {
            Ok(_) => response,
            Err(not_durable) => not_durable.response(),
        }
    }

    fn serving_at(&self, endpoint_url: &str) {
        self.router.serving_at(endpoint_url);
    }
}

/// A streaming exchange that writes every changed service after the handler has finished and
/// before its response head is returned. The bodies are never collected: the request body is
/// the handler's to consume, and the response body still streams to the caller afterwards.
/// What the handler committed by the time it returns its response (an upload's object record)
/// is therefore on disk before the caller sees the status.
struct PersistingExchange {
    exchange: Arc<dyn PayloadExchange>,
    persistence: Arc<Persistence>,
}

impl PayloadExchange for PersistingExchange {
    fn run(self: Arc<Self>, request: http::Request<TransportBody>, context: ExchangeContext) -> ExchangeFuture {
        Box::pin(async move {
            let response = Arc::clone(&self.exchange).run(request, context).await;
            match self.persistence.save_changed().await {
                Ok(_) => response,
                Err(not_durable) => not_durable.response().map(TransportBody::from),
            }
        })
    }
}
