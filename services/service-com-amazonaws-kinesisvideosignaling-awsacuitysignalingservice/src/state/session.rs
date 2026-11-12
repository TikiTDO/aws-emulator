// SPDX-License-Identifier: CC0-1.0

//! The LIVE peers of one signaling channel, and the frames routed between them.
//!
//! A signaling channel's own data plane is a WebSocket: a MASTER peer connects to it and
//! waits, VIEWER peers connect naming a `ClientId`, and the channel brokers the SDP and
//! ICE messages they exchange. Nothing of that is a modelled request — it is the
//! connection state the channel holds while those peers are attached — so it lives here,
//! beside the channel record, and is read by both the socket that carries the frames
//! ([`crate::service::wss`]) and the one REST route that delivers into it
//! (`SendAlexaOfferToMaster`).
//!
//! Three rules this module owns, so neither caller re-decides them:
//!
//! * a peer is reached through a BOUNDED queue ([`PEER_FRAME_DEPTH`]) and a frame is
//!   handed to it without ever awaiting, so a peer that has stopped reading cannot stall
//!   the store lock or grow this process;
//! * one channel holds ONE master and at most [`MAX_VIEWERS_PER_CHANNEL`] viewers, so the
//!   peer count is the channel's and not the callers';
//! * an ANSWER the master addresses to a recipient that holds no socket — the
//!   `SendAlexaOfferToMaster` caller is such a recipient — is handed to whoever is waiting
//!   for it ([`ChannelSession::deliver_answer`]), and the waiters are bounded too
//!   ([`MAX_ANSWER_WAITERS_PER_CHANNEL`]).

use std::collections::BTreeMap;

use ::tokio::sync::mpsc::{channel, Receiver, Sender};
use ::tokio::sync::oneshot;

/// The `messageType` a channel delivers an SDP offer under, and the `action` a peer sends
/// one with: the Kinesis Video Streams WebRTC signaling messages a channel brokers.
pub const SDP_OFFER: &str = "SDP_OFFER";

/// The answer to an offer; see [`SDP_OFFER`].
pub const SDP_ANSWER: &str = "SDP_ANSWER";

/// An ICE candidate a peer offers the other side; see [`SDP_OFFER`].
pub const ICE_CANDIDATE: &str = "ICE_CANDIDATE";

/// How many viewers one channel holds at once.
///
/// Not an AWS quota — a chosen bound. Every connection is a caller's, so without a ceiling
/// the peer count (and the frame queues it carries) would be chosen by whoever dials the
/// channel's socket. Past it a viewer's handshake is refused rather than an attached
/// viewer being dropped: a peer in a session is never severed to make room for a new one.
pub const MAX_VIEWERS_PER_CHANNEL: usize = 10;

/// How many frames wait for ONE peer before the channel stops queueing for it.
///
/// Not an AWS quota — a chosen bound, and the reason a frame is never awaited under the
/// store lock: the queue is handed a frame with `try_send`, so a peer that has stopped
/// reading loses frames instead of holding the lock or growing this process.
pub const PEER_FRAME_DEPTH: usize = 32;

/// How many callers may wait for an answer on ONE channel at once.
///
/// Not an AWS quota — a chosen bound: each waiter is a `SendAlexaOfferToMaster` call in
/// flight on that channel, so without a ceiling the waiter count would be the callers'.
pub const MAX_ANSWER_WAITERS_PER_CHANNEL: usize = 32;

/// One peer attached to a channel: the bounded queue the channel hands its frames to, and
/// the generation that tells one attachment of a slot from the next.
///
/// The peer's own socket task owns the other end ([`PeerConnection::new`]), so a frame
/// queued here is written to that socket by that task and by nothing holding the store
/// lock.
#[derive(Debug)]
pub struct PeerConnection {
    /// The frames queued for this peer, oldest first.
    frames: Sender<String>,
    /// Which attachment of this channel's slot this peer is — set when it is attached,
    /// and what a detach is matched against, so a connection that has already been
    /// REPLACED cannot evict the peer that replaced it.
    generation: u64,
}

