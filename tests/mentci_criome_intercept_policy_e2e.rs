use std::{
    io::Write,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use criome::{daemon::CriomeDaemon, tables::StoreLocation};
use mentci::{
    configuration::DaemonConfiguration,
    daemon::{BoundDaemon, Daemon},
    frame_codec::FrameCodec,
};
use meta_signal_mentci::{
    ComponentKind as MentciComponentKind, ComponentSocket, ComponentSocketKind,
    MentciDaemonConfiguration, NotificationClient, PersonaIdentity, PersonaKeyLabel, PersonaName,
    StandardSocket,
};
use nota::NotaEncode;
use signal_agent::{
    Completion, CompletionText, Input as AgentInput, Output as AgentOutput, StopReasonText,
    TokenUsage,
};
use signal_criome::{
    AuthorizationMode, AuthorizationStatus, CriomeReply, ExpiryAction, InterceptPolicyProposal,
    InterceptTargetSelector, MentciSessionSlot, ParkedRequestOutcome, ParkedRequestQuery,
    PolicyDurationNanos, PolicyOverlapMode, PolicyPriority, SpiritOperationName,
    SpiritOperationNames, SpiritProcessKey,
};
use signal_frame::{
    ExchangeIdentifier, ExchangeLane, LaneSequence, Reply, RequestPayload, SessionEpoch, SubReply,
};
use signal_mentci::{
    ApprovalDecision, ApprovalVerdict, InterfaceInterest, MentciFrame, MentciFrameBody,
    MentciReply, MentciRequest, ProjectedInterfaceState, SubscriberName,
};
use signal_spirit::{
    ConfigurationPath, SpiritGuardianAgentConfiguration, SpiritGuardianTimeoutMilliseconds,
};
use spirit::{
    Configuration, MetaSignalTransport,
    schema::{
        meta_signal::{
            ArchiveDatabaseTarget, ConfigureRequest, CriomeGateTarget, CriomeSocketPathText,
            Output as MetaOutput,
        },
        nexus::GuardianVerdict,
        signal::Output,
    },
};
use tempfile::TempDir;
use triad_runtime::{FrameBody, LengthPrefixedCodec};

struct FakeGuardianAgent {
    socket_path: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

struct SpiritDaemonProcess {
    child: Child,
    _guardian_agent: FakeGuardianAgent,
}

struct SpiritCliProcess {
    child: Option<Child>,
}

impl Drop for FakeGuardianAgent {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for SpiritDaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for SpiritCliProcess {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl FakeGuardianAgent {
    fn spawn(socket_path: std::path::PathBuf) -> Self {
        if let Some(parent) = socket_path.parent() {
            std::fs::create_dir_all(parent).expect("create fake guardian socket directory");
        }
        let listener = UnixListener::bind(&socket_path).expect("bind fake guardian socket");
        listener
            .set_nonblocking(true)
            .expect("fake guardian listener nonblocking");
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
            .expect("chmod fake guardian socket");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _address)) => Self::answer(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept fake guardian call: {error}"),
                }
            }
        });
        Self {
            socket_path,
            stop,
            thread: Some(thread),
        }
    }

    fn answer(mut stream: UnixStream) {
        let codec = LengthPrefixedCodec::default();
        let request = codec
            .read_body(&mut stream)
            .expect("read fake guardian request")
            .into_bytes();
        let (_route, input) =
            AgentInput::decode_signal_frame(&request).expect("decode fake guardian input");
        let AgentInput::Call(_) = input else {
            panic!("expected guardian Call input, got {input:?}");
        };
        let output = AgentOutput::completed(Completion {
            completion_text: CompletionText::new(GuardianVerdict::Accept.to_nota()),
            stop_reason: StopReasonText::new("stop"),
            token_usage: TokenUsage::new(None, None),
        });
        codec
            .write_body(
                &mut stream,
                &FrameBody::new(
                    output
                        .encode_signal_frame()
                        .expect("encode fake guardian output"),
                ),
            )
            .expect("write fake guardian output");
        stream.flush().expect("flush fake guardian output");
    }

    fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl SpiritDaemonProcess {
    fn spawn(socket_path: &Path, database_path: &Path, guardian_socket_path: &Path) -> Self {
        let guardian_agent = FakeGuardianAgent::spawn(guardian_socket_path.to_path_buf());
        let configuration_path = socket_path.with_extension("config.rkyv");
        let meta_socket_path = Self::meta_socket_path(socket_path);
        let configuration =
            Configuration::new(socket_path, database_path).with_meta_socket_path(&meta_socket_path);
        let configuration = Configuration::from_raw(
            configuration
                .raw()
                .clone()
                .with_guardian_agent_configuration(SpiritGuardianAgentConfiguration::new(
                    ConfigurationPath::new(guardian_agent.socket_path().display().to_string()),
                    None,
                    None,
                    SpiritGuardianTimeoutMilliseconds::new(5_000),
                    None,
                )),
        );
        configuration
            .write_binary_file(&configuration_path)
            .expect("write Spirit binary configuration");
        let child = Command::new(env!("CARGO_BIN_EXE_spirit-daemon"))
            .arg(configuration_path)
            .spawn()
            .expect("spawn Spirit daemon");
        let process = Self {
            child,
            _guardian_agent: guardian_agent,
        };
        wait_for_socket(socket_path);
        wait_for_socket(&meta_socket_path);
        process
    }

    fn meta_socket_path(socket_path: &Path) -> std::path::PathBuf {
        socket_path.with_extension("meta.sock")
    }
}

