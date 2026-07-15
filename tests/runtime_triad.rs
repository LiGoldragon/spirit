mod support;

#[cfg(feature = "nota-text")]
use spirit::schema::signal::{Export, Import, NotaEncode};
use spirit::{
    Engine, MailIdentifier, MailLedger, MailLedgerEvent, MessageIdentifier, MessageProcessed,
    MessageSent, MessageSentHook, Nexus, OriginRoute, SentMail, ShortHeader, SignalAdmission,
    Store,
    schema::{
        nexus::{self, CommandSemaWrite, NexusAction, NexusEffectCommand, NexusEngine, NexusWork},
        sema::{
            self, ReadInput as SemaReadInput, ReadOutput as SemaReadOutput, SemaEngine,
            WriteInput as SemaWriteInput, WriteOutput as SemaWriteOutput,
        },
        signal::{
            Certainty, CertaintyChange, CertaintySelection, Clarification, DataLeaf,
            DatabaseMarker, Description, Domain, DomainMatch, DomainScope, DomainScopes, Domains,
            Entry, ErrorMessage, ErrorReport, GuardianRejectionReason, HardwareLeaf,
            ImportanceBump, ImportanceSelection, Information, Input, Justification, Keyword,
            KeywordMatch, Keywords, Kind, Magnitude, Output, Privacy, PrivacySelection, Proposal,
            Query, QuoteText, Reasoning, RecordChange, RecordIdentifier, RecordRequest,
            RecordSelection, Referent, ReferentRegistration, ReferentSelection, Referents,
            Replacements, RetiredIdentifier, RetiredIdentifiers, Retirement, SearchText,
            SelectedKind, SemaReceipt, SignalRejection, Software, StashHandle, Statement,
            StatementText, Supersession, Technology, Testimony, TextMatch, ValidationError,
            VerbatimQuote,
        },
    },
};
use support::domain_fixtures;
use tempfile::TempDir;

#[cfg(all(feature = "agent-guardian", feature = "mirror-shipper"))]
use {
    criome::transport::CriomeFrameCodec,
    signal_criome::{
        AuthorizationObservationSnapshot, AuthorizationRequestSlot, AuthorizationStateRecord,
        AuthorizationStatus, CriomeReply, CriomeRequest, ObjectDigest,
    },
    spirit::schema::meta_signal::{
        ArchiveDatabaseTarget, ConfigureRequest, CriomeGateTarget, CriomeSocketPathText,
        Output as MetaOutput,
    },
    std::io::ErrorKind,
    std::time::Instant,
};

#[cfg(feature = "agent-guardian")]
use {
    signal_frame::{ExchangeFrameBody, NonEmpty, Reply, ShortHeader as JudgeShortHeader, SubReply},
    signal_spirit_judge::{
        AdmissionJudgePacket, AdmissionJudgeResponse, AdmissionJudgeVerdict,
        AdmissionRejectionReason, JudgeDiagnostic, RedactedText, ReferentRegistrationJudgeResponse,
        ReferentRegistrationJudgeVerdict, ReferentRegistrationRejectionReason, SpiritJudgeFrame,
        SpiritJudgeReply, SpiritJudgeRequest,
    },
    spirit::{
        AgentGuardian, AgentGuardianConfiguration,
        schema::nexus::{GuardianVerdict, ReferentGuardianVerdict, Reject, RejectReferent},
        schema::signal::ReferentGuardianRejectionReason,
    },
    std::{
        io::{Read, Write},
        os::unix::net::{UnixListener, UnixStream},
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    },
};

struct SemaFile {
    #[allow(dead_code)]
    directory: TempDir,
    path: std::path::PathBuf,
}

impl SemaFile {
    fn new() -> Self {
        let directory = TempDir::new().expect("tempdir");
        let path = directory.path().join("runtime-triad.sema");
        Self { directory, path }
    }

    fn open_store(&self) -> Store {
        Store::open(&self.path).expect("open sema store")
    }

    fn open_archive_store(&self) -> Store {
        let archive_path = self.path.with_file_name("runtime-triad.archive.sema");
        Store::open(archive_path).expect("open archive sema store")
    }

    fn engine(&self) -> Engine {
        Engine::new(self.open_store())
    }

    #[cfg(feature = "agent-guardian")]
    fn engine_with_guardian(&self, guardian: AgentGuardian) -> Engine {
        let mut engine = Engine::new(self.open_store());
        engine.set_guardian(guardian);
        engine
    }
}

struct SentHookProbe {
    events: Vec<MailLedgerEvent>,
}

#[cfg(all(feature = "agent-guardian", feature = "mirror-shipper"))]
struct FakeCriomeAuthorizationSocket {
    _directory: TempDir,
    socket_path: std::path::PathBuf,
    captured_requests: Arc<Mutex<Vec<CriomeRequest>>>,
    thread: thread::JoinHandle<()>,
}

#[cfg(feature = "agent-guardian")]
struct FakeSpiritJudge {
    _directory: TempDir,
    socket_path: std::path::PathBuf,
    captured_requests: Arc<Mutex<Vec<SpiritJudgeRequest>>>,
    thread: thread::JoinHandle<()>,
}

#[cfg(feature = "agent-guardian")]
impl FakeSpiritJudge {
    fn spawn(verdict: GuardianVerdict) -> Self {
        Self::spawn_replies(vec![Self::admission_reply(verdict)])
    }

    fn spawn_many(verdicts: Vec<GuardianVerdict>) -> Self {
        Self::spawn_replies(verdicts.into_iter().map(Self::admission_reply).collect())
    }

    fn spawn_referent(verdict: ReferentGuardianVerdict) -> Self {
        Self::spawn_replies(vec![Self::referent_reply(verdict)])
    }

    fn spawn_replies(replies: Vec<SpiritJudgeReply>) -> Self {
        let directory = TempDir::new().expect("judge tempdir");
        let socket_path = directory.path().join("spirit-judge.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake spirit judge socket");
        let captured_requests = Arc::new(Mutex::new(Vec::new()));
        let thread_captured_requests = Arc::clone(&captured_requests);
        let thread = thread::spawn(move || {
            let mut replies = std::collections::VecDeque::from(replies);
            while !replies.is_empty() {
                let (mut stream, _) = listener.accept().expect("accept spirit judge call");
                let request_frame = FrameIo::new(&mut stream).read_frame();
                let ExchangeFrameBody::Request { exchange, request } = request_frame.into_body()
                else {
                    panic!("expected spirit judge request frame");
                };
                let request_payload = request.payloads().head().clone();
                let reply = Self::reply_for_request(&request_payload, &mut replies);
                thread_captured_requests
                    .lock()
                    .expect("capture judge requests")
                    .push(request_payload);
                let reply_frame = SpiritJudgeFrame::with_short_header(
                    JudgeShortHeader::empty(),
                    ExchangeFrameBody::Reply {
                        exchange,
                        reply: Reply::committed(
                            NonEmpty::try_from_vec(vec![SubReply::Ok(reply)])
                                .expect("reply list is non-empty"),
                        ),
                    },
                );
                FrameIo::new(&mut stream).write_frame(&reply_frame);
            }
        });
        Self {
            _directory: directory,
            socket_path,
            captured_requests,
            thread,
        }
    }

    fn admission_reply(verdict: GuardianVerdict) -> SpiritJudgeReply {
        let (verdict, diagnostic) = match verdict {
            GuardianVerdict::Accept => (
                AdmissionJudgeVerdict::Accept,
                Self::diagnostic("accepted by typed fake judge"),
            ),
            GuardianVerdict::Reject(rejection) => (
                AdmissionJudgeVerdict::Reject(Self::admission_rejection_reason(
                    &rejection.guardian_rejection_reason,
                )),
                Self::diagnostic(rejection.explanation.payload()),
            ),
        };
        SpiritJudgeReply::AdmissionJudged(AdmissionJudgeResponse::new(verdict, diagnostic))
    }

    fn referent_reply(verdict: ReferentGuardianVerdict) -> SpiritJudgeReply {
        let (verdict, diagnostic) = match verdict {
            ReferentGuardianVerdict::Accept => (
                ReferentRegistrationJudgeVerdict::Accept,
                Self::diagnostic("accepted by typed fake judge"),
            ),
            ReferentGuardianVerdict::RejectReferent(rejection) => (
                ReferentRegistrationJudgeVerdict::RejectReferent(Self::referent_rejection_reason(
                    &rejection.referent_guardian_rejection_reason,
                )),
                Self::diagnostic(rejection.explanation.payload()),
            ),
        };
        SpiritJudgeReply::ReferentRegistrationJudged(ReferentRegistrationJudgeResponse::new(
            verdict, diagnostic,
        ))
    }

    fn diagnostic(text: &str) -> JudgeDiagnostic {
        JudgeDiagnostic::redacted(
            RedactedText::new(if text.is_empty() {
                "typed judge reply"
            } else {
                text
            })
            .expect("diagnostic text is non-empty"),
        )
    }

    fn admission_rejection_reason(reason: &GuardianRejectionReason) -> AdmissionRejectionReason {
        match reason {
            GuardianRejectionReason::Duplicate => AdmissionRejectionReason::Duplicate,
            GuardianRejectionReason::Contradiction => AdmissionRejectionReason::Contradiction,
            GuardianRejectionReason::Compound => AdmissionRejectionReason::Compound,
            GuardianRejectionReason::NonIntent => AdmissionRejectionReason::NonIntent,
            GuardianRejectionReason::NegativeGuideline => {
                AdmissionRejectionReason::NegativeGuideline
            }
            GuardianRejectionReason::Matter => AdmissionRejectionReason::Matter,
            GuardianRejectionReason::UnclearPrivacy => AdmissionRejectionReason::UnclearPrivacy,
            GuardianRejectionReason::UnclearDomain => AdmissionRejectionReason::UnclearDomain,
            GuardianRejectionReason::ClarifyTramples => AdmissionRejectionReason::ClarifyTramples,
            GuardianRejectionReason::ClarifyLosesMeaning => {
                AdmissionRejectionReason::ClarifyLosesMeaning
            }
            GuardianRejectionReason::SupersedeTargetMissing => {
                AdmissionRejectionReason::SupersedeTargetMissing
            }
            GuardianRejectionReason::RetrievalInsufficient => {
                AdmissionRejectionReason::RetrievalInsufficient
            }
            GuardianRejectionReason::MissingTestimony => AdmissionRejectionReason::MissingTestimony,
            GuardianRejectionReason::TestimonyFabricated => {
                AdmissionRejectionReason::TestimonyFabricated
            }
            GuardianRejectionReason::InsufficientWarrant => {
                AdmissionRejectionReason::InsufficientWarrant
            }
            GuardianRejectionReason::Overstated => AdmissionRejectionReason::Overstated,
            GuardianRejectionReason::ImportanceUnsupported => {
                AdmissionRejectionReason::ImportanceUnsupported
            }
            GuardianRejectionReason::HarnessUnavailable => {
                AdmissionRejectionReason::JudgeUnavailable
            }
            GuardianRejectionReason::HarnessMalformed => AdmissionRejectionReason::JudgeMalformed,
            GuardianRejectionReason::HarnessTimedOut => AdmissionRejectionReason::JudgeTimedOut,
        }
    }

    fn referent_rejection_reason(
        reason: &ReferentGuardianRejectionReason,
    ) -> ReferentRegistrationRejectionReason {
        match reason {
            ReferentGuardianRejectionReason::Duplicate => {
                ReferentRegistrationRejectionReason::Duplicate
            }
            ReferentGuardianRejectionReason::Ambiguous => {
                ReferentRegistrationRejectionReason::Ambiguous
            }
            ReferentGuardianRejectionReason::TooVague => {
                ReferentRegistrationRejectionReason::TooVague
            }
            ReferentGuardianRejectionReason::AliasCollision => {
                ReferentRegistrationRejectionReason::AliasCollision
            }
            ReferentGuardianRejectionReason::NonReferent => {
                ReferentRegistrationRejectionReason::NonReferent
            }
            ReferentGuardianRejectionReason::UnclearJustification => {
                ReferentRegistrationRejectionReason::UnclearJustification
            }
            ReferentGuardianRejectionReason::HarnessUnavailable => {
                ReferentRegistrationRejectionReason::JudgeUnavailable
            }
            ReferentGuardianRejectionReason::HarnessMalformed => {
                ReferentRegistrationRejectionReason::JudgeMalformed
            }
            ReferentGuardianRejectionReason::HarnessTimedOut => {
                ReferentRegistrationRejectionReason::JudgeTimedOut
            }
        }
    }

    fn reply_for_request(
        request: &SpiritJudgeRequest,
        replies: &mut std::collections::VecDeque<SpiritJudgeReply>,
    ) -> SpiritJudgeReply {
        match (request, replies.front()) {
            (SpiritJudgeRequest::JudgeAdmission(_), Some(SpiritJudgeReply::AdmissionJudged(_)))
            | (
                SpiritJudgeRequest::JudgeReferentRegistration(_),
                Some(SpiritJudgeReply::ReferentRegistrationJudged(_)),
            ) => replies.pop_front().expect("front reply exists"),
            (SpiritJudgeRequest::JudgeReferentRegistration(_), _) => {
                Self::referent_reply(ReferentGuardianVerdict::Accept)
            }
            other => panic!("fake judge reply does not match request kind: {other:?}"),
        }
    }

