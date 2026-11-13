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
//!   disk before the caller hears it, and a `kill -9` loses only unanswered requests. "Every
//!   listed service", not "the service the request named": a CloudFormation deploy changes EC2
//!   and IAM through in-process calls the outer request does not name. Unchanged services are
//!   compared, not written.
//! - **Background.** Work that changes state after its response (a container a task launched, a
//!   clock move) is written by a two-second periodic save in `main.rs`; that is the window
//!   such a change can be lost in.
//! - **Shutdown.** [`Persistence::save_changed`] once more after the listener drains.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

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
    /// services written.
    pub async fn save_changed(&self) -> Vec<&'static str> {
        let mut written = self.written.lock().await;
        let mut saved = Vec::new();
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
                // The change stays in memory and the next request tries again; it is not
                // durable until then, which is what the log says.
                Err(error) => tracing::error!(service = entry.service, %error, "snapshot not written"),
            }
        }
        saved
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
    async fn handle_request(
        &self,
        request: http::Request<bytes::Bytes>,
    ) -> http::Response<bytes::Bytes> {
        let response = self.router.dispatch(request).await;
        self.persistence.save_changed().await;
        response
    }

    fn serving_at(&self, endpoint_url: &str) {
        self.router.serving_at(endpoint_url);
    }
}
