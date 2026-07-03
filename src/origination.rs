//! Spirit mirror ORIGINATION — hand the authorized head to the LOCAL router.
//!
//! The dual of [`crate::apply_ingress`]. After the 1-of-1 LOCAL criome gate
//! authorizes the post-commit head, Spirit projects
//! `{record_identifier, versioned entry, Evidence}` into a `signal-spirit`
//! `ApplyAuthorizedRecord` frame, wraps that frame as ONE
//! [`RoutedContractObject`], and hands it to its LOCAL router's working socket
//! as a `signal-router` `SubmitRoutedObjects` origination. The router carries
//! the object to the peer node and delivers the raw frame octets to the peer
//! Spirit's working socket, where [`crate::apply_ingress`] re-judges the carried
//! Evidence and lands the record live.
//!
//! Two invariants make the far-side apply succeed by construction:
//!
//!   - The `versioned_entry_hex` is the byte-for-byte
//!     [`crate::store::Store::versioned_log_head_object`] octets, so it re-hashes
//!     to the same head digest on the peer.
//!   - The `authorized_evidence_hex` is the SAME
//!     [`signal_criome::Evidence`] the local criome authorized, whose
//!     `operation` is the head's operation digest — so the peer's content-address
//!     binding (`evidence.operation == entry_digest`) holds.
//!
//! The router hand-off dials the router's working socket synchronously (the
//! caller wraps it in `spawn_blocking`), mirroring the criome gate's
//! [`criome::transport::CriomeClient::send`]. The origination is best-effort
//! relative to LOCAL durability: the working write already committed before the
//! gate runs, so an unreachable or refusing router leaves the local commit
//! intact and the head waits for the next authorized drain (a durable outbox and
//! redial are a later milestone).

use std::{io::Write, os::unix::net::UnixStream, path::PathBuf};

use rkyv::rancor;
use sema_engine::VersionedCommitLogEntry;
use signal_criome::Evidence;
use signal_frame::{ExchangeIdentifier, ExchangeLane, LaneSequence, Reply, SessionEpoch, SubReply};
use signal_router::{
    ActorIdentifier, ContractName, ContractOperation, ContractPayloadSize, ForwardedMessagePayload,
    Frame as RouterFrame, FrameBody as RouterFrameBody, Input as RouterInput,
    Integer as RouterInteger, Output as RouterOutput, RoutedContractObject,
};
use thiserror::Error;
use triad_runtime::{
    FrameBody as LengthPrefixedFrameBody, FrameError as LengthPrefixedFrameError,
    LengthPrefixedCodec,
};

use crate::schema::{
    sema::StoredRecord,
    signal::{
        AuthorizedEvidenceHex, AuthorizedRecordApplication, Input as SpiritInput, RecordIdentifier,
        VersionedEntryHex,
    },
};

/// The `contract_name` label stamped on the routed object. The router relays the
/// octets payload-blind; the name is an attestation/audit label naming the
/// contract the octets belong to.
const SPIRIT_CONTRACT_NAME: &str = "signal-spirit";
/// The `contract_operation` label stamped on the routed object — the operation
/// the carried frame decodes to on the peer's working socket.
const APPLY_AUTHORIZED_RECORD_OPERATION: &str = "ApplyAuthorizedRecord";

/// The deploy-config origination target: the LOCAL router's working socket and
/// the source/destination actor identities the router routes the hand-off by.
///
/// Armed alongside the 1-of-1 criome gate. Unarmed means Spirit authorizes its
/// head but does not originate a forward — the head simply lands locally, with
/// no peer. This is a data-bearing config object: the socket and actor names
/// travel with it, so its projection and hand-off methods live here.
#[derive(Clone, Debug)]
pub struct RouterOrigination {
    router_socket: PathBuf,
    source_actor: ActorIdentifier,
    destination_actor: ActorIdentifier,
}

impl RouterOrigination {
    /// Configure origination against the LOCAL router working socket. The
    /// `source_actor` names this Spirit's router identity; the
    /// `destination_actor` names the peer Spirit the router resolves a remote
    /// route to.
    pub fn new(
        router_socket: impl Into<PathBuf>,
        source_actor: ActorIdentifier,
        destination_actor: ActorIdentifier,
    ) -> Self {
        Self {
            router_socket: router_socket.into(),
            source_actor,
            destination_actor,
        }
    }

    /// The configured LOCAL router working socket.
    pub fn router_socket(&self) -> &std::path::Path {
        &self.router_socket
    }