    fn guardian(&self) -> AgentGuardian {
        AgentGuardian::new(AgentGuardianConfiguration::new(
            self.socket_path.clone(),
            None,
            None,
            Duration::from_secs(5),
            None,
        ))
    }

    fn join(self) -> Vec<SpiritJudgeRequest> {
        self.thread.join().expect("fake spirit judge joins");
        self.captured_requests
            .lock()
            .expect("read captured judge requests")
            .clone()
    }
}

#[cfg(feature = "agent-guardian")]
struct FrameIo<'stream> {
    stream: &'stream mut UnixStream,
}

#[cfg(feature = "agent-guardian")]
impl<'stream> FrameIo<'stream> {
    fn new(stream: &'stream mut UnixStream) -> Self {
        Self { stream }
    }

    fn read_frame(&mut self) -> SpiritJudgeFrame {
        let mut prefix = [0_u8; 4];
        self.stream
            .read_exact(&mut prefix)
            .expect("read judge frame prefix");
        let length = u32::from_be_bytes(prefix) as usize;
        let mut bytes = Vec::with_capacity(4 + length);
        bytes.extend_from_slice(&prefix);
        bytes.resize(4 + length, 0);
        self.stream
            .read_exact(&mut bytes[4..])
            .expect("read judge frame body");
        SpiritJudgeFrame::decode_length_prefixed(bytes.as_slice()).expect("decode judge frame")
    }

    fn write_frame(&mut self, frame: &SpiritJudgeFrame) {
        let bytes = frame.encode_length_prefixed().expect("encode judge frame");
        self.stream
            .write_all(bytes.as_slice())
            .expect("write judge frame");
        self.stream.flush().expect("flush judge frame");
    }
}

#[cfg(feature = "agent-guardian")]
fn only_admission_packet(requests: &[SpiritJudgeRequest]) -> &AdmissionJudgePacket {
    assert_eq!(
        requests.len(),
        1,
        "expected exactly one typed admission judge request"
    );
    let SpiritJudgeRequest::JudgeAdmission(packet) = &requests[0] else {
        panic!(
            "expected typed admission judge request, got {:?}",
            requests[0]
        );
    };
    packet
}

#[cfg(all(feature = "agent-guardian", feature = "mirror-shipper"))]
impl FakeCriomeAuthorizationSocket {
    fn spawn_pending() -> Self {
        let directory = TempDir::new().expect("criome fake tempdir");
        let socket_path = directory.path().join("criome.sock");
        let listener = UnixListener::bind(&socket_path).expect("bind fake criome socket");
        listener
            .set_nonblocking(true)
            .expect("fake criome listener nonblocking");
        let captured_requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&captured_requests);
        let thread = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let codec = CriomeFrameCodec::default();
                        let request = codec
                            .read_request(&mut stream)
                            .expect("fake criome reads request");
                        let reply = Self::pending_reply(&request);
                        thread_requests
                            .lock()
                            .expect("fake criome captured requests")
                            .push(request);
                        codec
                            .write_reply(&mut stream, reply)
                            .expect("fake criome writes reply");
                        return;
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fake criome accept failed: {error}"),
                }
            }
        });
        Self {
            _directory: directory,
            socket_path,
            captured_requests,
            thread,
        }
    }

    /// The observation-session submission reply: a snapshot whose one state
    /// is the parked (non-terminal) request. The stub then closes the stream,
    /// so a Gating caller observes the park and blocks rather than waiting on
    /// pushes that never come.
    fn pending_reply(request: &CriomeRequest) -> CriomeReply {
        let request_digest = match request {
            CriomeRequest::AuthorizeSignalCall(authorization) => {
                authorization.object.digest.clone()
            }
            _ => ObjectDigest::from_bytes(b"unexpected-spirit-operation-request"),
        };
        let slot = AuthorizationRequestSlot::new("fake-spirit-slot");
        CriomeReply::AuthorizationObservationSnapshot(
            AuthorizationObservationSnapshot::from_states(vec![AuthorizationStateRecord::new(
                slot,
                request_digest,
                AuthorizationStatus::Parked,
                Vec::new(),
                None,
                None,
            )]),
        )
    }

    fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    fn join(self) -> Vec<CriomeRequest> {
        let captured_requests = Arc::clone(&self.captured_requests);
        self.thread.join().expect("join fake criome");
        captured_requests
            .lock()
            .expect("fake criome captured requests")
            .clone()
    }
}

impl MessageSentHook for SentHookProbe {
    type Error = std::convert::Infallible;

    fn message_sent(&mut self, event: MessageSent) -> Result<(), Self::Error> {
        self.events.push(event.into_mail_ledger_event());
        Ok(())
    }
}

fn entry(description: &str) -> Entry {
    entry_with_domains(&["runtime-triad"], description)
}

fn entry_with_domains(domains: &[&str], description: &str) -> Entry {
    Entry {
        domains: domains_from_slice(domains),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    }
}

#[cfg(feature = "agent-guardian")]
fn entry_without_referents(description: &str) -> Entry {
    Entry {
        domains: domains_from_slice(&["runtime-triad"]),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Magnitude::Zero.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: Referents::new(Vec::new()),
    }
}

fn entry_with_domain(domain: Domain, description: &str) -> Entry {
    Entry {
        domains: Domains::new(vec![domain]),
        kind: Kind::Decision,
        description: Description::new(description),
        certainty: Magnitude::Maximum.into(),
        importance: Magnitude::Minimum.into(),
        privacy: Privacy::new(Magnitude::Zero),
        referents: spirit::schema::signal::Referents::new(vec![
            spirit::schema::signal::Referent::new("spirit"),
        ]),
    }
}

fn entry_with_privacy(description: &str, privacy: Magnitude) -> Entry {
    Entry {
        privacy: Privacy::new(privacy),
        ..entry(description)
    }
}

fn entry_with_importance(description: &str, importance: Magnitude) -> Entry {
    Entry {
        importance: importance.into(),
        ..entry(description)
    }
}

fn entry_with_referents(description: &str, referents: &[&str]) -> Entry {
    Entry {
        referents: referents_from_slice(referents),
        ..entry(description)
    }
}

fn justification(statement: &str) -> Justification {
    Justification {
        testimony: Testimony::new(vec![VerbatimQuote::new(
            QuoteText::new(statement.to_owned()),
            None,
        )]),
        reasoning: Reasoning::new(statement.to_owned()),
    }
}

fn record_request(entry: Entry) -> RecordRequest {
    let statement = entry.description.payload().clone();
    RecordRequest {
        entry,
        justification: justification(&statement),
    }
}

fn record_identifier(code: &str) -> RecordIdentifier {
    RecordIdentifier::new(code)
}

fn domains_from_slice(domains: &[&str]) -> Domains {
    domain_fixtures::domains(domains)
}

fn domain_scopes_from_slice(domains: &[&str]) -> DomainScopes {
    domain_fixtures::scopes(domains)
}

fn software_scope() -> DomainScope {
    data_scope()
}

fn technology_scope() -> DomainScope {
    DomainScope::All
}

fn schema_evolution_scope() -> DomainScope {
    DomainScope::from(Domain::Technology(Technology::Software(Software::Data(
        DataLeaf::SchemaEvolution,
    ))))
}

fn data_scope() -> DomainScope {
    DomainScope::from(Domain::Technology(Technology::Software(Software::Data(
        DataLeaf::All,
    ))))
}

fn domain_scopes_from_scopes(scopes: &[DomainScope]) -> DomainScopes {
    DomainScopes::new(scopes.to_vec())
}

fn keywords_from_slice(keywords: &[&str]) -> Keywords {
    Keywords::new(
        keywords
            .iter()
            .map(|keyword| Keyword::new(String::from(*keyword)))
            .collect(),
    )
}

fn referents_from_slice(referents: &[&str]) -> Referents {
    Referents::new(
        referents
            .iter()
            .map(|referent| Referent::new(*referent))
            .collect(),
    )
}

fn assert_short_record_identifier(identifier: &RecordIdentifier) {
    assert!(
        (4..=7).contains(&identifier.payload().len()),
        "record identifier should use a four-to-seven-character code: {:?}",
        identifier.payload()
    );
    assert!(
        identifier
            .payload()
            .chars()
            .all(|character| character.is_ascii_digit() || character.is_ascii_lowercase()),
        "record identifier should be lower-base36: {:?}",
        identifier.payload()
    );
}

fn query() -> Query {
    full_query(&["runtime-triad"], Some(Kind::Decision))
}

fn full_query(domains: &[&str], kind: Option<Kind>) -> Query {
    Query {
        domain_match: DomainMatch::full(domain_scopes_from_slice(domains)),
        keyword_match: KeywordMatch::Any,
        text_match: TextMatch::Any,
        referent_selection: spirit::schema::signal::ReferentSelection::Any,
        selected_kind: SelectedKind::new(kind),
        privacy_selection: PrivacySelection::default_observation_privacy(),
        certainty_selection: CertaintySelection::default_observation_certainty(),
        importance_selection: ImportanceSelection::default_observation_importance(),
    }
}

fn partial_query(domains: &[&str], kind: Option<Kind>) -> Query {
    Query {
        domain_match: DomainMatch::partial(domain_scopes_from_slice(domains)),
        keyword_match: KeywordMatch::Any,
        text_match: TextMatch::Any,
        referent_selection: spirit::schema::signal::ReferentSelection::Any,
        selected_kind: SelectedKind::new(kind),
        privacy_selection: PrivacySelection::default_observation_privacy(),
        certainty_selection: CertaintySelection::default_observation_certainty(),
        importance_selection: ImportanceSelection::default_observation_importance(),
    }
}

fn query_with_domain_scopes(domain_scopes: DomainScopes) -> Query {
    Query {
        domain_match: DomainMatch::full(domain_scopes),
        keyword_match: KeywordMatch::Any,
        text_match: TextMatch::Any,
        referent_selection: spirit::schema::signal::ReferentSelection::Any,
        selected_kind: SelectedKind::new(Some(Kind::Decision)),
        privacy_selection: PrivacySelection::default_observation_privacy(),
        certainty_selection: CertaintySelection::default_observation_certainty(),
        importance_selection: ImportanceSelection::default_observation_importance(),
    }
}

fn privacy_query(privacy_selection: PrivacySelection) -> Query {
    Query {
        privacy_selection,
        ..query()
    }
}

fn record_selection() -> RecordSelection {
    RecordSelection {
        domain_match: DomainMatch::Any,
        selected_kind: SelectedKind::new(Some(Kind::Decision)),
    }
}

fn route(offset: u64) -> OriginRoute {
    OriginRoute::new(1_000_000 + offset)
}

fn nexus_route(offset: u64) -> spirit::schema::nexus::OriginRoute {
    route(offset).into()
}

fn execute_nexus(nexus: &mut Nexus, input: nexus::Nexus<NexusWork>) -> nexus::Nexus<NexusAction> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("nexus test runtime")
        .block_on(nexus.execute_to_reply(input))
}

fn execute_nexus_on_multi_thread_runtime(
    nexus: &mut Nexus,
    input: nexus::Nexus<NexusWork>,
) -> nexus::Nexus<NexusAction> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("nexus multi-thread test runtime")
        .block_on(nexus.execute_to_reply(input))
}

fn nexus_reply_output(output: &nexus::Nexus<NexusAction>) -> &Output {
    match output.root() {
        NexusAction::ReplyToSignal(reply) => reply,
        other => panic!("expected ReplyToSignal, got {other:?}"),
    }
}

fn nexus_signal_input(input: &nexus::Nexus<NexusWork>) -> &Input {
    match input.root() {
        NexusWork::SignalArrived(signal) => signal,
        other => panic!("expected SignalArrived, got {other:?}"),
    }
}

fn sema_route(offset: u64) -> spirit::schema::sema::OriginRoute {
    route(offset).into()
}

fn sema_write_message(input: SemaWriteInput, offset: u64) -> sema::Sema<sema::WriteInput> {
    input.with_origin_route(sema_route(offset))
}

fn sema_read_message(input: SemaReadInput, offset: u64) -> sema::Sema<sema::ReadInput> {
    input.with_origin_route(sema_route(offset))
}

fn input_record(entry: Entry) -> Input {
    Input::record(record_request(entry))
}

fn input_propose(entry: Entry) -> Input {
    Input::propose(Proposal {
        entry,
        justification: justification("proposed forward arrow"),
    })
}

fn input_state(statement: &str) -> Input {
    Input::state(Statement::new(StatementText::new(statement)))
}

fn input_observe(query: Query) -> Input {
    Input::observe(query)
}

fn input_public_records(selection: RecordSelection) -> Input {
    Input::public_records(selection)
}

fn input_public_intent(scopes: DomainScopes) -> Input {
    Input::public_intent(scopes)
}

fn input_public_text_search(search_text: &str) -> Input {
    Input::public_text_search(SearchText::new(search_text))
}