#[test]
fn guardian_allowed_spirit_operation_parks_in_criome_and_is_approved_through_mentci() {
    let temporary = TempDir::new().expect("tempdir");
    let criome_socket = temporary.path().join("criome.sock");
    let criome_meta_socket = temporary.path().join("criome-meta.sock");
    let criome_store = StoreLocation::new(temporary.path().join("criome.sema"));
    let mentci_socket = temporary.path().join("mentci.sock");
    let spirit_socket = temporary.path().join("spirit.sock");
    let spirit_database = temporary.path().join("spirit.sema");
    let guardian_socket = temporary.path().join("guardian.sock");

    let criome = CriomeDaemon::new(&criome_socket, criome_store.clone())
        .with_meta_socket(&criome_meta_socket)
        .with_authorization_mode(AuthorizationMode::AutoApprove)
        .bind()
        .expect("bind criome");
    let mentci =
        Daemon::from_configuration(mentci_configuration(&mentci_socket, &criome_meta_socket))
            .expect("mentci daemon")
            .bind()
            .expect("bind mentci");
    let _spirit = SpiritDaemonProcess::spawn(&spirit_socket, &spirit_database, &guardian_socket);

    configure_spirit_criome_gate(&spirit_socket, &criome_socket);

    let (created_policy, _criome_created_policy) = send_mentci_with_criome_meta(
        &criome,
        &mentci,
        &mentci_socket,
        MentciRequest::CreateInterceptPolicy(intercept_policy_proposal(
            "mentci-e2e-session",
            "spirit-process-main",
            "Record",
        )),
    );
    let MentciReply::InterceptPolicyCreated(policy) = created_policy else {
        panic!("expected InterceptPolicyCreated, got {created_policy:?}");
    };
    assert_eq!(policy.session_slot.as_str(), "mentci-e2e-session");

    let spirit_record = record_nota(
        "[(Technology (Software (Intelligence AgentSystems)))]",
        "Constraint",
        "guardian allowed e2e intercept policy path",
    );
    let spirit_cli = spawn_spirit_cli(&spirit_socket, &spirit_record);
    let criome_reply = criome.serve_next().expect("serve Spirit auth");
    assert!(matches!(criome_reply, CriomeReply::AuthorizationPending(_)));

    let (observed, _criome_observations) = send_mentci_with_criome_meta_replies(
        &criome,
        &mentci,
        &mentci_socket,
        MentciRequest::ObserveInterfaceState(signal_mentci::InterfaceStateObservation {
            subscriber: SubscriberName::new("mentci-e2e"),
            interest: InterfaceInterest::PendingQuestions,
        }),
        2,
    );
    let MentciReply::InterfaceObservationOpened(opened) = observed else {
        panic!("expected InterfaceObservationOpened, got {observed:?}");
    };
    let questions = opened.state.pending_questions();
    assert_eq!(questions.len(), 1);
    let question = &questions[0];
    assert_eq!(
        question
            .proposal
            .source
            .parked_request()
            .map(|identifier| identifier.as_str()),
        Some("0")
    );
    assert_question_context(&opened.state, "spirit-target", "spirit-process-main");
    assert_question_context(&opened.state, "spirit-operation", "Record");
    assert_question_context_contains(
        &opened.state,
        "raw-spirit-payload",
        "guardian allowed e2e intercept policy path",
    );

    let verdict = ApprovalVerdict {
        question: question.identifier.clone(),
        decision: ApprovalDecision::ApproveSuggestedAnswer,
        answered_by: SubscriberName::new("psyche"),
    };
    let (answer_reply, criome_answer) = send_mentci_with_criome_meta(
        &criome,
        &mentci,
        &mentci_socket,
        MentciRequest::AnswerQuestion(verdict),
    );
    assert!(matches!(answer_reply, MentciReply::VerdictAccepted(_)));
    let meta_signal_criome::Output::ParkedRequestAnswered(answered) = criome_answer else {
        panic!("expected ParkedRequestAnswered, got {criome_answer:?}");
    };
    assert_eq!(answered.identifier.as_str(), "0");
    assert_eq!(answered.outcome, ParkedRequestOutcome::Approved);

    let fetched_after_approval = send_mentci_with_criome_meta(
        &criome,
        &mentci,
        &mentci_socket,
        MentciRequest::FetchParkedRequests(ParkedRequestQuery {
            session_slot: None,
            target: None,
        }),
    )
    .0;
    let MentciReply::ParkedRequestsFetched(snapshot) = fetched_after_approval else {
        panic!("expected ParkedRequestsFetched, got {fetched_after_approval:?}");
    };
    assert!(snapshot.requests().is_empty());

    let mut spirit_cli = spirit_cli;
    let early_spirit_output = spirit_cli.try_finish(Duration::from_secs(1));
    assert!(
        early_spirit_output.is_none(),
        "gating Spirit operation should still be waiting for criome observation after approval, got {early_spirit_output:?}"
    );

    let observed_after_approval = criome
        .serve_next()
        .expect("serve Spirit authorization observation after approval");
    let CriomeReply::AuthorizationObservationSnapshot(observed_after_approval) =
        observed_after_approval
    else {
        panic!("expected AuthorizationObservationSnapshot, got {observed_after_approval:?}");
    };
    let authorization_states = observed_after_approval.into_states();
    assert_eq!(authorization_states.len(), 1);
    assert_eq!(authorization_states[0].status, AuthorizationStatus::Granted);

    let implied_referent_authorization = criome
        .serve_next()
        .expect("serve implied referent authorization after approved record");
    assert!(
        matches!(
            implied_referent_authorization,
            CriomeReply::AuthorizationGranted(_)
        ),
        "expected implied referent authorization grant, got {implied_referent_authorization:?}"
    );
    let spirit_output = spirit_cli.finish(Duration::from_secs(5));
    assert!(
        matches!(spirit_output, Output::RecordAccepted(_)),
        "gating Spirit operation should resume after parked approval, got {spirit_output:?}"
    );

    mentci.shutdown().expect("shutdown mentci");
    criome.shutdown().expect("shutdown criome");
}