impl PeerConnection {
    /// A peer and the receiving half its own socket task reads frames from.
    #[must_use]
    pub fn new() -> (Self, Receiver<String>) {
        let (frames, queued) = channel(PEER_FRAME_DEPTH);
        (
            Self {
                frames,
                generation: 0,
            },
            queued,
        )
    }

    /// Hand `frame` to this peer, WITHOUT awaiting: `false` when the peer has gone or is
    /// already holding [`PEER_FRAME_DEPTH`] frames it has not read.
    pub fn deliver(&self, frame: String) -> bool {
        self.frames.try_send(frame).is_ok()
    }

    /// Whether the peer's socket task is still reading this queue.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.frames.is_closed()
    }
}

/// Which attachment of a channel's slot a connection holds — what its socket task hands
/// back when it closes, so a detach removes ITS peer and never the one that replaced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attachment(u64);

/// The peers attached to one channel, and the answers they owe.
#[derive(Debug, Default)]
pub struct ChannelSession {
    /// The channel's master peer, while one is connected. A `SINGLE_MASTER` channel has
    /// at most one.
    master: Option<PeerConnection>,
    /// The connected viewers, by the `ClientId` each handshake named.
    viewers: BTreeMap<String, PeerConnection>,
    /// Who is waiting for an answer addressed to a recipient that holds no socket, by
    /// that recipient's client id — the `SendAlexaOfferToMaster` callers.
    answer_waiters: BTreeMap<String, Vec<oneshot::Sender<String>>>,
    /// How many peers this session has attached, which is what makes each
    /// [`Attachment`] a new one.
    attachments: u64,
}

impl ChannelSession {
    /// Attach `peer` as this channel's master; `None` when a master is already connected,
    /// which a `SINGLE_MASTER` channel does not take a second of.
    ///
    /// A master whose socket task has ended is replaced rather than counted: the record of
    /// a connection that is gone must not keep the channel masterless for good. The
    /// [`Attachment`] answered is what [`detach_master`](Self::detach_master) is matched
    /// against, so the replaced connection cannot evict its replacement.
    pub fn attach_master(&mut self, mut peer: PeerConnection) -> Option<Attachment> {
        if self.master.as_ref().is_some_and(PeerConnection::is_open) {
            return None;
        }
        let attachment = self.next_attachment();
        peer.generation = attachment.0;
        self.master = Some(peer);
        Some(attachment)
    }

    /// Forget the master `attachment` named, which is what its socket closing does.
    ///
    /// A no-op when the slot has since been taken by another connection: that peer is
    /// live, and this one's close says nothing about it.
    pub fn detach_master(&mut self, attachment: Attachment) {
        if self
            .master
            .as_ref()
            .is_some_and(|held| held.generation == attachment.0)
        {
            self.master = None;
        }
    }

    /// Whether a master peer is connected and still reading.
    #[must_use]
    pub fn has_master(&self) -> bool {
        self.master.as_ref().is_some_and(PeerConnection::is_open)
    }

    /// The master peer, for a frame addressed to it.
    #[must_use]
    pub fn master(&self) -> Option<&PeerConnection> {
        self.master.as_ref().filter(|peer| peer.is_open())
    }

    /// Attach `peer` as the viewer `client_id`; `None` past
    /// [`MAX_VIEWERS_PER_CHANNEL`]. A viewer reconnecting under a client id whose socket
    /// has ended replaces that record rather than being refused, and the [`Attachment`]
    /// answered is what a detach is matched against.
    pub fn attach_viewer(
        &mut self,
        client_id: &str,
        mut peer: PeerConnection,
    ) -> Option<Attachment> {
        self.viewers.retain(|_, held| held.is_open());
        if !self.viewers.contains_key(client_id) && self.viewers.len() >= MAX_VIEWERS_PER_CHANNEL
        {
            return None;
        }
        let attachment = self.next_attachment();
        peer.generation = attachment.0;
        self.viewers.insert(client_id.to_string(), peer);
        Some(attachment)
    }

