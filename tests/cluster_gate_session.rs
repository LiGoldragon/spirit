//! THE SESSION-PARSE MATRIX WITNESSES (§3.2): spirit's side of the criome
//! authorization observation session is a CLOSED binding matrix with no
//! default-open branch. Every rule violation is a `CriomeGateError` and every
//! fault refuses the operation on the intake path (and withholds the ship on
//! the drain); terminal refusals are typed outcomes; only a Granted state
//! whose grant binds the session slot AND the submitted digest authorizes.
//!
//! Pure negatives run the matrix directly over crafted state records; the
//! socket-level negatives run a stub criome socket writing crafted frames:
//! the retired one-shot reply shape is held as off-contract, a dead session
//! deadline is held Unreachable, and an absent criome is held Unreachable.

use std::io::BufReader;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use criome::transport::CriomeFrameCodec;
use signal_criome::{
    AuthorizationDenial, AuthorizationDenialReason, AuthorizationDenialSource, AuthorizationGrant,
    AuthorizationObservationSnapshot, AuthorizationPolicyClass, AuthorizationPolicySatisfaction,
    AuthorizationRequestSlot, AuthorizationStateRecord, AuthorizationStatus, AuthorizedObjectKind,
    AuthorizedObjectReference, ComponentKind, CriomeReply, CriomeRequest, Identity, ObjectDigest,
    SignatureAuthorizationResult, TimestampNanos,
};
use spirit::{ClusterAuthorizer, CriomeGateError, GateDecision, GateRefusal, HeadSessionBinding};

fn slot(name: &str) -> AuthorizationRequestSlot {
    AuthorizationRequestSlot::new(name)
}

fn submitted_head() -> AuthorizedObjectReference {
    AuthorizedObjectReference {
        component_kind: ComponentKind::Spirit,
        object_digest: ObjectDigest::from_bytes(b"the submitted batch head"),
        authorized_object_kind: AuthorizedObjectKind::Head,
    }
}

fn binding() -> HeadSessionBinding {
    HeadSessionBinding::new(slot("session-slot"), submitted_head())
}

fn grant_for(
    request_slot: AuthorizationRequestSlot,
    object_digest: ObjectDigest,
) -> AuthorizationGrant {
    AuthorizationGrant::new(
        request_slot,
        AuthorizedObjectReference {
            component_kind: ComponentKind::Spirit,
            object_digest,
            authorized_object_kind: AuthorizedObjectKind::Head,
        },
        AuthorizationPolicySatisfaction::new(
            AuthorizationPolicyClass::ComplexQuorum,
            signal_criome::RequiredSignatureThreshold::new(2),
            vec![Identity::host("node-a".to_owned())],
        ),
        SignatureAuthorizationResult::RequiredSignaturesSatisfied,
        Vec::new(),
        Identity::host("node-a".to_owned()),
        TimestampNanos::new(1),
        None,
    )
}

fn state(
    request_slot: AuthorizationRequestSlot,
    object_digest: ObjectDigest,
    status: AuthorizationStatus,
    grant: Option<AuthorizationGrant>,
) -> AuthorizationStateRecord {
    AuthorizationStateRecord::new(request_slot, object_digest, status, Vec::new(), grant, None)
}

/// The repaired positive: a terminal Granted state whose grant binds the
/// session slot and the submitted digest authorizes — including when it is
/// already in the submission snapshot (the fast path).
#[test]
fn granted_with_binding_grant_authorizes() {
    let binding = binding();
    let granted = state(
        slot("session-slot"),
        submitted_head().object_digest,
        AuthorizationStatus::Granted,
        Some(grant_for(
            slot("session-slot"),
            submitted_head().object_digest,
        )),
    );
    let decision = binding
        .decide(&granted)
        .expect("a binding grant is on-contract");
    assert_eq!(
        decision,
        Some(GateDecision::Authorized(submitted_head())),
        "the bound grant releases the ship"
    );
}

/// Rule 3 negative: Granted WITHOUT a grant is a fault, never an
/// authorization — status alone is never proof.
#[test]
fn granted_without_grant_is_held_as_a_fault() {
    let verdict = binding().decide(&state(
        slot("session-slot"),
        submitted_head().object_digest,
        AuthorizationStatus::Granted,
        None,
    ));
    assert!(
        matches!(verdict, Err(CriomeGateError::HeadBindingViolation { .. })),
        "Granted-without-grant must hold the head, got {verdict:?}"
    );
}

/// Rule 3 negative: a grant whose authorized object digest differs from the
/// submitted head is a fault.
#[test]
fn grant_digest_mismatch_is_held_as_a_fault() {
    let verdict = binding().decide(&state(
        slot("session-slot"),
        submitted_head().object_digest,
        AuthorizationStatus::Granted,
        Some(grant_for(
            slot("session-slot"),
            ObjectDigest::from_bytes(b"a different object entirely"),
        )),
    ));
    assert!(
        matches!(verdict, Err(CriomeGateError::HeadBindingViolation { .. })),
        "a grant for another digest must hold the head, got {verdict:?}"
    );
}