fn input_private_records(selection: RecordSelection) -> Input {
    Input::private_records(selection)
}

fn input_lookup(record_identifier: RecordIdentifier) -> Input {
    Input::lookup(record_identifier)
}

fn input_clarify(record_identifier: RecordIdentifier, description: &str) -> Input {
    Input::clarify(Clarification {
        record_identifier,
        description: Description::new(description),
        justification: justification(description),
    })
}

fn input_supersede(record_identifier: RecordIdentifier, replacement: Entry) -> Input {
    Input::supersede(Supersession {
        retired_identifiers: RetiredIdentifiers::new(vec![RetiredIdentifier::new(
            record_identifier,
        )]),
        replacements: Replacements::new(vec![replacement]),
        justification: justification("replacement forward arrow"),
    })
}

fn input_retire(record_identifier: RecordIdentifier) -> Input {
    Input::retire(Retirement {
        record_identifier,
        justification: justification("retire this record"),
    })
}

fn input_count(query: Query) -> Input {
    Input::count(query)
}

fn input_change_certainty(record_identifier: RecordIdentifier, certainty: Magnitude) -> Input {
    Input::change_certainty(CertaintyChange {
        record_identifier,
        certainty: Certainty::new(certainty),
    })
}

fn input_change_record(record_identifier: RecordIdentifier, entry: Entry) -> Input {
    Input::change_record(RecordChange {
        record_identifier,
        entry,
        justification: justification("change record"),
    })
}

fn input_bump_importance(record_identifier: RecordIdentifier) -> Input {
    Input::bump_importance(ImportanceBump::new(record_identifier))
}

fn input_register_referent(referent: &str, aliases: &[&str]) -> Input {
    Input::register_referent(ReferentRegistration {
        referent: Referent::new(referent),
        aliases: referents_from_slice(aliases).into(),
        justification: justification(referent),
    })
}

fn input_lookup_stash(stash_handle: StashHandle) -> Input {
    Input::lookup_stash(stash_handle)
}

fn nexus_signal_arrived(input: Input) -> NexusWork {
    NexusWork::signal_arrived(input)
}

fn sema_record(entry: Entry) -> SemaWriteInput {
    SemaWriteInput::record(entry)
}

fn sema_change_certainty(
    record_identifier: RecordIdentifier,
    certainty: Magnitude,
) -> SemaWriteInput {
    SemaWriteInput::change_certainty(CertaintyChange {
        record_identifier,
        certainty: Certainty::new(certainty),
    })
}

fn sema_change_record(record_identifier: RecordIdentifier, entry: Entry) -> SemaWriteInput {
    SemaWriteInput::change_record(RecordChange {
        record_identifier,
        entry,
        justification: justification("change record"),
    })
}

fn sema_bump_importance(record_identifier: RecordIdentifier) -> SemaWriteInput {
    SemaWriteInput::bump_importance(ImportanceBump::new(record_identifier))
}

fn sema_observe(query: Query) -> SemaReadInput {
    SemaReadInput::observe(query)
}

fn sema_lookup(record_identifier: RecordIdentifier) -> SemaReadInput {
    SemaReadInput::lookup(record_identifier)
}

fn sema_count(query: Query) -> SemaReadInput {
    SemaReadInput::count(query)
}

#[test]
fn generated_plane_roots_implement_shared_triad_runtime_roles() {
    fn assert_nexus_work<Work: triad_runtime::NexusWork>() {}
    fn assert_sema_write_input<Input: triad_runtime::SemaWriteInput>() {}
    fn assert_sema_write_output<Output: triad_runtime::SemaWriteOutput>() {}
    fn assert_sema_read_input<Input: triad_runtime::SemaReadInput>() {}
    fn assert_sema_read_output<Output: triad_runtime::SemaReadOutput>() {}

    assert_nexus_work::<NexusWork>();
    assert_sema_write_input::<SemaWriteInput>();
    assert_sema_write_output::<SemaWriteOutput>();
    assert_sema_read_input::<SemaReadInput>();
    assert_sema_read_output::<SemaReadOutput>();
}

#[test]
fn nexus_runner_loop_routes_record_input_to_sema_write_command_then_back_to_reply() {
    // Designer 480 / operator 287 §"Recursive Computation": the runner
    // loop replaces the old projection chain. NexusEngine::execute drives
    // SignalArrived(Record) → CommandSemaWrite → SemaWriteCompleted →
    // ReplyToSignal(RecordAccepted). The trace property is observable on
    // the Nexus instance after execution.
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_record(entry("nexus runner routes")))
        .with_origin_route(nexus_route(1));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);

    assert_eq!(nexus_output.origin_route(), nexus_route(1));
    match nexus_reply_output(&nexus_output) {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected ReplyToSignal(RecordAccepted), got {other:?}"),
    }
    assert_eq!(nexus.store().len(), 1, "runner loop committed to SEMA");
}

#[test]
fn nexus_runner_moves_sema_work_through_multi_thread_runtime_boundary() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let record_input = nexus_signal_arrived(input_record(entry("multi-thread sema write")))
        .with_origin_route(nexus_route(11));

    let record_output = execute_nexus_on_multi_thread_runtime(&mut nexus, record_input);

    match nexus_reply_output(&record_output) {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected multi-thread RecordAccepted, got {other:?}"),
    }

    let observe_input =
        nexus_signal_arrived(input_observe(query())).with_origin_route(nexus_route(12));
    let observe_output = execute_nexus_on_multi_thread_runtime(&mut nexus, observe_input);

    match nexus_reply_output(&observe_output) {
        Output::RecordsStashed(stash) => {
            assert_eq!(*stash.stash_handle.payload(), 1);
            assert_eq!(*stash.record_count.payload(), 1);
        }
        other => panic!("expected observed records to become RecordsStashed, got {other:?}"),
    }
}

#[test]
fn nexus_classifies_state_into_provisional_record_through_sema_write() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_state("capture this statement"))
        .with_origin_route(nexus_route(2));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);

    match nexus_reply_output(&nexus_output) {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected State to classify into RecordAccepted, got {other:?}"),
    }

    let observed = SemaEngine::observe(
        nexus.store(),
        sema_read_message(
            sema_observe(full_query(&["unclassified"], Some(Kind::Clarification))),
            3,
        ),
    );
    match observed.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "capture this statement"
            );
            assert_eq!(
                records.payload().payload()[0].entry.certainty,
                Magnitude::Minimum
            );
            assert_eq!(
                records.payload().payload()[0].entry.privacy,
                Magnitude::Zero
            );
        }
        other => panic!("expected classified State record to be observable, got {other:?}"),
    }
}

#[test]
fn nexus_change_certainty_is_visible_as_schema_declared_write_command() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let identifier = record_identifier("003g");
    let nexus_input =
        nexus_signal_arrived(input_change_certainty(identifier.clone(), Magnitude::Zero))
            .with_origin_route(nexus_route(5));

    let first_action = NexusEngine::decide(&mut nexus, nexus_input);

    assert_eq!(first_action.origin_route(), nexus_route(5));
    match first_action.root() {
        NexusAction::CommandSemaWrite(CommandSemaWrite::ChangeCertainty(change)) => {
            assert_eq!(change.record_identifier, identifier);
            assert_eq!(change.certainty, Magnitude::Zero);
        }
        other => panic!(
            "expected ChangeCertainty to become CommandSemaWrite(ChangeCertainty), got {other:?}"
        ),
    }
}

#[test]
fn nexus_change_record_is_visible_as_schema_declared_write_command() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let identifier = record_identifier("003g");
    let replacement = entry_with_domains(&["runtime-triad", "replacement"], "replacement record");
    let nexus_input =
        nexus_signal_arrived(input_change_record(identifier.clone(), replacement.clone()))
            .with_origin_route(nexus_route(6));

    let first_action = NexusEngine::decide(&mut nexus, nexus_input);

    assert_eq!(first_action.origin_route(), nexus_route(6));
    match first_action.root() {
        NexusAction::CommandEffect(effect) => {
            let NexusEffectCommand::ChangeRecordWithImpliedReferents(change) = effect else {
                panic!("expected ChangeRecordWithImpliedReferents effect, got {effect:?}");
            };
            assert_eq!(change.payload().record_identifier, identifier);
            assert_eq!(change.payload().entry, replacement);
        }
        other => {
            panic!(
                "expected ChangeRecord to become CommandEffect(ChangeRecordWithImpliedReferents), got {other:?}"
            )
        }
    }
}

#[test]
fn nexus_bump_importance_is_visible_as_schema_declared_write_command() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let identifier = record_identifier("003g");
    let nexus_input = nexus_signal_arrived(input_bump_importance(identifier.clone()))
        .with_origin_route(nexus_route(7));

    let first_action = NexusEngine::decide(&mut nexus, nexus_input);

    assert_eq!(first_action.origin_route(), nexus_route(7));
    match first_action.root() {
        NexusAction::CommandSemaWrite(CommandSemaWrite::BumpImportance(change)) => {
            assert_eq!(change.payload().payload(), &identifier);
        }
        other => {
            panic!(
                "expected BumpImportance to become CommandSemaWrite(BumpImportance), got {other:?}"
            )
        }
    }
}

#[test]
fn nexus_state_classification_is_visible_as_schema_declared_effect_command() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_state("visible classification"))
        .with_origin_route(nexus_route(4));

    let first_action = NexusEngine::decide(&mut nexus, nexus_input);

    assert_eq!(first_action.origin_route(), nexus_route(4));
    match first_action.root() {
        NexusAction::CommandEffect(effect) => match effect {
            NexusEffectCommand::ClassifyState(statement) => {
                assert_eq!(
                    statement.payload().payload().payload(),
                    "visible classification"
                );
            }
            other => panic!("expected State to become ClassifyState, got {other:?}"),
        },
        other => panic!("expected State to become CommandEffect(ClassifyState), got {other:?}"),
    }
    assert_eq!(
        nexus.store().len(),
        0,
        "the first Nexus decision exposes classification before durable SEMA write"
    );
}

#[test]
fn sema_engine_changes_certainty_without_changing_record_identifier() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    let recorded = SemaEngine::apply(
        &mut store,
        sema_write_message(sema_record(entry("certainty target")), 1),
    );
    let record_identifier = match recorded.into_root() {
        SemaWriteOutput::Recorded(receipt) => receipt.record_identifier.clone(),
        other => panic!("expected initial Recorded receipt, got {other:?}"),
    };

    let changed = SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_change_certainty(record_identifier.clone(), Magnitude::Zero),
            2,
        ),
    );
    match changed.root() {
        SemaWriteOutput::CertaintyChanged(receipt) => {
            assert_eq!(receipt.record_identifier, record_identifier);
            assert_eq!(receipt.certainty, Magnitude::Zero);
        }
        other => panic!("expected CertaintyChanged receipt, got {other:?}"),
    }

    let found = SemaEngine::observe(
        &store,
        sema_read_message(sema_lookup(record_identifier.clone()), 3),
    );
    match found.root() {
        SemaReadOutput::Found(record) => {
            assert_eq!(record.record_identifier, record_identifier);
            assert_eq!(record.entry.description.payload(), "certainty target");
            assert_eq!(record.entry.certainty, Magnitude::Zero);
        }
        other => panic!("expected changed record lookup, got {other:?}"),
    }
}

#[test]
fn nexus_write_operations_are_visible_as_schema_declared_effect_commands() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let identifier = record_identifier("003g");

    let propose = NexusEngine::decide(
        &mut nexus,
        nexus_signal_arrived(input_propose(entry("proposed arrow")))
            .with_origin_route(nexus_route(8)),
    );
    match propose.root() {
        NexusAction::CommandEffect(effect) => {
            assert!(matches!(
                effect,
                NexusEffectCommand::ProposeWithImpliedReferents(_)
            ));
        }
        other => panic!(
            "expected Propose to become CommandEffect(ProposeWithImpliedReferents), got {other:?}"
        ),
    }

    let clarify = NexusEngine::decide(
        &mut nexus,
        nexus_signal_arrived(input_clarify(identifier.clone(), "clearer arrow"))
            .with_origin_route(nexus_route(9)),
    );
    match clarify.root() {
        NexusAction::CommandEffect(effect) => {
            assert!(matches!(effect, NexusEffectCommand::Clarify(_)));
        }
        other => panic!("expected Clarify to become CommandEffect(Clarify), got {other:?}"),
    }

    let supersede = NexusEngine::decide(
        &mut nexus,
        nexus_signal_arrived(input_supersede(
            identifier.clone(),
            entry("replacement arrow"),
        ))
        .with_origin_route(nexus_route(10)),
    );
    match supersede.root() {
        NexusAction::CommandEffect(effect) => {
            assert!(matches!(
                effect,
                NexusEffectCommand::SupersedeWithImpliedReferents(_)
            ));
        }
        other => panic!(
            "expected Supersede to become CommandEffect(SupersedeWithImpliedReferents), got {other:?}"
        ),
    }

    let retire = NexusEngine::decide(
        &mut nexus,
        nexus_signal_arrived(input_retire(identifier)).with_origin_route(nexus_route(11)),
    );
    match retire.root() {
        NexusAction::CommandEffect(effect) => {
            assert!(matches!(effect, NexusEffectCommand::Retire(_)));
        }
        other => panic!("expected Retire to become CommandEffect(Retire), got {other:?}"),
    }
}