    /// Forget the viewer `client_id` held under `attachment`, which is what its socket
    /// closing does. A no-op once another connection holds that client id.
    pub fn detach_viewer(&mut self, client_id: &str, attachment: Attachment) {
        if self
            .viewers
            .get(client_id)
            .is_some_and(|held| held.generation == attachment.0)
        {
            self.viewers.remove(client_id);
        }
    }

    /// The next attachment of a slot of this session.
    fn next_attachment(&mut self) -> Attachment {
        self.attachments = self.attachments.wrapping_add(1);
        Attachment(self.attachments)
    }

    /// The viewer `client_id`, for a frame addressed to it.
    #[must_use]
    pub fn viewer(&self, client_id: &str) -> Option<&PeerConnection> {
        self.viewers
            .get(client_id)
            .filter(|peer| peer.is_open())
    }

    /// The client ids of the connected viewers, in order.
    #[must_use]
    pub fn viewer_ids(&self) -> Vec<String> {
        self.viewers
            .iter()
            .filter(|(_, peer)| peer.is_open())
            .map(|(client_id, _)| client_id.clone())
            .collect()
    }

    /// Wait for the answer the master addresses to `recipient_client_id`.
    ///
    /// `None` past [`MAX_ANSWER_WAITERS_PER_CHANNEL`], which leaves the caller to answer
    /// without one rather than growing the map. Dropping the returned receiver is how a
    /// caller that gave up stops waiting; such a waiter is pruned here on the next call.
    pub fn wait_for_answer(
        &mut self,
        recipient_client_id: &str,
    ) -> Option<oneshot::Receiver<String>> {
        self.prune_waiters();
        let waiting: usize = self.answer_waiters.values().map(Vec::len).sum();
        if waiting >= MAX_ANSWER_WAITERS_PER_CHANNEL {
            return None;
        }
        let (answered, answer) = oneshot::channel();
        self.answer_waiters
            .entry(recipient_client_id.to_string())
            .or_default()
            .push(answered);
        Some(answer)
    }

    /// Hand `payload` to one caller waiting for an answer addressed to
    /// `recipient_client_id`; `false` when nobody is waiting for one.
    pub fn deliver_answer(&mut self, recipient_client_id: &str, payload: &str) -> bool {
        self.prune_waiters();
        let Some(waiters) = self.answer_waiters.get_mut(recipient_client_id) else {
            return false;
        };
        while let Some(waiter) = waiters.pop() {
            if waiter.send(payload.to_string()).is_ok() {
                return true;
            }
        }
        self.answer_waiters.remove(recipient_client_id);
        false
    }

