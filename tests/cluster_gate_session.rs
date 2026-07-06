//! THE SESSION-PARSE MATRIX WITNESSES (§3.2): spirit's side of the criome
//! authorization observation session is a CLOSED binding matrix with no
//! default-open branch. Every rule violation is a `CriomeGateError` and every
//! fault holds the head; terminal refusals are typed outcomes; only a Granted
//! state whose grant binds the session slot AND the submitted digest
//! authorizes.
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
        component: ComponentKind::Spirit,
        digest: ObjectDigest::from_bytes(b"the submitted batch head"),
        kind: AuthorizedObjectKind::Head,
    }
}

fn binding() -> HeadSessionBinding {
    HeadSessionBinding::new(slot("session-slot"), submitted_head())
}

fn grant_for(request_slot: AuthorizationRequestSlot, digest: ObjectDigest) -> AuthorizationGrant {
    AuthorizationGrant::new(
        request_slot,
        AuthorizedObjectReference {
            component: ComponentKind::Spirit,
            digest,
            kind: AuthorizedObjectKind::Head,
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
    digest: ObjectDigest,
    status: AuthorizationStatus,
    grant: Option<AuthorizationGrant>,
) -> AuthorizationStateRecord {
    AuthorizationStateRecord::new(request_slot, digest, status, Vec::new(), grant, None)
}

/// The repaired positive: a terminal Granted state whose grant binds the
/// session slot and the submitted digest authorizes — including when it is
/// already in the submission snapshot (the fast path).
#[test]
fn granted_with_binding_grant_authorizes() {
    let binding = binding();
    let granted = state(
        slot("session-slot"),
        submitted_head().digest,
        AuthorizationStatus::Granted,
        Some(grant_for(slot("session-slot"), submitted_head().digest)),
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
        submitted_head().digest,
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
        submitted_head().digest,
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
        submitted_head().digest,
        AuthorizationStatus::Granted,
        Some(grant_for(slot("someone-elses-slot"), submitted_head().digest)),
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
        Some(grant_for(slot("session-slot"), submitted_head().digest)),
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
            submitted_head().digest,
            AuthorizationStatus::Granted,
            Some(grant_for(slot("someone-elses-slot"), submitted_head().digest)),
        ))
        .expect("a foreign record is ignored, not a fault");
    assert_eq!(verdict, None, "a foreign grant never authorizes this session");
}

/// Rule 4: terminal non-Granted states are typed refusals — outcomes, not
/// errors; the head is held.
#[test]
fn terminal_refusals_map_to_typed_refusal_decisions() {
    let binding = binding();
    let denied = state(
        slot("session-slot"),
        submitted_head().digest,
        AuthorizationStatus::Denied,
        None,
    )
    .with_denial_marker();
    let expectations = [
        (denied, GateRefusal::Denied),
        (
            state(
                slot("session-slot"),
                submitted_head().digest,
                AuthorizationStatus::Expired,
                None,
            ),
            GateRefusal::Expired,
        ),
        (
            state(
                slot("session-slot"),
                submitted_head().digest,
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
            record.status
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
                submitted_head().digest,
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
        self.denial = Some(AuthorizationDenial {
            source: AuthorizationDenialSource::Policy,
            reason: AuthorizationDenialReason::PolicyRefused,
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
                    authorization.object.digest.clone(),
                )),
                StubScript::SnapshotThenSilence => CriomeReply::AuthorizationObservationSnapshot(
                    AuthorizationObservationSnapshot::from_states(vec![state(
                        slot("stub-slot"),
                        authorization.object.digest.clone(),
                        AuthorizationStatus::Pending,
                        None,
                    )]),
                ),
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
        "deadline expiry holds the head Unreachable"
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