#[test]
fn sema_engine_bumps_record_importance_without_changing_record_identifier() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    let recorded = SemaEngine::apply(
        &mut store,
        sema_write_message(sema_record(entry("importance target")), 1),
    );
    let record_identifier = match recorded.into_root() {
        SemaWriteOutput::Recorded(receipt) => receipt.record_identifier.clone(),
        other => panic!("expected initial Recorded receipt, got {other:?}"),
    };

    let bumped = SemaEngine::apply(
        &mut store,
        sema_write_message(sema_bump_importance(record_identifier.clone()), 2),
    );
    match bumped.root() {
        SemaWriteOutput::ImportanceBumped(receipt) => {
            let receipt = receipt.payload();
            assert_eq!(receipt.record_identifier, record_identifier);
            assert_eq!(receipt.importance.payload(), &Magnitude::VeryLow);
        }
        other => panic!("expected ImportanceBumped receipt, got {other:?}"),
    }

    let found = SemaEngine::observe(
        &store,
        sema_read_message(sema_lookup(record_identifier.clone()), 3),
    );
    match found.root() {
        SemaReadOutput::Found(record) => {
            assert_eq!(record.record_identifier, record_identifier);
            assert_eq!(record.entry.description.payload(), "importance target");
            assert_eq!(record.entry.importance.payload(), &Magnitude::VeryLow);
        }
        other => panic!("expected bumped record lookup, got {other:?}"),
    }
}

#[test]
fn sema_engine_changes_record_without_changing_record_identifier() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    let recorded = SemaEngine::apply(
        &mut store,
        sema_write_message(sema_record(entry("original target")), 1),
    );
    let record_identifier = match recorded.into_root() {
        SemaWriteOutput::Recorded(receipt) => receipt.record_identifier.clone(),
        other => panic!("expected initial Recorded receipt, got {other:?}"),
    };

    let replacement = entry_with_domains(&["runtime-triad", "replacement"], "replacement target");
    let changed = SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_change_record(record_identifier.clone(), replacement.clone()),
            2,
        ),
    );
    match changed.root() {
        SemaWriteOutput::RecordChanged(receipt) => {
            assert_eq!(receipt.payload().payload(), &record_identifier);
        }
        other => panic!("expected RecordChanged receipt, got {other:?}"),
    }

    let found = SemaEngine::observe(
        &store,
        sema_read_message(sema_lookup(record_identifier.clone()), 3),
    );
    match found.root() {
        SemaReadOutput::Found(record) => {
            assert_eq!(record.record_identifier, record_identifier);
            assert_eq!(record.entry, replacement);
        }
        other => panic!("expected changed record lookup, got {other:?}"),
    }
}

#[test]
fn signal_write_operations_propose_clarify_supersede_and_retire() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let proposed = engine.handle(input_propose(entry("initial forward arrow")));
    let original_identifier = match proposed.root() {
        Output::Proposed(receipt) => receipt.payload().clone(),
        other => panic!("expected Proposed receipt, got {other:?}"),
    };

    let duplicate = engine.handle(input_propose(entry("initial forward arrow")));
    match duplicate.root() {
        Output::GuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().guardian_rejection_reason,
                GuardianRejectionReason::Duplicate
            );
            assert_eq!(rejection.payload().record_set.len(), 1);
            assert_eq!(
                rejection.payload().record_set[0].record_identifier,
                original_identifier
            );
            assert_eq!(
                rejection.payload().record_set[0].entry.importance.payload(),
                &Magnitude::VeryLow
            );
        }
        other => panic!("expected duplicate GuardianRejected receipt, got {other:?}"),
    }

    let clarified = engine.handle(input_clarify(
        original_identifier.clone(),
        "clearer forward arrow",
    ));
    match clarified.root() {
        Output::Clarified(receipt) => {
            assert_eq!(receipt.payload().payload(), &original_identifier);
        }
        other => panic!("expected Clarified receipt, got {other:?}"),
    }
    match engine
        .handle(input_lookup(original_identifier.clone()))
        .into_root()
    {
        Output::RecordFound(found) => {
            assert_eq!(found.entry.description.payload(), "clearer forward arrow");
            assert_eq!(found.record_identifier, original_identifier);
        }
        other => panic!("expected clarified record lookup, got {other:?}"),
    }

    let superseded = engine.handle(input_supersede(
        original_identifier.clone(),
        entry("replacement forward arrow"),
    ));
    let replacement_identifier = match superseded.root() {
        Output::Superseded(receipt) => {
            assert_eq!(receipt.payload().retired_identifiers.payload().len(), 1);
            assert_eq!(
                receipt.payload().retired_identifiers.payload()[0].payload(),
                &original_identifier
            );
            assert_eq!(receipt.payload().record_identifiers.payload().len(), 1);
            receipt.payload().record_identifiers.payload()[0].clone()
        }
        other => panic!("expected Superseded receipt, got {other:?}"),
    };
    assert!(matches!(
        engine.handle(input_lookup(original_identifier)).into_root(),
        Output::Error(_)
    ));
    match engine
        .handle(input_lookup(replacement_identifier.clone()))
        .into_root()
    {
        Output::RecordFound(found) => {
            assert_eq!(
                found.entry.description.payload(),
                "replacement forward arrow"
            );
            assert_eq!(found.record_identifier, replacement_identifier);
        }
        other => panic!("expected replacement lookup, got {other:?}"),
    }

    let retired = engine.handle(input_retire(replacement_identifier.clone()));
    match retired.root() {
        Output::Retired(receipt) => {
            assert_eq!(receipt.payload().payload(), &replacement_identifier);
        }
        other => panic!("expected Retired receipt, got {other:?}"),
    }
    assert!(matches!(
        engine
            .handle(input_lookup(replacement_identifier))
            .into_root(),
        Output::Error(_)
    ));
    assert_eq!(
        sema.open_archive_store().len(),
        3,
        "clarify, supersede, and retire each archive the prior live arrow"
    );
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_accept_verdict_admits_proposal() {
    let sema = SemaFile::new();
    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::Accept);
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_propose(entry_without_referents(
        "model accepted forward arrow",
    )));

    assert!(matches!(output.root(), Output::Proposed(_)));
    assert_eq!(engine.record_count(), 1);
    assert_eq!(engine.guardian_decision_count(), 1);
    fake_agent.join();
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_reject_verdict_blocks_proposal() {
    let sema = SemaFile::new();
    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::reject(Reject {
        guardian_rejection_reason: GuardianRejectionReason::NonIntent,
        explanation: spirit::schema::signal::Explanation::new("not settled intent"),
    }));
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_propose(entry_without_referents(
        "model rejected forward arrow",
    )));

    match output.root() {
        Output::GuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().guardian_rejection_reason,
                GuardianRejectionReason::NonIntent
            );
            assert_eq!(
                rejection.payload().explanation.payload(),
                "not settled intent"
            );
        }
        other => panic!("expected GuardianRejected, got {other:?}"),
    }
    assert_eq!(engine.record_count(), 0);
    assert_eq!(engine.guardian_decision_count(), 1);
    fake_agent.join();
}

#[cfg(all(feature = "agent-guardian", feature = "mirror-shipper"))]
#[test]
fn guardian_rejection_does_not_contact_criome_operation_authorizer() {
    let sema = SemaFile::new();
    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::reject(Reject {
        guardian_rejection_reason: GuardianRejectionReason::NonIntent,
        explanation: spirit::schema::signal::Explanation::new("not settled intent"),
    }));
    let fake_criome = FakeCriomeAuthorizationSocket::spawn_pending();
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());
    let configured = engine.configure(ConfigureRequest::new(
        ArchiveDatabaseTarget::Default,
        None,
        Some(CriomeGateTarget::socket(CriomeSocketPathText::new(
            fake_criome.socket_path().display().to_string(),
        ))),
        None,
    ));
    assert!(matches!(configured, MetaOutput::Configured(_)));

    let output = engine.handle(input_record(entry_without_referents(
        "guardian denial happens before criome interception",
    )));

    assert!(matches!(output.root(), Output::GuardianRejected(_)));
    assert_eq!(engine.record_count(), 0);
    fake_agent.join();
    assert!(
        fake_criome.join().is_empty(),
        "guardian denial must stop before criome receives an authorization request"
    );
}

#[cfg(all(feature = "agent-guardian", feature = "mirror-shipper"))]
#[test]
fn guardian_acceptance_sends_spirit_context_to_criome_before_write() {
    let sema = SemaFile::new();
    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::Accept);
    let fake_criome = FakeCriomeAuthorizationSocket::spawn_pending();
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());
    let configured = engine.configure(ConfigureRequest::new(
        ArchiveDatabaseTarget::Default,
        None,
        Some(CriomeGateTarget::socket(CriomeSocketPathText::new(
            fake_criome.socket_path().display().to_string(),
        ))),
        None,
    ));
    assert!(matches!(configured, MetaOutput::Configured(_)));

    let output = engine.handle(input_record(entry_without_referents(
        "guardian allow sends raw Spirit payload to criome",
    )));

    assert!(
        matches!(output.root(), Output::Error(_)),
        "fake criome parks the request, so gating blocks the write"
    );
    assert_eq!(engine.record_count(), 0);
    fake_agent.join();
    let requests = fake_criome.join();
    assert_eq!(requests.len(), 1);
    let CriomeRequest::AuthorizeSignalCall(authorization) = &requests[0] else {
        panic!("expected AuthorizeSignalCall, got {:?}", requests[0]);
    };
    assert_eq!(
        authorization.object.component,
        signal_criome::ComponentKind::Spirit,
        "the typed ask names spirit as the owning component"
    );
    assert_eq!(
        authorization.object.kind,
        signal_criome::AuthorizedObjectKind::Operation,
        "an operation-level ask carries the Operation kind, never strings"
    );
    let context = authorization
        .spirit_context()
        .expect("Spirit authorization context");
    assert_eq!(context.operation_name.payload(), "Record");
    assert_eq!(context.target_key.payload(), "spirit-process-main");
    assert!(
        context
            .raw_payload
            .payload()
            .contains("guardian allow sends raw Spirit payload to criome"),
        "raw payload should preserve the submitted Spirit operation"
    );
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_reject_verdict_blocks_record() {
    let sema = SemaFile::new();
    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::reject(Reject {
        guardian_rejection_reason: GuardianRejectionReason::NonIntent,
        explanation: spirit::schema::signal::Explanation::new("not durable intent"),
    }));
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_record(entry_without_referents(
        "raw record should still be guarded",
    )));

    assert!(matches!(output.root(), Output::GuardianRejected(_)));
    assert_eq!(engine.record_count(), 0);
    assert_eq!(engine.guardian_decision_count(), 1);
    fake_agent.join();
}

#[cfg(feature = "agent-guardian")]
#[test]
fn required_guardian_rejects_writes_when_unconfigured() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();
    engine.require_guardian();

    let output = engine.handle(input_record(entry_without_referents(
        "unguarded write should fail closed",
    )));

    match output.root() {
        Output::GuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().guardian_rejection_reason,
                GuardianRejectionReason::HarnessUnavailable
            );
            assert_eq!(
                rejection.payload().explanation.payload(),
                "guardian is required but no guardian agent is configured"
            );
        }
        other => panic!("expected GuardianRejected for missing guardian, got {other:?}"),
    }
    assert_eq!(engine.record_count(), 0);
    assert_eq!(engine.guardian_decision_count(), 1);
}

#[cfg(feature = "agent-guardian")]
#[test]
fn required_guardian_rejects_referent_registration_when_unconfigured() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();
    engine.require_guardian();

    let output = engine.handle(input_register_referent("unguarded-referent", &[]));

    match output.root() {
        Output::ReferentGuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().referent_guardian_rejection_reason,
                ReferentGuardianRejectionReason::HarnessUnavailable
            );
            assert_eq!(
                rejection.payload().explanation.payload(),
                "guardian is required but no guardian agent is configured"
            );
        }
        other => panic!("expected ReferentGuardianRejected for missing guardian, got {other:?}"),
    }
    assert_eq!(engine.guardian_decision_count(), 1);
}

#[cfg(feature = "agent-guardian")]
#[test]
fn required_guardian_skips_noop_existing_referent_registration() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let registered = engine.handle(input_register_referent("schema", &["schema-alias"]));
    assert!(matches!(registered.root(), Output::ReferentRegistered(_)));
    assert_eq!(engine.guardian_decision_count(), 0);

    engine.require_guardian();
    let repeated = engine.handle(input_register_referent("schema", &[]));

    match repeated.root() {
        Output::ReferentRegistered(receipt) => {
            assert_eq!(receipt.payload().payload().payload(), "schema");
        }
        other => {
            panic!("expected existing referent registration to bypass guardian, got {other:?}")
        }
    }
    assert_eq!(engine.guardian_decision_count(), 0);
}