    /// Drop the waiters whose caller has gone, so the map holds only live ones.
    fn prune_waiters(&mut self) {
        for waiters in self.answer_waiters.values_mut() {
            waiters.retain(|waiter| !waiter.is_closed());
        }
        self.answer_waiters.retain(|_, waiters| !waiters.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_channel_takes_one_master_and_a_bounded_set_of_viewers() {
        let mut session = ChannelSession::default();
        let (master, _master_frames) = PeerConnection::new();
        assert!(
            session.attach_master(master).is_some(),
            "case first master: a channel with no master must take one"
        );
        let (second, _second_frames) = PeerConnection::new();
        assert!(
            session.attach_master(second).is_none(),
            "case second master: a SINGLE_MASTER channel must not take a second"
        );

        // The rows span the viewer count: up to the cap every viewer attaches, past it
        // none does, and a viewer already attached still re-attaches.
        let mut queues = Vec::new();
        for index in 0..MAX_VIEWERS_PER_CHANNEL {
            let client_id = format!("viewer-{index}");
            let (viewer, frames) = PeerConnection::new();
            queues.push(frames);
            assert!(
                session.attach_viewer(&client_id, viewer).is_some(),
                "case {client_id:?}: a viewer inside the cap must attach"
            );
        }
        let (over, over_frames) = PeerConnection::new();
        assert!(
            session.attach_viewer("viewer-over", over).is_none(),
            "case over the cap: expected the handshake to be refused"
        );
        drop(over_frames);
        let (again, again_frames) = PeerConnection::new();
        queues.push(again_frames);
        assert!(
            session.attach_viewer("viewer-0", again).is_some(),
            "case reconnecting viewer: an attached client id must re-attach"
        );
    }

    /// A connection that has already been REPLACED must not evict the peer that replaced
    /// it: a detach names the attachment it held, and only that one is forgotten.
    #[tokio::test]
    async fn a_closed_connection_does_not_detach_the_peer_that_replaced_it() {
        let mut session = ChannelSession::default();
        // The rows span both slots: the master, and a viewer under one client id.
        let (first_master, first_master_frames) = PeerConnection::new();
        let first = session
            .attach_master(first_master)
            .expect("case master: the first master attaches");
        // The first socket ends, so the slot is free and a second connection takes it.
        drop(first_master_frames);
        let (second_master, _second_master_frames) = PeerConnection::new();
        let second = session
            .attach_master(second_master)
            .expect("case master: the replacement attaches");
        assert_ne!(
            first, second,
            "case master: each attachment of a slot must be its own"
        );
        // The first connection's task now runs its detach.
        session.detach_master(first);
        assert!(
            session.has_master(),
            "case master: the live master must still be attached after the closed one \
             detached"
        );
        session.detach_master(second);
        assert!(
            !session.has_master(),
            "case master: the live master's own detach must forget it"
        );

        let (first_viewer, first_viewer_frames) = PeerConnection::new();
        let first = session
            .attach_viewer("viewer-1", first_viewer)
            .expect("case viewer: the first viewer attaches");
        drop(first_viewer_frames);
        let (second_viewer, _second_viewer_frames) = PeerConnection::new();
        let second = session
            .attach_viewer("viewer-1", second_viewer)
            .expect("case viewer: the replacement attaches");
        session.detach_viewer("viewer-1", first);
        assert!(
            session.viewer("viewer-1").is_some(),
            "case viewer: the live viewer must still be attached after the closed one \
             detached"
        );
        session.detach_viewer("viewer-1", second);
        assert!(
            session.viewer("viewer-1").is_none(),
            "case viewer: the live viewer's own detach must forget it"
        );
    }

    #[tokio::test]
    async fn an_answer_reaches_one_waiter_per_offer() {
        let mut session = ChannelSession::default();
        // The rows span the recipients an answer can name: one waiting, one waiting twice,
        // and one nobody waits for.
        let first = session
            .wait_for_answer("alexa-1")
            .expect("a waiter inside the cap is registered");
        let second = session
            .wait_for_answer("alexa-1")
            .expect("a second waiter inside the cap is registered");
        assert!(
            session.deliver_answer("alexa-1", "answer-1"),
            "case one waiter: an answer must reach it"
        );
        assert!(
            session.deliver_answer("alexa-1", "answer-2"),
            "case second waiter: an answer must reach it"
        );
        assert!(
            !session.deliver_answer("alexa-1", "answer-3"),
            "case no waiter left: expected no delivery"
        );
        assert!(
            !session.deliver_answer("nobody", "answer-4"),
            "case unknown recipient: expected no delivery"
        );
        let answers: Vec<String> = vec![
            second.await.expect("the second waiter is answered"),
            first.await.expect("the first waiter is answered"),
        ];
        assert_eq!(
            answers,
            vec!["answer-1".to_string(), "answer-2".to_string()],
            "case two waiters: each answer must reach exactly one, got {answers:?}"
        );
    }

    #[tokio::test]
    async fn a_peer_queue_is_bounded_and_never_blocks() {
        let (peer, queued) = PeerConnection::new();
        for index in 0..PEER_FRAME_DEPTH {
            assert!(
                peer.deliver(format!("frame-{index}")),
                "case frame {index}: a frame inside the queue depth must be taken"
            );
        }
        assert!(
            !peer.deliver("one too many".to_string()),
            "case over the queue depth: expected the frame to be refused, not queued"
        );
        drop(queued);
        assert!(
            !peer.is_open(),
            "case closed queue: a peer whose task has gone must report closed"
        );
    }
}