fn configure_spirit_criome_gate(spirit_socket: &Path, criome_socket: &Path) {
    let mut transport =
        MetaSignalTransport::connect(SpiritDaemonProcess::meta_socket_path(spirit_socket))
            .expect("connect Spirit meta socket");
    let (_route, output) = transport
        .configure(
            ConfigureRequest::new(
                ArchiveDatabaseTarget::Default,
                None,
                Some(CriomeGateTarget::socket(CriomeSocketPathText::new(
                    criome_socket.display().to_string(),
                ))),
            )
            .into(),
        )
        .expect("configure Spirit criome gate");
    assert!(matches!(output, MetaOutput::Configured(_)));
}

fn mentci_configuration(socket: &Path, criome_meta_socket: &Path) -> DaemonConfiguration {
    DaemonConfiguration::new(MentciDaemonConfiguration::new(
        vec![
            ComponentSocket::new(
                ComponentSocketKind::Mentci,
                StandardSocket::unix(socket.display().to_string()),
            ),
            ComponentSocket::new(
                ComponentSocketKind::MetaCriome,
                StandardSocket::unix(criome_meta_socket.display().to_string()),
            ),
        ],
        PersonaIdentity::new(
            PersonaName::new("psyche"),
            MentciComponentKind::Persona,
            PersonaKeyLabel::new("home-verdict"),
        ),
        vec![NotificationClient::StatusBar],
    ))
}

fn intercept_policy_proposal(
    session: &str,
    target: &str,
    operation: &str,
) -> InterceptPolicyProposal {
    InterceptPolicyProposal {
        session_slot: MentciSessionSlot::new(session),
        target: InterceptTargetSelector::new(SpiritProcessKey::new(target)),
        spirit_operation_names: SpiritOperationNames::from_names(vec![SpiritOperationName::new(
            operation,
        )]),
        duration: PolicyDurationNanos::new(9_000_000_000_000_000_000),
        expiry_action: ExpiryAction::LeaveParked,
        priority: PolicyPriority::new(50),
        overlap_mode: PolicyOverlapMode::RejectSamePriorityOverlap,
    }
}

fn send_mentci_with_criome_meta(
    criome: &criome::daemon::BoundCriomeDaemon,
    mentci: &BoundDaemon,
    mentci_socket: &Path,
    request: MentciRequest,
) -> (MentciReply, meta_signal_criome::Output) {
    let (reply, mut meta_replies) =
        send_mentci_with_criome_meta_replies(criome, mentci, mentci_socket, request, 1);
    (reply, meta_replies.remove(0))
}