#[cfg(feature = "agent-guardian")]
#[test]
fn required_guardian_still_rejects_existing_referent_with_new_alias_when_unconfigured() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let registered = engine.handle(input_register_referent("schema", &[]));
    assert!(matches!(registered.root(), Output::ReferentRegistered(_)));

    engine.require_guardian();
    let output = engine.handle(input_register_referent("schema", &["schema-alias"]));

    match output.root() {
        Output::ReferentGuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().referent_guardian_rejection_reason,
                ReferentGuardianRejectionReason::HarnessUnavailable
            );
        }
        other => panic!("expected new alias to require referent guardian, got {other:?}"),
    }
    assert_eq!(engine.guardian_decision_count(), 1);
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_duplicate_rejection_bumps_importance() {
    let sema = SemaFile::new();
    let mut setup_engine = sema.engine();
    let original_identifier = match setup_engine
        .handle(input_propose(entry("duplicate model verdict")))
        .into_root()
    {
        Output::Proposed(identifier) => identifier.into_payload(),
        other => panic!("expected setup Proposed, got {other:?}"),
    };
    drop(setup_engine);
    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::reject(Reject {
        guardian_rejection_reason: GuardianRejectionReason::Duplicate,
        explanation: spirit::schema::signal::Explanation::new("same arrow already exists"),
    }));
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_propose(entry("duplicate model verdict")));

    match output.root() {
        Output::GuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().guardian_rejection_reason,
                GuardianRejectionReason::Duplicate
            );
            assert_eq!(rejection.payload().record_set.len(), 1);
            assert_eq!(
                rejection.payload().record_set[0].record_identifier,
                original_identifier
            );
            assert_eq!(
                rejection.payload().record_set[0].entry.importance.payload(),
                &Magnitude::VeryLow
            );
        }
        other => panic!("expected duplicate GuardianRejected, got {other:?}"),
    }
    assert_eq!(engine.record_count(), 1);
    assert_eq!(engine.guardian_decision_count(), 1);
    fake_agent.join();
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_typed_packet_includes_equivalent_domain_records() {
    let sema = SemaFile::new();
    let mut setup_engine = sema.engine();
    assert!(matches!(
        setup_engine
            .handle(input_record(entry_with_domain(
                Domain::Information(Information::Database),
                "database-live",
            )))
            .root(),
        Output::RecordAccepted(_)
    ));
    drop(setup_engine);

    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::Accept);
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());
    let output = engine.handle(input_propose(entry_with_domain(
        Domain::Technology(Technology::Software(Software::Data(DataLeaf::All))),
        "software-data-candidate",
    )));

    assert!(matches!(output.root(), Output::Proposed(_)));
    let requests = fake_agent.join();
    let packet = only_admission_packet(&requests);
    assert!(
        packet.records.payload().iter().any(|record| {
            record.entry.description.payload() == "database-live"
                && record
                    .entry
                    .domains
                    .payload()
                    .contains(&Domain::Information(Information::Database))
        }),
        "typed judge packet should include the equivalent information-database record: {:?}",
        packet.records
    );
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_typed_packet_excludes_unmatched_live_records() {
    let sema = SemaFile::new();
    let mut setup_engine = sema.engine();
    assert!(matches!(
        setup_engine
            .handle(input_record(Entry {
                domains: Domains::new(vec![Domain::Health(
                    spirit::schema::signal::Health::Medicine,
                )]),
                referents: referents_from_slice(&["medicine"]),
                ..entry("same-kind-only-live")
            }))
            .root(),
        Output::RecordAccepted(_)
    ));
    assert!(matches!(
        setup_engine
            .handle(input_record(entry_with_domain(
                Domain::Technology(Technology::Software(Software::Security(
                    spirit::schema::signal::SecurityLeaf::Authorization
                ))),
                "authorization-live",
            )))
            .root(),
        Output::RecordAccepted(_)
    ));
    drop(setup_engine);

    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::Accept);
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());
    let output = engine.handle(input_propose(entry_with_domain(
        Domain::Technology(Technology::Software(Software::Security(
            spirit::schema::signal::SecurityLeaf::Authorization,
        ))),
        "authorization-candidate",
    )));

    assert!(matches!(output.root(), Output::Proposed(_)));
    let requests = fake_agent.join();
    let packet = only_admission_packet(&requests);
    assert!(
        packet
            .records
            .payload()
            .iter()
            .any(|record| record.entry.description.payload() == "authorization-live"),
        "typed judge packet should include domain-relevant records: {:?}",
        packet.records
    );
    assert!(
        !packet
            .records
            .payload()
            .iter()
            .any(|record| record.entry.description.payload() == "same-kind-only-live"),
        "typed judge packet should exclude records outside the domain/referent neighborhood: {:?}",
        packet.records
    );
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_typed_packet_stays_bounded_as_corpus_grows() {
    let sema = SemaFile::new();
    let mut setup_engine = sema.engine();
    assert!(matches!(
        setup_engine
            .handle(input_register_referent("schema", &["schema-alias"]))
            .root(),
        Output::ReferentRegistered(_)
    ));
    for offset in 0..70 {
        let description = format!("guardian-unrelated-live-{offset:02}");
        assert!(matches!(
            setup_engine
                .handle(input_record(entry_with_domain(
                    Domain::Health(spirit::schema::signal::Health::Medicine),
                    &description,
                )))
                .root(),
            Output::RecordAccepted(_)
        ));
    }
    assert!(matches!(
        setup_engine
            .handle(input_record(entry_with_domain(
                Domain::Technology(Technology::Software(Software::Quality(
                    spirit::schema::signal::QualityLeaf::Testing
                ))),
                "guardian-domain-neighbor-live",
            )))
            .root(),
        Output::RecordAccepted(_)
    ));
    assert!(matches!(
        setup_engine
            .handle(input_record(Entry {
                domains: Domains::new(vec![Domain::Health(
                    spirit::schema::signal::Health::Medicine,
                )]),
                referents: referents_from_slice(&["schema"]),
                ..entry("guardian-referent-neighbor-live")
            }))
            .root(),
        Output::RecordAccepted(_)
    ));
    drop(setup_engine);

    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::Accept);
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());
    let output = engine.handle(input_propose(Entry {
        domains: Domains::new(vec![Domain::Technology(Technology::Software(
            Software::Quality(spirit::schema::signal::QualityLeaf::Testing),
        ))]),
        referents: referents_from_slice(&["schema"]),
        ..entry("guardian bounded candidate")
    }));

    assert!(matches!(output.root(), Output::Proposed(_)));
    let requests = fake_agent.join();
    let packet = only_admission_packet(&requests);
    assert_eq!(
        packet
            .records
            .payload()
            .iter()
            .filter(|record| record
                .entry
                .description
                .payload()
                .starts_with("guardian-unrelated-live-"))
            .count(),
        0,
        "typed judge packet should not scale with unrelated corpus size: {:?}",
        packet.records
    );
    assert!(
        packet
            .records
            .payload()
            .iter()
            .any(|record| record.entry.description.payload() == "guardian-domain-neighbor-live"),
        "typed judge packet should include exact-domain neighbors: {:?}",
        packet.records
    );
    assert!(
        packet
            .records
            .payload()
            .iter()
            .any(|record| record.entry.description.payload() == "guardian-referent-neighbor-live"),
        "typed judge packet should include shared-referent neighbors: {:?}",
        packet.records
    );
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_reject_verdict_blocks_clarify_supersede_and_retire() {
    let sema = SemaFile::new();
    let mut setup_engine = sema.engine();
    let original_identifier = match setup_engine
        .handle(input_record(entry("original guarded mutation target")))
        .into_root()
    {
        Output::RecordAccepted(identifier) => identifier.into_payload(),
        other => panic!("expected setup RecordAccepted, got {other:?}"),
    };
    drop(setup_engine);
    let fake_agent = FakeSpiritJudge::spawn_many(vec![
        GuardianVerdict::reject(Reject {
            guardian_rejection_reason: GuardianRejectionReason::ClarifyTramples,
            explanation: spirit::schema::signal::Explanation::new("changes the arrow"),
        }),
        GuardianVerdict::reject(Reject {
            guardian_rejection_reason: GuardianRejectionReason::Contradiction,
            explanation: spirit::schema::signal::Explanation::new("replacement conflicts"),
        }),
        GuardianVerdict::reject(Reject {
            guardian_rejection_reason: GuardianRejectionReason::NonIntent,
            explanation: spirit::schema::signal::Explanation::new("retirement not justified"),
        }),
    ]);
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let clarified = engine.handle(input_clarify(
        original_identifier.clone(),
        "trampling replacement text",
    ));
    assert!(matches!(clarified.root(), Output::GuardianRejected(_)));
    let superseded = engine.handle(input_supersede(
        original_identifier.clone(),
        entry("blocked replacement"),
    ));
    assert!(matches!(superseded.root(), Output::GuardianRejected(_)));
    let retired = engine.handle(input_retire(original_identifier.clone()));
    assert!(matches!(retired.root(), Output::GuardianRejected(_)));

    match engine.handle(input_lookup(original_identifier)).into_root() {
        Output::RecordFound(found) => {
            assert_eq!(
                found.entry.description.payload(),
                "original guarded mutation target"
            );
        }
        other => panic!("expected original record to remain, got {other:?}"),
    }
    assert_eq!(engine.record_count(), 1);
    assert_eq!(engine.guardian_decision_count(), 3);
    fake_agent.join();
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_accept_verdict_admits_referent_registration() {
    let sema = SemaFile::new();
    let fake_agent = FakeSpiritJudge::spawn_referent(ReferentGuardianVerdict::Accept);
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_register_referent("schema", &["schema-alias"]));

    match output.root() {
        Output::ReferentRegistered(receipt) => {
            assert_eq!(receipt.payload().payload().payload(), "schema");
        }
        other => panic!("expected ReferentRegistered, got {other:?}"),
    }
    assert_eq!(engine.guardian_decision_count(), 1);
    fake_agent.join();
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_reject_verdict_blocks_referent_registration() {
    let sema = SemaFile::new();
    let fake_agent =
        FakeSpiritJudge::spawn_referent(ReferentGuardianVerdict::reject_referent(RejectReferent {
            referent_guardian_rejection_reason: ReferentGuardianRejectionReason::TooVague,
            explanation: spirit::schema::signal::Explanation::new("not a concrete referent"),
        }));
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_register_referent("thing", &[]));

    match output.root() {
        Output::ReferentGuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().referent_guardian_rejection_reason,
                ReferentGuardianRejectionReason::TooVague
            );
            assert_eq!(
                rejection.payload().explanation.payload(),
                "not a concrete referent"
            );
        }
        other => panic!("expected ReferentGuardianRejected, got {other:?}"),
    }
    assert_eq!(engine.guardian_decision_count(), 1);
    fake_agent.join();
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_reject_verdict_blocks_embedded_referent_registration() {
    let sema = SemaFile::new();
    let fake_agent =
        FakeSpiritJudge::spawn_referent(ReferentGuardianVerdict::reject_referent(RejectReferent {
            referent_guardian_rejection_reason: ReferentGuardianRejectionReason::TooVague,
            explanation: spirit::schema::signal::Explanation::new("not a concrete referent"),
        }));
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_record(entry_with_referents(
        "embedded referent rejection blocks record",
        &["thing"],
    )));

    match output.root() {
        Output::ReferentGuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().referent_guardian_rejection_reason,
                ReferentGuardianRejectionReason::TooVague
            );
        }
        other => panic!("expected ReferentGuardianRejected, got {other:?}"),
    }
    assert_eq!(engine.record_count(), 0);
    assert_eq!(engine.guardian_decision_count(), 1);
    fake_agent.join();
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_accepts_embedded_referent_before_guarding_record() {
    let sema = SemaFile::new();
    let fake_agent = FakeSpiritJudge::spawn_replies(vec![
        FakeSpiritJudge::referent_reply(ReferentGuardianVerdict::Accept),
        FakeSpiritJudge::admission_reply(GuardianVerdict::Accept),
    ]);
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_record(entry_with_referents(
        "embedded referent accepted before record",
        &["schema"],
    )));

    match output.root() {
        Output::RecordAccepted(receipt) => assert_short_record_identifier(receipt.payload()),
        other => panic!("expected RecordAccepted, got {other:?}"),
    }
    assert_eq!(engine.record_count(), 1);
    assert_eq!(engine.guardian_decision_count(), 2);
    fake_agent.join();
}

#[cfg(feature = "agent-guardian")]
#[test]
fn agent_guardian_preserves_large_rejection_explanation() {
    let sema = SemaFile::new();
    let explanation = vec!["This rejection explanation is intentionally long."; 300].join(" ");
    let fake_agent = FakeSpiritJudge::spawn(GuardianVerdict::reject(Reject {
        guardian_rejection_reason: GuardianRejectionReason::Contradiction,
        explanation: spirit::schema::signal::Explanation::new(explanation.clone()),
    }));
    let mut engine = sema.engine_with_guardian(fake_agent.guardian());

    let output = engine.handle(input_propose(entry_without_referents(
        "model provides a long explanation",
    )));

    match output.root() {
        Output::GuardianRejected(rejection) => {
            assert_eq!(
                rejection.payload().guardian_rejection_reason,
                GuardianRejectionReason::Contradiction
            );
            assert_eq!(rejection.payload().explanation.payload(), &explanation);
        }
        other => panic!("expected GuardianRejected, got {other:?}"),
    }
    assert_eq!(engine.record_count(), 0);
    fake_agent.join();
}

