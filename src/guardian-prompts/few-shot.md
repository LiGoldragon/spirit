WORKED EXAMPLES. Study the contrastive pairs — within a pair only the tested feature differs.

[Record — the burden pair, the single most important lesson]
A) Entry Certainty High; Testimony [I could maybe use the schema-derived contracts to emit most of the client side]; Reasoning argues the schema should emit most client machinery. The quote hedges (could, maybe) and cannot clear High. -> (Reject (Overstated [could and maybe are hedged and clear only Low; High is unearned]))
B) The SAME Testimony, Entry Certainty Low. The hedge honestly clears Low. -> Accept

[Record — orthogonal axes]
C) Entry Certainty VeryLow, Importance High; Testimony [I keep coming back to whether the guardian should be one model or two, I really am not sure yet]; Reasoning notes the topic recurs across three sessions and blocks the guardian design. Tentative wording clears VeryLow; recurrence + blocking supports High importance. -> Accept
D) Entry Certainty VeryLow, Importance High; Testimony [maybe two models could be interesting]; Reasoning asserts High importance with no recurrence or blast-radius basis. -> (Reject (ImportanceUnsupported [no recurrence or blast-radius evidence is offered for High importance]))
AA) Entry Certainty Medium, Importance High, Privacy Maximum; Testimony [this guardian-policy record is High importance and Maximum privacy]; Reasoning notes the psyche directly named both metadata rungs. The named rungs are direct evidence for those values. -> Accept
AB) Entry Certainty Medium, Importance High; Testimony [the guardian should keep testimony and reasoning separate]; Reasoning argues High importance because the split controls every guarded Spirit write, affects the intent layer's coherence, and blocks prompt alignment. Architectural centrality + blocking supports High importance even without a direct rung declaration. -> Accept

[Record — testimony production]
E) Entry any; Testimony empty; Reasoning is a confident paraphrase of what the psyche supposedly wants. No verbatim quote. -> (Reject (MissingTestimony [no verbatim psyche quote is supplied]))
F) Entry Decision; Testimony [the architecture decision is finalized and the team will proceed accordingly per our alignment]. That sentence reads like agent prose, not how this psyche talks. -> (Reject (TestimonyFabricated [the quote reads like polished agent prose, not a human utterance]))
G) Entry Decision High; Testimony quote [yes do that] with Antecedent [shall we make the daemon reject inline NOTA configuration?]. The bare affirmation is anchored by its antecedent and clears High. -> Accept
H) The SAME [yes do that] with NO antecedent. Meaningless alone. -> (Reject (MissingTestimony [a bare yes carries no arrow without its antecedent]))

[Record — shape and classification]
I) One Entry whose Description bundles a key-resolution rule AND a deploy-cadence rule. -> (Reject (Compound [key resolution and deploy cadence are two separable arrows]))
J) Testimony [I am not sure the rebuild is ready, let me look again]; Reasoning records it as a Constraint. Transient uncertainty, not durable intent. -> (Reject (NonIntent [a momentary not-sure-yet is task state, not a durable arrow]))
K) Entry Description [Canonical prose names are criome for the authentication component and criomos for the operating system name; creome and creomos are misspellings]; Testimony [its criome and criomos, not creome and creomos]. The description centers the forbidden spellings instead of only stating the canonical names. -> (Reject (NegativeGuideline [the rule is framed around rejected spellings; reword as the affirmative canonical naming rule]))
L) The SAME Testimony, Entry Description [Canonical prose uses criome for the authentication component and criomos for the operating system name; exact on-disk path spelling is preserved when citing repository paths]. The positive names are the center; the path citation boundary is affirmative. -> Accept
M) Entry Domains [spirit]; the daemon name is a particular. -> (Reject (UnclearDomain [spirit is a referent, not a universal domain; classify by subject like AdmissionControl]))

[Record — cross-record collision]
N) Candidate restates a forward arrow already present verbatim in the bundle. -> (Reject (Duplicate [the same forward arrow already lives in record in the bundle]))
O) Candidate says daemons MAY parse NOTA config; the bundle holds a live psyche arrow that daemons NEVER parse NOTA, and no quote authorizes reversing it. -> (Reject (Contradiction [negates the live daemons-never-parse-NOTA arrow with no authorizing psyche quote]))
Y) Candidate is a fresh Record whose reasoning says it refines target t00s, and the bundle contains t00s as the live record holding that arrow. The quote supports tightening t00s, not creating a sibling record. -> (Reject (InsufficientWarrant [the testimony licenses editing t00s, not a fresh Record; remand for Clarify or Supersede]))
Z) Candidate is a fresh Record saying daemons MAY parse NOTA config; the bundle holds daemons NEVER parse NOTA; Testimony [change the daemon config rule so daemons may parse NOTA config now]. The psyche authorizes a reversal, but a fresh sibling would leave the old arrow live. -> (Reject (InsufficientWarrant [the testimony authorizes replacement, but the repair shape is Supersede or ChangeRecord, not a fresh Record]))

[Clarify — sharpen vs trample]
P) Target says the guardian is binary; Clarify adds that a reject is a remand the agent re-pleads. Same arrow, sharper. -> Accept
Q) Target says the guardian is binary; Clarify rewrites it to allow admitting at a corrected certainty. That reverses the arrow. -> (Reject (ClarifyTramples [admitting-at-corrected-certainty inverts the binary arrow; that is a Supersede, not a Clarify]))

[Supersede — multi-replacement preservation and authorization]
R) Retire two distinct live arrows (X: testimony stores raw words; Y: asterisks are a render marker) and install TWO replacements preserving both; Testimony carries the psyche quote [supersede those two with these]. -> Accept
S) The SAME two targets collapsed into ONE replacement that keeps only X. -> (Reject (ClarifyLosesMeaning [a single replacement drops arrow Y; preserve both or it loses meaning]))
T) Supersede a live psyche record; Reasoning argues only that the agent judges it stale; no psyche quote authorizes the retirement. -> (Reject (InsufficientWarrant [no verbatim psyche authorization to retire a psyche arrow; staleness judged by the agent is not enough]))
U) Supersede names target abcd, but abcd is absent from the bundle. -> (Reject (SupersedeTargetMissing [target abcd is not in the bundle and cannot be judged]))

[Retire / ChangeRecord / ChangeCertainty]
V) Retire a record; Testimony [kill that rule, we are not doing backward compatibility]. Verbatim psyche authorization. -> Accept
W) ChangeRecord fixes a typo in a Description, same arrow, same magnitudes. -> Accept
X) ChangeRecord keeps the wording but raises Certainty from Medium to Maximum; Testimony is the original [we should probably do this]. The words still clear only Medium. -> (Reject (Overstated [should probably clears Medium; Maximum is unearned by the quote]))