/// Rule 3 negative: a grant bound to a foreign request slot is a fault even
/// when its digest matches.
#[test]
fn grant_slot_mismatch_is_held_as_a_fault() {
    let verdict = binding().decide(&state(
        slot("session-slot"),
        submitted_head().object_digest,
        AuthorizationStatus::Granted,
        Some(grant_for(
            slot("someone-elses-slot"),
            submitted_head().object_digest,
        )),
    ));
    assert!(
        matches!(verdict, Err(CriomeGateError::HeadBindingViolation { .. })),
        "a grant for another slot must hold the head, got {verdict:?}"
    );
}

/// Rule 2 negative: a token-bound record whose request digest differs from
/// the submitted head is a fault.
#[test]
fn request_digest_mismatch_is_held_as_a_fault() {
    let verdict = binding().decide(&state(
        slot("session-slot"),
        ObjectDigest::from_bytes(b"not the submitted head"),
        AuthorizationStatus::Granted,
        Some(grant_for(
            slot("session-slot"),
            submitted_head().object_digest,
        )),
    ));
    assert!(
        matches!(verdict, Err(CriomeGateError::HeadBindingViolation { .. })),
        "a digest-mismatched record must hold the head, got {verdict:?}"
    );
}

/// Rule 1: records for a foreign request slot are IGNORED — even a Granted
/// one never authorizes this session, and never faults it either.
#[test]
fn foreign_slot_records_are_ignored() {
    let verdict = binding()
        .decide(&state(
            slot("someone-elses-slot"),
            submitted_head().object_digest,
            AuthorizationStatus::Granted,
            Some(grant_for(
                slot("someone-elses-slot"),
                submitted_head().object_digest,
            )),
        ))
        .expect("a foreign record is ignored, not a fault");
    assert_eq!(
        verdict, None,
        "a foreign grant never authorizes this session"
    );
}

/// Rule 4: terminal non-Granted states are typed refusals — outcomes, not
/// errors; the head is held.
#[test]
fn terminal_refusals_map_to_typed_refusal_decisions() {
    let binding = binding();
    let denied = state(
        slot("session-slot"),
        submitted_head().object_digest,
        AuthorizationStatus::Denied,
        None,
    )
    .with_denial_marker();
    let expectations = [
        (denied, GateRefusal::Denied),
        (
            state(
                slot("session-slot"),
                submitted_head().object_digest,
                AuthorizationStatus::Expired,
                None,
            ),
            GateRefusal::Expired,
        ),
        (
            state(
                slot("session-slot"),
                submitted_head().object_digest,
                AuthorizationStatus::Unavailable,
                None,
            ),
            GateRefusal::Unavailable,
        ),
    ];
    for (record, refusal) in expectations {
        let decision = binding.decide(&record).expect("a refusal is on-contract");
        assert_eq!(
            decision,
            Some(GateDecision::Refused(refusal)),
            "terminal {:?} maps to the typed refusal",
            record.authorization_status
        );
    }
}

/// Rule 5: non-terminal states keep the session draining.
#[test]
fn non_terminal_states_keep_draining() {
    let binding = binding();
    for status in [
        AuthorizationStatus::Pending,
        AuthorizationStatus::Signing,
        AuthorizationStatus::Parked,
    ] {
        let verdict = binding
            .decide(&state(
                slot("session-slot"),
                submitted_head().object_digest,
                status,
                None,
            ))
            .expect("a non-terminal state is on-contract");
        assert_eq!(verdict, None, "{status:?} keeps the session draining");
    }
}

/// A test-only marker so the Denied fixture reads as a genuine denial (the
/// matrix itself never inspects the denial payload).
trait WithDenialMarker {
    fn with_denial_marker(self) -> Self;
}

impl WithDenialMarker for AuthorizationStateRecord {
    fn with_denial_marker(mut self) -> Self {
        self.optional_authorization_denial = Some(AuthorizationDenial {
            authorization_denial_source: AuthorizationDenialSource::Policy,
            authorization_denial_reason: AuthorizationDenialReason::PolicyRefused,
        });
        self
    }
}

/// A stub criome socket that accepts ONE connection, reads the request, and
/// writes exactly the frames the scenario dictates — the crafted-daemon
/// falsification harness.
struct StubCriomeSocket {
    _directory: tempfile::TempDir,
    socket_path: PathBuf,
    thread: thread::JoinHandle<()>,
}

enum StubScript {
    /// The retired one-shot shape: a bare `AuthorizationGranted` reply
    /// instead of the observation snapshot.
    BareGrantedReply,
    /// An on-contract non-terminal snapshot, then silence (a dead criome
    /// process that never pushes).
    SnapshotThenSilence,
    /// A HUNG-BUT-ACCEPTING criome: the connection is accepted and the ask
    /// is read, but the submission's snapshot reply NEVER comes (audit F2 —
    /// the read that ran before any deadline was set). The connection is
    /// held open long past the session deadline so only a bounded
    /// submission read can return early.
    AcceptThenNeverReply,
}