#[test]
fn signal_admission_pushes_accepted_message_through_sent_hook_before_nexus_holds_mail() {
    let signal_admission = SignalAdmission::default();
    let signal_entry = entry("signal pushes to nexus");
    let expected_short_header = input_record(signal_entry.clone()).short_header();
    let accepted = signal_admission
        .admit(input_record(signal_entry.clone()))
        .expect("signal input admits");
    let mut hook = SentHookProbe { events: Vec::new() };
    let expected_sent = MailLedgerEvent::Sent(SentMail {
        mail_identifier: MailIdentifier::new(1),
        origin_route: route(1),
        short_header: ShortHeader::new(expected_short_header),
    });

    assert_eq!(
        accepted.message_sent().identifier,
        MessageIdentifier::new(1)
    );
    assert_eq!(accepted.message_sent().origin_route(), route(1));
    assert_ne!(
        accepted.message_sent().origin_route(),
        OriginRoute::new(accepted.message_sent().identifier.payload())
    );
    assert_eq!(hook.events, []);

    // The sent hook fires at the Signal -> Nexus handoff, witnessed by a
    // schema-emitted mail ledger event.
    accepted
        .message_sent()
        .clone()
        .push_to(&mut hook)
        .expect("sent hook fires");

    assert_eq!(
        hook.events,
        vec![expected_sent],
        "hook witness must be a schema-emitted mail ledger event"
    );
}

#[test]
fn mail_ledger_reclaims_terminal_mail_and_ignores_duplicate_terminal_events() {
    let identifier = MessageIdentifier::new(7);
    let origin_route = route(7);
    let ledger = MailLedger::default();
    MessageSent::new(identifier, origin_route, 42)
        .push_to(&mut ledger.hook())
        .expect("store sent mail");
    assert_eq!(ledger.in_flight_count(), 1);

    let terminal = MessageProcessed::new(
        identifier,
        origin_route,
        Output::rejected(SignalRejection::new(ValidationError::EmptyDomain)),
    );
    terminal
        .clone()
        .push_to(&mut ledger.hook())
        .expect("process mail");
    terminal
        .push_to(&mut ledger.hook())
        .expect("repeat terminal mail");

    assert_eq!(ledger.in_flight_count(), 0);
    assert!(ledger.events().is_empty());
    assert_eq!(ledger.sent_message_count(), 1);
    assert_eq!(ledger.processed_message_count(), 2);
}

#[test]
fn nexus_step_decide_routes_signal_arrival_to_sema_command_without_committing() {
    // Designer 480: the runner loop's step plane is now visible through
    // Nexus's hand-written decision center. A SignalArrived(Record) becomes
    // a CommandSemaWrite action; SEMA only commits when the runner spends
    // the action against SemaEngine::apply.
    let sema = SemaFile::new();
    let store = sema.open_store();
    assert!(store.is_empty(), "store starts empty");

    // Trace the runner's first action without driving it.
    let signal_admission = SignalAdmission::default();
    let signal_input = input_record(entry("held in flight"));
    let nexus_input = signal_admission.triage(signal_input, route(1));
    assert_eq!(nexus_input.origin_route(), nexus_route(1));
    match nexus_signal_input(&nexus_input) {
        Input::Record(recorded) => {
            assert_eq!(
                recorded.payload().entry.description.payload(),
                "held in flight"
            )
        }
        other => panic!("expected SignalArrived(Record), got {other:?}"),
    }

    assert!(
        store.is_empty(),
        "triage runs without committing to the durable SEMA store"
    );
}

#[test]
fn sema_engine_writes_durable_records_and_returns_schema_objects() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    let operation = sema_write_message(sema_record(entry("SEMA writes durable facts")), 1);

    let response = SemaEngine::apply(&mut store, operation);

    assert_eq!(response.origin_route(), sema_route(1));
    match response.root() {
        SemaWriteOutput::Recorded(receipt) => {
            assert_short_record_identifier(&receipt.record_identifier);
            assert_eq!(*receipt.database_marker.commit_sequence.payload(), 2);
            assert_ne!(
                *receipt.database_marker.state_digest.payload(),
                0,
                "a committed record yields a non-empty content digest"
            );
        }
        other => panic!("expected schema-emitted Recorded receipt, got {other:?}"),
    }
    assert_eq!(store.len(), 1);
    // PROOF the .sema file is real on disk.
    assert!(store.path().exists(), "the .sema database file exists");
}

#[test]
fn engine_lifecycle_runs_generated_trait_hooks_without_actor_mailboxes() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    engine.start().expect("generated lifecycle start hooks run");
    let output = engine.handle(input_record(entry("lifecycle still handles input")));
    engine.stop().expect("generated lifecycle stop hooks run");

    match output.root() {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected lifecycle-wrapped engine to accept record, got {other:?}"),
    }
}

#[test]
fn nexus_engine_trait_runs_nexus_decision_through_sema_state() {
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_record(entry("nexus trait root")))
        .with_origin_route(nexus_route(20));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);

    assert_eq!(nexus_output.origin_route(), nexus_route(20));
    match nexus_reply_output(&nexus_output) {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected NexusEngine to return a Signal reply root, got {other:?}"),
    }
    assert_eq!(
        nexus.store().len(),
        1,
        "NexusEngine is the computation plane that invokes SEMA for database work"
    );
}

#[test]
fn signal_engine_trait_triages_signal_roots_to_nexus_and_back() {
    let sema = SemaFile::new();
    let signal_admission = SignalAdmission::default();
    let mut nexus = Nexus::new(sema.open_store());
    let signal_input = input_record(entry("signal trait root"));

    let nexus_input = signal_admission.triage(signal_input, route(21));
    assert_eq!(nexus_input.origin_route(), nexus_route(21));
    assert!(matches!(nexus_signal_input(&nexus_input), Input::Record(_)));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);
    let signal_output = signal_admission.reply(nexus_output);

    assert_eq!(signal_output.origin_route(), route(21));
    match signal_output.root() {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected SignalAdmission to return a Signal output root, got {other:?}"),
    }
    assert_eq!(nexus.store().len(), 1);
}

#[test]
fn sema_store_persists_records_across_reopen_of_the_same_sema_file() {
    // The durability proof (record 1007/1008, bead primary-q2au): write
    // records, drop the store, reopen from the same `.sema` path, and
    // observe the records survive with their commit ledger intact.
    let sema = SemaFile::new();

    let first_marker;
    {
        let mut store = sema.open_store();
        SemaEngine::apply(
            &mut store,
            sema_write_message(sema_record(entry("durable one")), 1),
        );
        let recorded = SemaEngine::apply(
            &mut store,
            sema_write_message(sema_record(entry("durable two")), 2),
        );
        first_marker = match recorded.into_root() {
            SemaWriteOutput::Recorded(receipt) => receipt.database_marker.clone(),
            other => panic!("expected Recorded, got {other:?}"),
        };
        assert_eq!(store.len(), 2);
        // store drops here, releasing the sema-engine file handle.
    }

    // Reopen from the SAME path — a fresh process would do exactly this.
    let mut reopened = sema.open_store();
    assert_eq!(
        reopened.len(),
        2,
        "records written before the drop survive the reopen"
    );

    // The commit ledger resumed: the next write is commit sequence 4,
    // not 1, proving the counter persisted, not just the records and the
    // implied referent-registration writes.
    let after = SemaEngine::apply(
        &mut reopened,
        sema_write_message(sema_record(entry("durable three")), 3),
    );
    match after.root() {
        SemaWriteOutput::Recorded(receipt) => {
            assert_short_record_identifier(&receipt.record_identifier);
            assert_eq!(*receipt.database_marker.commit_sequence.payload(), 4);
        }
        other => panic!("expected Recorded after reopen, got {other:?}"),
    }

    // An Observe against the reopened store finds a record written before
    // the drop, through the schema-emitted query path.
    let observed = SemaEngine::observe(&reopened, sema_read_message(sema_observe(query()), 4));
    assert!(
        matches!(observed.root(), SemaReadOutput::Observed(_)),
        "the reopened store observes a pre-drop record"
    );
    assert_ne!(
        *first_marker.state_digest.payload(),
        0,
        "the durable digest is content-addressed, not a zero placeholder"
    );
}

#[test]
fn sema_engine_queries_partial_and_full_domain_sets() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domains(&["runtime-triad", "schema"], "both")),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domains(&["runtime-triad"], "runtime only")),
            2,
        ),
    );

    let partial = SemaEngine::observe(
        &store,
        sema_read_message(sema_observe(partial_query(&["schema", "other"], None)), 3),
    );
    match partial.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "both"
            );
        }
        other => panic!("expected partial query to observe one record, got {other:?}"),
    }

    let full = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(full_query(
                &["runtime-triad", "schema"],
                Some(Kind::Decision),
            )),
            4,
        ),
    );
    match full.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "both"
            );
        }
        other => panic!("expected full query to require every domain, got {other:?}"),
    }
}

#[test]
fn sema_engine_queries_domain_scopes_by_prefix_breadth() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domain(
                Domain::Technology(Technology::Software(Software::Data(
                    DataLeaf::SchemaEvolution,
                ))),
                "software schema",
            )),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domain(
                Domain::Technology(Technology::Hardware(HardwareLeaf::Networking)),
                "hardware network",
            )),
            2,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domain(
                Domain::Information(Information::Documentation),
                "documentation",
            )),
            3,
        ),
    );

    let software = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(query_with_domain_scopes(domain_scopes_from_scopes(&[
                software_scope(),
            ]))),
            4,
        ),
    );
    match software.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "software schema"
            );
        }
        other => panic!("expected software scope to observe software record, got {other:?}"),
    }

    let technology = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(query_with_domain_scopes(domain_scopes_from_scopes(&[
                technology_scope(),
            ]))),
            5,
        ),
    );
    match technology.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 3);
        }
        other => {
            panic!("expected all scope to observe all records, got {other:?}")
        }
    }

    let leaf = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(query_with_domain_scopes(domain_scopes_from_scopes(&[
                schema_evolution_scope(),
            ]))),
            6,
        ),
    );
    match leaf.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "software schema"
            );
        }
        other => panic!("expected leaf scope to observe only the leaf record, got {other:?}"),
    }
}

#[test]
fn sema_engine_expands_symmetric_domain_equivalence_without_chaining() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domain(
                Domain::Technology(Technology::Hardware(HardwareLeaf::Networking)),
                "hardware network",
            )),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domain(
                Domain::Information(Information::Database),
                "information database",
            )),
            2,
        ),
    );

    let software_data = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(query_with_domain_scopes(domain_scopes_from_scopes(&[
                data_scope(),
            ]))),
            3,
        ),
    );
    match software_data.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "information database"
            );
        }
        other => panic!(
            "expected software-data scope to observe equivalent information database, got {other:?}"
        ),
    }

    let information_database = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(query_with_domain_scopes(domain_scopes_from_scopes(&[
                DomainScope::Information(signal_domain::InformationScope::Database),
            ]))),
            4,
        ),
    );
    match information_database.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "information database"
            );
        }
        other => panic!("expected information-database scope to observe itself, got {other:?}"),
    }

    assert!(
        !data_scope().contains_domain(&Domain::Technology(Technology::Hardware(
            HardwareLeaf::Networking
        ))),
        "equivalence expansion must not chain from database into unrelated hardware"
    );
}

#[test]
fn sema_engine_queries_description_keyword_spans() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domains(
                &["runtime-triad"],
                "guardian retrieval uses *Schema Language* and *NOTA* terms",
            )),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domains(
                &["runtime-triad"],
                "unmarked schema language remains full text only",
            )),
            2,
        ),
    );

    let observed = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(Query {
                keyword_match: KeywordMatch::all_keywords(keywords_from_slice(&[
                    "schema language",
                    "nota",
                ])),
                ..query()
            }),
            3,
        ),
    );
    match observed.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "guardian retrieval uses *Schema Language* and *NOTA* terms"
            );
        }
        other => panic!("expected keyword query to observe one record, got {other:?}"),
    }

    let missed = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(Query {
                keyword_match: KeywordMatch::all_keywords(keywords_from_slice(&[
                    "schema language",
                    "missing",
                ])),
                ..query()
            }),
            4,
        ),
    );
    assert!(
        matches!(missed.root(), SemaReadOutput::Missed(_)),
        "all-keyword query should require every requested keyword"
    );
}

#[test]
fn sema_engine_queries_description_full_text() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domains(
                &["runtime-triad"],
                "Guardian retrieval keeps full text as the recall floor",
            )),
            1,
        ),
    );

    let observed = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(Query {
                text_match: TextMatch::contains_text(SearchText::new("RECALL floor")),
                ..query()
            }),
            2,
        ),
    );
    match observed.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "Guardian retrieval keeps full text as the recall floor"
            );
        }
        other => panic!("expected full-text query to observe one record, got {other:?}"),
    }
}