fn send_mentci_with_criome_meta_replies(
    criome: &criome::daemon::BoundCriomeDaemon,
    mentci: &BoundDaemon,
    mentci_socket: &Path,
    request: MentciRequest,
    meta_reply_count: usize,
) -> (MentciReply, Vec<meta_signal_criome::Output>) {
    thread::scope(|scope| {
        let criome_meta_server = scope.spawn(|| {
            let mut replies = Vec::new();
            for _ in 0..meta_reply_count {
                replies.push(criome.serve_next_meta().expect("serve criome meta"));
            }
            replies
        });
        let mentci_server = scope.spawn(|| mentci.serve_next().expect("serve mentci"));
        let reply = send_mentci(mentci_socket, request);
        let meta_replies = criome_meta_server.join().expect("join criome meta");
        mentci_server.join().expect("join mentci");
        (reply, meta_replies)
    })
}

fn send_mentci(socket: &Path, request: MentciRequest) -> MentciReply {
    let codec = FrameCodec::new();
    let mut stream = UnixStream::connect(socket).expect("connect mentci");
    let frame = MentciFrame::new(MentciFrameBody::Request {
        exchange: exchange(),
        request: request.into_request(),
    });
    codec
        .write_mentci_frame(&mut stream, &frame)
        .expect("write mentci request");
    let reply = codec
        .read_mentci_frame(&mut stream)
        .expect("read mentci reply");
    match reply.into_body() {
        MentciFrameBody::Reply { reply, .. } => match reply {
            Reply::Accepted { per_operation, .. } => match per_operation.into_head() {
                SubReply::Ok(payload) => payload,
                other => panic!("expected Mentci Ok reply, got {other:?}"),
            },
            Reply::Rejected { reason } => panic!("unexpected Mentci rejection: {reason:?}"),
        },
        other => panic!("expected Mentci reply frame, got {other:?}"),
    }
}

fn spawn_spirit_cli(socket_path: &Path, nota_argument: &str) -> SpiritCliProcess {
    let child = Command::new(env!("CARGO_BIN_EXE_spirit"))
        .env("SPIRIT_SOCKET", socket_path)
        .arg(nota_argument)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run Spirit CLI");
    SpiritCliProcess { child: Some(child) }
}

impl SpiritCliProcess {
    fn try_finish(&mut self, timeout: Duration) -> Option<Output> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let child = self
                .child
                .as_mut()
                .expect("Spirit CLI child already waited");
            if child.try_wait().expect("poll Spirit CLI").is_some() {
                let child = self.child.take().expect("Spirit CLI child already waited");
                return Some(parse_spirit_cli_output(
                    child.wait_with_output().expect("collect Spirit CLI output"),
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn finish(mut self, timeout: Duration) -> Output {
        if let Some(output) = self.try_finish(timeout) {
            return output;
        }
        panic!("Spirit CLI did not finish within {timeout:?}");
    }
}

fn parse_spirit_cli_output(output: std::process::Output) -> Output {
    assert!(
        output.status.success(),
        "Spirit CLI stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("Spirit CLI stdout is UTF-8");
    stdout.trim().parse::<Output>().unwrap_or_else(|error| {
        panic!(
            "schema-emitted Output::FromStr on Spirit CLI stdout {:?}: {error}",
            stdout.trim()
        )
    })
}

fn record_nota(domains: &str, kind: &str, description: &str) -> String {
    format!(
        "(Record (({domains} {kind} [{description}] Maximum Minimum Zero [mentci-criome-intercept-e2e]) ([([{description}] None)] [{description}])))"
    )
}

fn assert_question_context(state: &ProjectedInterfaceState, label: &str, expected: &str) {
    assert!(
        state.pending_questions().iter().any(|question| {
            question
                .proposal
                .context()
                .iter()
                .any(|context| context.label.as_str() == label && context.body.as_str() == expected)
        }),
        "missing context {label}={expected} in {state:?}"
    );
}

fn assert_question_context_contains(state: &ProjectedInterfaceState, label: &str, expected: &str) {
    assert!(
        state.pending_questions().iter().any(|question| {
            question.proposal.context().iter().any(|context| {
                context.label.as_str() == label && context.body.as_str().contains(expected)
            })
        }),
        "missing context {label} containing {expected} in {state:?}"
    );
}

fn wait_for_socket(socket: &Path) {
    for _ in 0..100 {
        if socket.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("socket did not appear: {}", socket.display());
}

fn exchange() -> ExchangeIdentifier {
    ExchangeIdentifier::new(
        SessionEpoch::new(1),
        ExchangeLane::Connector,
        LaneSequence::first(),
    )
}