impl StubCriomeSocket {
    fn spawn(script: StubScript) -> Self {
        let directory = tempfile::tempdir().expect("stub criome tempdir");
        let socket_path = directory.path().join("criome.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind stub criome socket");
        let thread = thread::spawn(move || {
            let Ok((stream, _peer)) = listener.accept() else {
                return;
            };
            let mut stream = BufReader::new(stream);
            let codec = CriomeFrameCodec::default();
            let request = codec
                .read_request(&mut stream)
                .expect("stub reads the submitted ask");
            let CriomeRequest::AuthorizeSignalCall(authorization) = request else {
                panic!("expected AuthorizeSignalCall at the stub");
            };
            let reply = match script {
                StubScript::BareGrantedReply => CriomeReply::AuthorizationGranted(grant_for(
                    slot("stub-slot"),
                    authorization
                        .authorized_object_reference
                        .object_digest
                        .clone(),
                )),
                StubScript::SnapshotThenSilence => CriomeReply::AuthorizationObservationSnapshot(
                    AuthorizationObservationSnapshot::from_states(vec![state(
                        slot("stub-slot"),
                        authorization
                            .authorized_object_reference
                            .object_digest
                            .clone(),
                        AuthorizationStatus::Pending,
                        None,
                    )]),
                ),
                StubScript::AcceptThenNeverReply => {
                    // Hold the accepted connection open, writing nothing:
                    // the caller's SUBMISSION read must trip its own
                    // deadline — an unbounded read would sit here for the
                    // full hold.
                    thread::sleep(Duration::from_secs(20));
                    return;
                }
            };
            codec
                .write_reply(stream.get_mut(), reply)
                .expect("stub writes its scripted reply");
            // Hold the connection open briefly so silence (not EOF racing the
            // deadline) is what the deadline measures, then drop.
            thread::sleep(Duration::from_secs(3));
        });
        Self {
            _directory: directory,
            socket_path,
            thread,
        }
    }

    fn join(self) {
        let _ = self.thread.join();
    }
}

fn authorize_against(socket: PathBuf) -> Result<GateDecision, CriomeGateError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    runtime.block_on(async {
        let authorizer =
            ClusterAuthorizer::new(socket).with_session_deadline(Duration::from_secs(1));
        let capture = spirit::LocalHeadCapture::spirit_head(sema_engine::EntryDigest::new(
            *blake3::hash(b"stub head entry").as_bytes(),
        ));
        authorizer.authorize_head(&capture).await
    })
}

/// A daemon writing the retired one-shot `AuthorizationGranted` reply is
/// off-contract: held as a machinery fault, never authorized.
#[test]
fn bare_granted_reply_is_held_as_off_contract() {
    let stub = StubCriomeSocket::spawn(StubScript::BareGrantedReply);
    let socket = stub.socket_path.clone();
    let verdict = authorize_against(socket);
    assert!(
        matches!(verdict, Err(CriomeGateError::UnexpectedReply { .. })),
        "the retired one-shot grant shape must hold the head, got {verdict:?}"
    );
    stub.join();
}

/// A criome that goes silent after a non-terminal snapshot trips the session
/// deadline: held Unreachable, never authorized.
#[test]
fn session_deadline_expiry_is_held_unreachable() {
    let stub = StubCriomeSocket::spawn(StubScript::SnapshotThenSilence);
    let socket = stub.socket_path.clone();
    let verdict = authorize_against(socket).expect("a dead criome is an outcome, not a fault");
    assert_eq!(
        verdict,
        GateDecision::Unreachable,
        "deadline expiry is judged Unreachable — a typed refusal, fail-closed"
    );
    stub.join();
}

/// No criome at all: held Unreachable.
#[test]
fn absent_criome_is_held_unreachable() {
    let directory = tempfile::tempdir().expect("tempdir");
    let verdict = authorize_against(directory.path().join("no-criome.sock"))
        .expect("an absent criome is an outcome, not a fault");
    assert_eq!(verdict, GateDecision::Unreachable);
}

/// AUDIT F2 — the SUBMISSION legs are bounded. A hung-but-accepting criome
/// (connection accepted, ask read, snapshot never written) must trip the IO
/// deadline on the submission read and hold the head Unreachable within
/// bounded time. Pre-fix, this read ran before any deadline was set, so the
/// blocking worker (and with it the drain's serialization lock) sat for the
/// stub's full 20-second hold — unboundedly, against a truly hung daemon.
#[test]
fn hung_but_accepting_criome_is_held_unreachable_within_the_deadline() {
    let stub = StubCriomeSocket::spawn(StubScript::AcceptThenNeverReply);
    let socket = stub.socket_path.clone();
    let started = std::time::Instant::now();
    let verdict = authorize_against(socket).expect("a hung criome is an outcome, not a fault");
    assert_eq!(
        verdict,
        GateDecision::Unreachable,
        "the bounded submission read judges Unreachable — a typed refusal"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the submission read returned on its own deadline, not on the stub's 20 s hold \
         (took {:?})",
        started.elapsed()
    );
    // The stub thread is still inside its hold; do not join it — the claim
    // is precisely that the caller returns first.
}