#[test]
fn signal_admission_rejects_empty_keyword_and_text_queries() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let empty_keyword = engine.handle(input_observe(Query {
        keyword_match: KeywordMatch::all_keywords(Keywords::new(Vec::new())),
        ..query()
    }));
    match empty_keyword.root() {
        Output::Rejected(rejection) => {
            assert_eq!(
                rejection.payload().payload(),
                &ValidationError::EmptyKeyword
            );
        }
        other => panic!("expected EmptyKeyword rejection, got {other:?}"),
    }

    let empty_text = engine.handle(input_observe(Query {
        text_match: TextMatch::contains_text(SearchText::new("   ")),
        ..query()
    }));
    match empty_text.root() {
        Output::Rejected(rejection) => {
            assert_eq!(
                rejection.payload().payload(),
                &ValidationError::EmptySearchText
            );
        }
        other => panic!("expected EmptySearchText rejection, got {other:?}"),
    }
}

#[test]
fn sema_engine_observation_orders_by_certainty_then_importance() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_importance("tertiary low", Magnitude::Low)),
            1,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_importance("tertiary high", Magnitude::High)),
            2,
        ),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_importance("reaffirmed", Magnitude::VeryHigh)),
            3,
        ),
    );

    let observed = SemaEngine::observe(&store, sema_read_message(sema_observe(query()), 4));
    match observed.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 3);
            assert_short_record_identifier(&records.payload().payload()[0].record_identifier);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "reaffirmed"
            );
            assert_eq!(
                records.payload().payload()[1].entry.description.payload(),
                "tertiary high"
            );
            assert_eq!(
                records.payload().payload()[2].entry.description.payload(),
                "tertiary low"
            );
        }
        other => panic!("expected ranked observation records, got {other:?}"),
    }

    let high_importance_query = Query {
        importance_selection: ImportanceSelection::at_least_importance(Magnitude::VeryHigh.into()),
        ..query()
    };
    let filtered = SemaEngine::observe(
        &store,
        sema_read_message(sema_observe(high_importance_query), 5),
    );
    match filtered.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "reaffirmed"
            );
        }
        other => panic!("expected high-importance filtered record, got {other:?}"),
    }
}

#[test]
fn sema_engine_queries_privacy_as_directional_magnitude() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(sema_record(entry("open")), 1),
    );
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_privacy("private", Magnitude::High)),
            2,
        ),
    );

    let default = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(privacy_query(
                PrivacySelection::default_observation_privacy(),
            )),
            3,
        ),
    );
    let any = SemaEngine::observe(
        &store,
        sema_read_message(sema_observe(privacy_query(PrivacySelection::Any)), 4),
    );
    let high = SemaEngine::observe(
        &store,
        sema_read_message(
            sema_observe(privacy_query(PrivacySelection::at_least(Privacy::new(
                Magnitude::High,
            )))),
            5,
        ),
    );

    match default.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "open"
            );
        }
        other => panic!("expected default privacy query to observe open record, got {other:?}"),
    }
    match any.root() {
        SemaReadOutput::Observed(records) => assert_eq!(records.payload().payload().len(), 2),
        other => panic!("expected any privacy query to observe both records, got {other:?}"),
    }
    match high.root() {
        SemaReadOutput::Observed(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "private"
            );
        }
        other => panic!("expected high privacy query to observe private record, got {other:?}"),
    }
}

#[test]
fn public_private_record_shortcuts_project_to_privacy_queries_through_nexus() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let public_record = engine.handle(input_record(entry("public shortcut target")));
    match public_record.root() {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected public RecordAccepted, got {other:?}"),
    };
    let private_record = engine.handle(input_record(entry_with_privacy(
        "private shortcut target",
        Magnitude::High,
    )));
    let _private_marker = match private_record.root() {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
            engine.database_marker()
        }
        other => panic!("expected private RecordAccepted, got {other:?}"),
    };

    let public_observed = engine.handle(input_public_records(record_selection()));
    let public_stash = match public_observed.root() {
        Output::RecordsStashed(stashed) => {
            assert_eq!(*stashed.record_count.payload(), 1);
            stashed.stash_handle.clone()
        }
        other => panic!("expected public shortcut to stash matching records, got {other:?}"),
    };
    let private_observed = engine.handle(input_private_records(record_selection()));
    let private_stash = match private_observed.root() {
        Output::RecordsStashed(stashed) => {
            assert_eq!(*stashed.record_count.payload(), 1);
            stashed.stash_handle.clone()
        }
        other => panic!("expected private shortcut to stash matching records, got {other:?}"),
    };

    let public_records = engine.handle(input_lookup_stash(public_stash));
    match public_records.root() {
        Output::RecordsObserved(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "public shortcut target"
            );
            assert_eq!(
                records.payload().payload()[0].entry.privacy,
                Magnitude::Zero
            );
        }
        other => panic!("expected public shortcut stash lookup to return records, got {other:?}"),
    }

    let private_records = engine.handle(input_lookup_stash(private_stash));
    match private_records.root() {
        Output::RecordsObserved(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "private shortcut target"
            );
            assert_eq!(
                records.payload().payload()[0].entry.privacy,
                Magnitude::High
            );
        }
        other => panic!("expected private shortcut stash lookup to return records, got {other:?}"),
    }
}

#[test]
fn public_intent_returns_public_domain_ancestors_and_requested_leaves() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    for entry in [
        Entry {
            importance: Magnitude::Maximum.into(),
            ..entry_with_domain(Domain::All, "all intent ancestor")
        },
        Entry {
            importance: Magnitude::VeryHigh.into(),
            ..entry_with_domain(
                Domain::Technology(Technology::Software(Software::Data(DataLeaf::All))),
                "data ancestor",
            )
        },
        Entry {
            importance: Magnitude::High.into(),
            ..entry_with_domain(
                Domain::Technology(Technology::Software(Software::Data(DataLeaf::Persistence))),
                "persistence leaf",
            )
        },
        Entry {
            importance: Magnitude::Low.into(),
            ..entry_with_domain(
                Domain::Technology(Technology::Software(Software::Data(
                    DataLeaf::SchemaEvolution,
                ))),
                "schema leaf",
            )
        },
        entry_with_domain(
            Domain::Technology(Technology::Software(Software::Quality(
                spirit::schema::signal::QualityLeaf::Testing,
            ))),
            "quality sibling",
        ),
    ] {
        assert!(matches!(
            engine.handle(input_record(entry)).root(),
            Output::RecordAccepted(_)
        ));
    }

    let output = engine.handle(input_public_intent(DomainScopes::new(vec![
        DomainScope::from(Domain::Technology(Technology::Software(Software::Data(
            DataLeaf::Persistence,
        )))),
        schema_evolution_scope(),
    ])));

    match output.root() {
        Output::RecordsObserved(records) => {
            let descriptions = records
                .payload()
                .payload()
                .iter()
                .map(|record| record.entry.description.payload().as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                descriptions,
                vec![
                    "all intent ancestor",
                    "data ancestor",
                    "persistence leaf",
                    "schema leaf"
                ]
            );
        }
        other => panic!("expected PublicIntent to return observed records, got {other:?}"),
    }
}

#[test]
fn public_text_search_returns_ranked_public_records() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    for entry in [
        entry_with_domain(
            Domain::Technology(Technology::Software(Software::Engineering(
                spirit::schema::signal::EngineeringLeaf::Architecture,
            ))),
            "standardized routing protocol envelope",
        ),
        entry_with_domain(
            Domain::Technology(Technology::Software(Software::Distributed(
                spirit::schema::signal::DistributedLeaf::ProtocolDesign,
            ))),
            "routing fallback note",
        ),
        Entry {
            privacy: Magnitude::High.into(),
            ..entry_with_domain(
                Domain::Technology(Technology::Software(Software::Distributed(
                    spirit::schema::signal::DistributedLeaf::ProtocolDesign,
                ))),
                "private routing protocol note",
            )
        },
    ] {
        assert!(matches!(
            engine.handle(input_record(entry)).root(),
            Output::RecordAccepted(_)
        ));
    }

    let output = engine.handle(input_public_text_search("routing protocol"));

    match output.root() {
        Output::RecordsObserved(records) => {
            let descriptions = records
                .payload()
                .payload()
                .iter()
                .map(|record| record.entry.description.payload().as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                descriptions,
                vec![
                    "standardized routing protocol envelope",
                    "routing fallback note"
                ]
            );
        }
        other => panic!("expected PublicTextSearch to return public records, got {other:?}"),
    }
}

#[test]
fn sema_engine_lookup_and_count_are_read_plane_operations() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    let first = SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domains(&["runtime-triad", "schema"], "first")),
            1,
        ),
    );
    let first_identifier = match first.into_root() {
        SemaWriteOutput::Recorded(receipt) => receipt.record_identifier.clone(),
        other => panic!("expected first Recorded receipt, got {other:?}"),
    };
    let second = SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domains(&["runtime-triad"], "second")),
            2,
        ),
    );
    let second_identifier = match second.into_root() {
        SemaWriteOutput::Recorded(receipt) => receipt.record_identifier.clone(),
        other => panic!("expected second Recorded receipt, got {other:?}"),
    };
    assert_ne!(first_identifier, second_identifier);

    let found = SemaEngine::observe(
        &store,
        sema_read_message(sema_lookup(second_identifier.clone()), 3),
    );
    match found.root() {
        SemaReadOutput::Found(record) => {
            assert_eq!(record.record_identifier, second_identifier);
            assert_eq!(record.entry.description.payload(), "second");
        }
        other => panic!("expected SEMA lookup to find the second record, got {other:?}"),
    }

    let counted = SemaEngine::observe(
        &store,
        sema_read_message(sema_count(partial_query(&["runtime-triad"], None)), 4),
    );
    match counted.root() {
        SemaReadOutput::Counted(records) => {
            assert_eq!(*records.payload().payload().payload(), 2);
        }
        other => panic!("expected SEMA count to return two records, got {other:?}"),
    }
}

#[test]
fn sema_engine_observes_through_shared_reference_for_parallel_readers() {
    let sema = SemaFile::new();
    let mut store = sema.open_store();
    SemaEngine::apply(
        &mut store,
        sema_write_message(
            sema_record(entry_with_domains(&["runtime-triad", "schema"], "parallel")),
            1,
        ),
    );

    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|offset| {
                let store = &store;
                scope.spawn(move || {
                    SemaEngine::observe(
                        store,
                        sema_read_message(
                            sema_observe(full_query(&["runtime-triad"], None)),
                            10 + offset,
                        ),
                    )
                })
            })
            .collect();

        for worker in workers {
            let observed = worker.join().expect("reader thread joins");
            assert!(matches!(observed.root(), SemaReadOutput::Observed(_)));
        }
    });
}

#[test]
fn nexus_runs_sema_while_holding_mail_then_replies_through_schema_objects() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let recorded = engine.handle(input_record(entry("nexus drives sema")));
    assert_eq!(recorded.origin_route(), route(1));
    match recorded.root() {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    }
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);
    assert_eq!(engine.in_flight_mail_count(), 0);
    assert!(engine.mail_ledger().is_empty());
    assert_eq!(engine.record_count(), 1);
}

#[test]
fn signal_admission_rejects_invalid_input_with_schema_emitted_rejection_before_mail_or_sema() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();
    let mut bad = entry("missing domain");
    bad.domains = domain_fixtures::domains(&[""]);

    let output = engine.handle(input_record(bad));

    assert_eq!(
        output.root(),
        &Output::rejected(SignalRejection::new(ValidationError::EmptyDomain))
    );
    assert_eq!(output.origin_route(), route(1));
    assert_eq!(engine.record_count(), 0);
    assert_eq!(engine.sent_message_count(), 0);
    assert_eq!(engine.processed_message_count(), 0);
    assert_eq!(engine.mail_ledger(), []);
}

#[test]
fn sema_read_miss_completion_routes_through_runner_loop_to_error_reply() {
    // Designer 480: the SemaReadCompleted(Missed) fact becomes
    // ReplyToSignal(Error) through the runner loop's decide step.
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = NexusWork::sema_read_completed(SemaReadOutput::missed(ErrorReport::new(
        ErrorMessage::new("no matching record"),
    )))
    .with_origin_route(nexus_route(7));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);
    let signal_output = SignalAdmission::default().reply(nexus_output);

    assert_eq!(
        signal_output.root(),
        &Output::error(ErrorReport::new(ErrorMessage::new("no matching record")))
    );
    assert_eq!(signal_output.origin_route(), route(7));
}

#[test]
fn plane_envelopes_keep_payload_names_scoped() {
    // Designer 480: plane envelopes still enforce the per-plane scoping
    // of payload names. The Nexus envelope around a CommandSemaWrite
    // action is a distinct generated type from the Signal output
    // envelope that carries the eventual reply.
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_record(entry("language input")))
        .with_origin_route(nexus_route(11));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);
    assert_eq!(nexus_output.origin_route(), nexus_route(11));
    // The runner loop drove the full cycle to ReplyToSignal(RecordAccepted).
    assert!(matches!(
        nexus_reply_output(&nexus_output),
        Output::RecordAccepted(_)
    ));

    let signal_output = SignalAdmission::default().reply(nexus_output);
    assert_eq!(signal_output.origin_route(), route(11));
    assert!(matches!(signal_output.root(), Output::RecordAccepted(_)));
}