    /// Project the authorized head into the router hand-off payload: an
    /// `ApplyAuthorizedRecord` frame carried as ONE [`RoutedContractObject`]
    /// destined for the peer Spirit.
    ///
    /// `versioned_entry_octets` are the rkyv [`VersionedCommitLogEntry`] head
    /// object (`Store::versioned_log_head_object`); `evidence` is the SAME
    /// [`Evidence`] the local criome authorized. The record identifier is decoded
    /// from the carried entry so it matches what the apply ingress recovers, by
    /// construction — the origination and the apply read the identity from the
    /// same content-addressed bytes.
    pub fn submission_for_head(
        &self,
        versioned_entry_octets: Vec<u8>,
        evidence: Evidence,
    ) -> Result<ForwardedMessagePayload, RouterOriginationError> {
        let record_identifier = Self::head_record_identifier(&versioned_entry_octets)?;
        let evidence_octets = rkyv::to_bytes::<rancor::Error>(&evidence)
            .map_err(|_| RouterOriginationError::EvidenceEncode)?
            .to_vec();
        let application = AuthorizedRecordApplication {
            record_identifier: RecordIdentifier::new(record_identifier),
            versioned_entry_hex: VersionedEntryHex::new(Self::to_hex(&versioned_entry_octets)),
            authorized_evidence_hex: AuthorizedEvidenceHex::new(Self::to_hex(&evidence_octets)),
        };
        let frame_octets = SpiritInput::apply_authorized_record(application)
            .encode_signal_frame()
            .map_err(|source| RouterOriginationError::Frame {
                message: source.to_string(),
            })?;
        let routed_object = RoutedContractObject::new(
            ContractName::new(SPIRIT_CONTRACT_NAME),
            ContractOperation::new(APPLY_AUTHORIZED_RECORD_OPERATION),
            ContractPayloadSize::new(frame_octets.len() as RouterInteger),
            frame_octets.into_iter().map(RouterInteger::from).collect(),
        );
        Ok(ForwardedMessagePayload::new(
            self.source_actor.clone(),
            self.destination_actor.clone(),
            APPLY_AUTHORIZED_RECORD_OPERATION.to_owned(),
            Vec::new(),
            vec![routed_object],
        ))
    }

    /// Hand the payload to the LOCAL router's working socket as a
    /// `SubmitRoutedObjects` origination and confirm the router accepted it.
    ///
    /// Synchronous `UnixStream` round-trip (the caller wraps it in
    /// `spawn_blocking`): write the exchange-framed request, read the router's
    /// reply, and require `RoutedObjectsAccepted`. Any transport error, a router
    /// rejection, or an off-contract reply is a typed [`RouterOriginationError`].
    pub fn hand_off(&self, payload: ForwardedMessagePayload) -> Result<(), RouterOriginationError> {
        let mut stream = UnixStream::connect(&self.router_socket)?;
        let exchange = ExchangeIdentifier::new(
            SessionEpoch::new(0),
            ExchangeLane::Connector,
            LaneSequence::first(),
        );
        let request_octets = RouterInput::submit_routed_objects(payload)
            .into_frame(exchange)
            .encode()
            .map_err(|source| RouterOriginationError::Frame {
                message: source.to_string(),
            })?;
        let codec = LengthPrefixedCodec::default();
        codec.write_body(&mut stream, &LengthPrefixedFrameBody::new(request_octets))?;
        stream.flush()?;
        let reply_body = codec.read_body(&mut stream)?;
        let reply_frame = RouterFrame::decode(reply_body.bytes()).map_err(|source| {
            RouterOriginationError::Frame {
                message: source.to_string(),
            }
        })?;
        Self::accepted(reply_frame)
    }

    /// Confirm the router reply is a `RoutedObjectsAccepted` acceptance.
    fn accepted(frame: RouterFrame) -> Result<(), RouterOriginationError> {
        match frame.into_body() {
            RouterFrameBody::Reply { reply, .. } => match reply {
                Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                    SubReply::Ok(RouterOutput::RoutedObjectsAccepted(_)) => Ok(()),
                    other => Err(RouterOriginationError::UnexpectedReply {
                        got: format!("{other:?}"),
                    }),
                },
                Reply::Rejected { reason } => Err(RouterOriginationError::Rejected {
                    reason: reason.to_string(),
                }),
            },
            other => Err(RouterOriginationError::UnexpectedReply {
                got: format!("{other:?}"),
            }),
        }
    }

    /// Decode the record identifier the carried head commits — the head
    /// operation's rkyv [`StoredRecord`] identity. Mirrors the apply ingress's
    /// own decode so origination and apply agree on the identity by construction.
    fn head_record_identifier(
        versioned_entry_octets: &[u8],
    ) -> Result<String, RouterOriginationError> {
        let versioned_entry =
            rkyv::from_bytes::<VersionedCommitLogEntry, rancor::Error>(versioned_entry_octets)
                .map_err(|_| RouterOriginationError::MalformedHead)?;
        let payload_octets = versioned_entry
            .operations()
            .head()
            .payload()
            .bytes()
            .ok_or(RouterOriginationError::MalformedHead)?;
        let stored = rkyv::from_bytes::<StoredRecord, rancor::Error>(payload_octets)
            .map_err(|_| RouterOriginationError::MalformedHead)?;
        Ok(stored.record_identifier.into_payload())
    }

    /// Lowercase hex-encode octets for the `VersionedEntryHex` /
    /// `AuthorizedEvidenceHex` carriage — the inverse of the apply ingress's
    /// `CarriedHex::octets` decode.
    fn to_hex(octets: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut text = String::with_capacity(octets.len() * 2);
        for byte in octets {
            let _ = write!(text, "{byte:02x}");
        }
        text
    }
}

/// The origination hand-off's typed failure modes. An unreachable or refusing
/// router is a real hand-off failure the daemon logs and drops (the local commit
/// already stands); a malformed head or an evidence-encode failure is a genuine
/// projection fault.
#[derive(Debug, Error)]
pub enum RouterOriginationError {
    #[error("router origination socket connect/io failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("router origination frame transport failed: {0}")]
    Transport(#[from] LengthPrefixedFrameError),

    #[error("router origination frame codec failed: {message}")]
    Frame { message: String },

    #[error("evidence archive encode failed")]
    EvidenceEncode,

    #[error("the versioned head entry is malformed")]
    MalformedHead,

    #[error("router refused the origination: {reason}")]
    Rejected { reason: String },

    #[error("router answered with an unexpected reply: {got}")]
    UnexpectedReply { got: String },
}