#[test]
fn nexus_and_sema_have_explicit_input_output_languages() {
    // Designer 480: the Nexus and SEMA languages remain explicit, but the
    // step-of-decision is now driven by the runner loop. Probing one cycle
    // of NexusEngine::execute proves both languages cross cleanly.
    let sema = SemaFile::new();
    let mut nexus = Nexus::new(sema.open_store());
    let nexus_input = nexus_signal_arrived(input_record(entry("language input")))
        .with_origin_route(nexus_route(13));

    let nexus_output = execute_nexus(&mut nexus, nexus_input);
    assert!(matches!(
        nexus_reply_output(&nexus_output),
        Output::RecordAccepted(_)
    ));

    // Standalone SEMA completion fact also routes back to the right reply.
    let sema_output = SemaWriteOutput::recorded(SemaReceipt {
        record_identifier: record_identifier("003g"),
        database_marker: DatabaseMarker {
            commit_sequence: 4.into(),
            state_digest: 127.into(),
        },
    });
    let nexus_input_from_sema =
        NexusWork::sema_write_completed(sema_output).with_origin_route(nexus_route(14));
    let mut second_nexus = Nexus::new(SemaFile::new().open_store());
    let nexus_output_from_sema = execute_nexus(&mut second_nexus, nexus_input_from_sema);
    let signal_output = SignalAdmission::default().reply(nexus_output_from_sema);
    assert!(matches!(signal_output.root(), Output::RecordAccepted(_)));
}

#[cfg(feature = "nota-text")]
#[test]
fn import_export_paths_use_single_colon_namespaces() {
    let import = Import {
        source_path: String::from("signal:sema:Magnitude").into(),
        local_path: String::from("spirit:core:Magnitude").into(),
    };
    let export = Export {
        local_path: String::from("spirit:core:SemaWriteOutput").into(),
        public_path: String::from("spirit:sema:SemaWriteOutput").into(),
    };

    assert_eq!(
        import.to_nota(),
        "(signal:sema:Magnitude spirit:core:Magnitude)"
    );
    assert_eq!(
        export.to_nota(),
        "(spirit:core:SemaWriteOutput spirit:sema:SemaWriteOutput)"
    );
}

#[test]
fn full_runtime_triad_records_then_observes_through_durable_sema_with_stash() {
    // Designer 480 layer-2 witness for the Stash effect (operator 287 §
    // "Acceptance Tests"): Observe with a non-empty result drives the
    // recursive Nexus loop through Stash and returns a RecordsStashed
    // reply carrying both the records and a recovery handle. A follow-up
    // LookupStash by handle returns the same records.
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let recorded = engine.handle(input_record(entry("full runtime triad works")));
    assert_eq!(recorded.origin_route(), route(1));
    let record_marker = match recorded.root() {
        Output::RecordAccepted(receipt) => {
            assert_short_record_identifier(receipt.payload());
            engine.database_marker()
        }
        other => panic!("expected RecordAccepted, got {other:?}"),
    };
    assert_eq!(*record_marker.commit_sequence.payload(), 2);
    assert_eq!(engine.sent_message_count(), 1);
    assert_eq!(engine.processed_message_count(), 1);
    assert_eq!(engine.in_flight_mail_count(), 0);
    assert!(engine.mail_ledger().is_empty());

    let observed = engine.handle(input_observe(Query {
        domain_match: DomainMatch::full(domain_fixtures::scopes(&["runtime-triad"])),
        keyword_match: KeywordMatch::Any,
        text_match: TextMatch::Any,
        referent_selection: spirit::schema::signal::ReferentSelection::Any,
        selected_kind: SelectedKind::new(Some(Kind::Decision)),
        privacy_selection: PrivacySelection::default_observation_privacy(),
        certainty_selection: CertaintySelection::default_observation_certainty(),
        importance_selection: ImportanceSelection::default_observation_importance(),
    }));

    assert_eq!(observed.origin_route(), route(2));
    let stash_handle = match observed.root() {
        Output::RecordsStashed(stashed) => {
            assert_eq!(*stashed.record_count.payload(), 1);
            assert_short_record_identifier(
                &stashed.observed_records.payload().payload()[0].record_identifier,
            );
            assert_eq!(
                stashed.observed_records.payload().payload()[0].entry,
                entry("full runtime triad works")
            );
            stashed.stash_handle.clone()
        }
        other => panic!("expected RecordsStashed reply after Observe, got {other:?}"),
    };
    assert_eq!(engine.sent_message_count(), 2);
    assert_eq!(engine.processed_message_count(), 2);
    assert_eq!(engine.in_flight_mail_count(), 0);
    assert!(engine.mail_ledger().is_empty());
    assert_eq!(engine.stash_table().len(), 1);

    // Recovery witness: one LookupStash consumes the returned recovery handle.
    let looked_up = engine.handle(input_lookup_stash(stash_handle.clone()));
    assert_eq!(looked_up.origin_route(), route(3));
    match looked_up.root() {
        Output::RecordsObserved(records) => {
            assert_short_record_identifier(&records.payload().payload()[0].record_identifier);
            assert_eq!(
                records.payload().payload()[0].entry,
                entry("full runtime triad works")
            );
        }
        other => panic!(
            "expected LookupStash to return RecordsObserved with full records, got {other:?}"
        ),
    }
    assert_eq!(engine.sent_message_count(), 3);
    assert_eq!(engine.processed_message_count(), 3);
    assert_eq!(engine.in_flight_mail_count(), 0);
    assert!(engine.mail_ledger().is_empty());
    assert!(engine.stash_table().is_empty());

    let stale_lookup = engine.handle(input_lookup_stash(stash_handle));
    assert!(
        matches!(stale_lookup.root(), Output::Rejected(_)),
        "a consumed recovery handle must not retain or replay stale records"
    );
    assert!(engine.stash_table().is_empty());
    assert!(engine.mail_ledger().is_empty());
}

#[test]
fn full_runtime_triad_looks_up_and_counts_through_signal_nexus_and_sema() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let first = engine.handle(input_record(entry_with_domains(
        &["runtime-triad", "schema"],
        "lookup one",
    )));
    let first_identifier = match first.into_root() {
        Output::RecordAccepted(receipt) => receipt.payload().clone(),
        other => panic!("expected first RecordAccepted, got {other:?}"),
    };
    engine.handle(input_record(entry_with_domains(
        &["runtime-triad"],
        "lookup two",
    )));

    let found = engine.handle(input_lookup(first_identifier.clone()));
    match found.root() {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, first_identifier);
            assert_eq!(record.entry.description.payload(), "lookup one");
        }
        other => panic!("expected full runtime lookup to return RecordFound, got {other:?}"),
    }

    let counted = engine.handle(input_count(partial_query(&["runtime-triad"], None)));
    match counted.root() {
        Output::RecordsCounted(records) => {
            assert_eq!(*records.payload().payload().payload(), 2);
        }
        other => panic!("expected full runtime count to return RecordsCounted, got {other:?}"),
    }
}

#[test]
fn full_runtime_triad_reports_database_marker_only_through_marker_operation() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let initial = engine.handle(Input::Marker);
    match initial.root() {
        Output::MarkerReported(marker) => {
            assert_eq!(marker.payload().commit_sequence.payload(), &0);
            assert_eq!(marker.payload().state_digest.payload(), &0);
        }
        other => panic!("expected initial MarkerReported, got {other:?}"),
    }

    let recorded = engine.handle(input_record(entry("marker operation target")));
    assert!(
        matches!(recorded.root(), Output::RecordAccepted(_)),
        "record should not return marker state: {recorded:?}"
    );

    let marker = engine.handle(Input::Marker);
    match marker.root() {
        Output::MarkerReported(marker) => {
            assert_eq!(marker.payload().commit_sequence.payload(), &2);
            assert_ne!(marker.payload().state_digest.payload(), &0);
        }
        other => panic!("expected MarkerReported after write, got {other:?}"),
    }
}

#[test]
fn full_runtime_triad_registers_referent_through_signal_nexus_and_sema() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let registered = engine.handle(input_register_referent("schema", &["schema-alias"]));

    match registered.root() {
        Output::ReferentRegistered(receipt) => {
            assert_eq!(receipt.payload().payload().payload(), "schema");
        }
        other => panic!("expected ReferentRegistered, got {other:?}"),
    }
}

#[test]
fn full_runtime_triad_records_and_registers_embedded_referent() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let output = engine.handle(input_record(entry_with_referents(
        "embedded referent should register",
        &["schema"],
    )));

    match output.root() {
        Output::RecordAccepted(receipt) => assert_short_record_identifier(receipt.payload()),
        other => panic!("expected RecordAccepted for embedded referent, got {other:?}"),
    }
    let observed = engine.handle(input_observe(Query {
        referent_selection: ReferentSelection::any_referent(referents_from_slice(&["schema"])),
        ..query()
    }));
    let stash = match observed.root() {
        Output::RecordsStashed(stashed) => stashed.stash_handle.clone(),
        other => panic!("expected RecordsStashed for embedded referent query, got {other:?}"),
    };
    let records = engine.handle(input_lookup_stash(stash));
    match records.root() {
        Output::RecordsObserved(records) => {
            assert_eq!(records.payload().payload().len(), 1);
            assert_eq!(
                records.payload().payload()[0].entry.description.payload(),
                "embedded referent should register"
            );
        }
        other => panic!("expected RecordsObserved for embedded referent query, got {other:?}"),
    }
}

#[test]
fn full_runtime_triad_proposes_and_registers_embedded_referent() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let output = engine.handle(input_propose(entry_with_referents(
        "embedded proposal referent should register",
        &["schema"],
    )));

    match output.root() {
        Output::Proposed(receipt) => assert_short_record_identifier(receipt.payload()),
        other => panic!("expected Proposed for embedded referent, got {other:?}"),
    }
}

#[test]
fn full_runtime_triad_change_record_registers_embedded_referent() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();

    let accepted = engine.handle(input_record(entry(
        "record before embedded referent change",
    )));
    let record_identifier = match accepted.root() {
        Output::RecordAccepted(receipt) => receipt.payload().clone(),
        other => panic!("expected setup RecordAccepted, got {other:?}"),
    };
    let changed = engine.handle(input_change_record(
        record_identifier.clone(),
        entry_with_referents("record after embedded referent change", &["schema"]),
    ));

    match changed.root() {
        Output::RecordChanged(receipt) => {
            assert_eq!(
                receipt.payload().payload().payload(),
                record_identifier.payload()
            )
        }
        other => panic!("expected RecordChanged for embedded referent change, got {other:?}"),
    }
    let found = engine.handle(Input::lookup(record_identifier));
    match found.root() {
        Output::RecordFound(record) => {
            assert_eq!(record.entry.referents.payload(), &[Referent::new("schema")]);
        }
        other => panic!("expected RecordFound after embedded referent change, got {other:?}"),
    }
}

#[test]
fn full_runtime_triad_canonicalizes_referent_aliases_on_write_and_query() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();
    engine.handle(input_register_referent("schema", &["schema-alias"]));

    let recorded = engine.handle(input_record(entry_with_referents(
        "alias referent canonicalizes",
        &["schema-alias"],
    )));
    let identifier = match recorded.into_root() {
        Output::RecordAccepted(receipt) => receipt.payload().clone(),
        other => panic!("expected RecordAccepted, got {other:?}"),
    };

    let found = engine.handle(input_lookup(identifier.clone()));
    match found.root() {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, identifier);
            assert_eq!(
                record.entry.referents.payload(),
                &vec![Referent::new("schema")]
            );
        }
        other => panic!("expected RecordFound, got {other:?}"),
    }

    let observed = engine.handle(input_observe(Query {
        referent_selection: ReferentSelection::any_referent(referents_from_slice(&[
            "schema-alias",
        ])),
        ..query()
    }));
    match observed.root() {
        Output::RecordsStashed(stash) => {
            assert_eq!(*stash.record_count.payload(), 1);
        }
        other => panic!("expected alias query to match the canonical record, got {other:?}"),
    }
}

#[test]
fn full_runtime_triad_canonicalizes_referent_aliases_on_propose() {
    let sema = SemaFile::new();
    let mut engine = sema.engine();
    engine.handle(input_register_referent("schema", &["schema-alias"]));

    let proposed = engine.handle(input_propose(entry_with_referents(
        "alias referent canonicalizes on proposal",
        &["schema-alias"],
    )));
    let identifier = match proposed.into_root() {
        Output::Proposed(receipt) => receipt.payload().clone(),
        other => panic!("expected Proposed, got {other:?}"),
    };

    let found = engine.handle(input_lookup(identifier.clone()));
    match found.root() {
        Output::RecordFound(record) => {
            assert_eq!(record.record_identifier, identifier);
            assert_eq!(
                record.entry.referents.payload(),
                &vec![Referent::new("schema")]
            );
        }
        other => panic!("expected RecordFound, got {other:?}"),
    }
}
