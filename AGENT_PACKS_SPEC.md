# PushOS Agent Packs

Status: Product architecture add-on  
Related documents:

- `SPEC.md`
- `AGENTS.md`

Purpose:

Define how PushOS supports pre-built business assistants, reusable agent packs, learning assistants, process optimisation, agent creation, and continuous improvement of a user's workflows.

---

# 1. Vision

PushOS should not require users to start with an empty controller and manually invent every agent.

It should ship with useful, installable **Agent Packs** that immediately turn Push 2 into a functional AI operating system for a role, team or business.

Examples:

- Developer Pack
- Business Pack
- Growth Pack
- Agency Pack
- Research Pack
- Learning Pack

An Agent Pack may contain:

- agents
- workflows
- pages
- bindings
- prompts
- memory policies
- tool requirements
- provider preferences
- permissions
- scheduled tasks
- event subscriptions
- KPIs
- UI metadata

The system must remain flexible enough for users and the community to create and share their own packs.

---

# 2. Core principle

An Agent Pack is not simply a collection of prompts.

It is a declarative package describing a working AI team.

Conceptually:

```text
Agent Pack
│
├── Agents
├── Workflows
├── Pages
├── Bindings
├── Triggers
├── Memory Policies
├── Permissions
├── KPIs
└── Tool Requirements
```

Installing a pack should create an immediately useful operating environment.

---

# 3. Key product objective

PushOS should become progressively more useful as the user works.

The desired loop is:

```text
User works
↓
PushOS captures structured activity
↓
Patterns accumulate
↓
Process Optimiser detects friction/opportunity
↓
Agent Architect proposes automation
↓
Human approves
↓
New workflow/agent is created
↓
Tutor explains the improvement
↓
Future work becomes easier
```

This is a core product capability.

It must not become uncontrolled autonomous self-modification.

All significant system changes require explicit approval.

---

# 4. Agent Pack directory structure

Recommended format:

```text
seo-pack/
├── pack.toml
├── README.md
│
├── agents/
│   ├── seo-strategist.toml
│   ├── technical-seo.toml
│   └── content-analyst.toml
│
├── workflows/
│   ├── site-audit.toml
│   ├── content-gap.toml
│   └── weekly-review.toml
│
├── pages/
│   └── seo.toml
│
├── bindings/
│   └── default.toml
│
├── prompts/
│   └── optional reusable prompt files
│
└── assets/
    └── optional icons/sprites
```

Packs should remain human-readable and Git-friendly.

---

# 5. Pack manifest

Example:

```toml
id = "seo"
name = "SEO Growth Pack"
version = "1.0.0"

description = """
SEO agents and workflows for technical audits,
content optimisation and opportunity discovery.
"""

author = "PushOS"
license = "Apache-2.0"

minimum_pushos_version = "0.1.0"

[requirements]
network = true

[recommended_providers]
analysis = ["claude", "codex"]
cheap = ["local"]

[[permissions]]
id = "network"
required = true

[[permissions]]
id = "filesystem.read"
required = false
```

Pack installation must validate all requirements before activation.

---

# 6. Installation model

Target UX:

```bash
pushos pack install seo
```

Future Studio UX:

```text
Agent Packs

SEO Growth Pack
Developer Pack
Agency Pack
Business Pack

[INSTALL]
```

Before installation, PushOS displays:

```text
SEO Growth Pack requests:

✓ Network access
✓ Read project files
✗ Production deployment
✗ Email sending

Adds:

3 agents
5 workflows
1 page
8 default bindings

[INSTALL]
[CANCEL]
```

No pack receives hidden permissions.

---

# 7. Pack lifecycle

Support:

```text
install
enable
disable
update
remove
export
clone
```

Removing a pack must not silently delete user-created data or historical records.

User customisations should be preserved separately from pack defaults.

---

# 8. Defaults versus user overrides

Pack content is immutable package configuration.

User changes become overrides.

Example:

```text
Pack default:
Pad 4 → Technical Audit

User override:
Pad 4 → Competitor Analysis
```

Updating the pack must not overwrite user bindings.

Precedence:

```text
user override
↓
workspace override
↓
pack default
```

---

# 9. Recommended built-in packs

Initial distribution should include several high-quality official packs.

Do not ship dozens of shallow assistants.

Prefer a few excellent packs.

---

# 10. Developer Pack

Purpose:

Manage software development using multiple AI coding agents.

Suggested agents:

```text
Architect
Builder
Reviewer
Tester
Security Reviewer
Debugger
Documentation
Tutor
```

Suggested workflows:

```text
Plan → Build → Test → Review
Bug Investigation
Security Review
Release Check
Architecture Review
```

Possible Push page:

```text
[ARCH] [BUILD] [REVIEW] [TEST]
[BUG ] [SEC  ] [DOCS  ] [TUTOR]
```

---

# 11. Business Pack

Purpose:

Provide an executive operating layer.

Suggested agents:

```text
CEO
Chief of Staff
Process Optimiser
Agent Architect
Researcher
Risk Analyst
KPI Analyst
Memory Curator
```

Suggested workflows:

```text
Morning Brief
Weekly Business Review
Decision Review
Opportunity Review
Process Improvement Review
```

---

# 12. Growth Pack

Suggested agents:

```text
SEO Strategist
Technical SEO Analyst
Content Researcher
Content Planner
Competitor Analyst
Analytics Analyst
CRO Analyst
Growth Strategist
```

Suggested workflows:

```text
Technical SEO Audit
Content Gap Analysis
Page Review
Keyword/Topic Research
Competitor Review
Weekly Growth Brief
```

---

# 13. Agency Pack

Suggested agents:

```text
Client Success
Project Manager
Scope Guardian
Proposal Assistant
Meeting Analyst
QA Reviewer
Reporting Assistant
Upsell Finder
```

Suggested workflows:

```text
Client Onboarding
Weekly Client Review
Scope Change Detection
Meeting Follow-up
Monthly Reporting
Opportunity Review
```

---

# 14. Learning Pack

Suggested agents:

```text
Tutor
Learning Manager
Socratic Coach
Quiz Builder
Knowledge Gap Detector
Practice Generator
```

This pack is particularly important because learning should happen from real work.

---

# 15. Tutor agent

Tutor is a first-class PushOS assistant.

Purpose:

Help the user understand the work being performed by agents and progressively improve their own knowledge.

Tutor may receive context from:

- selected workspace
- selected file
- completed coding session
- workflow output
- error
- architecture decision
- previous lesson history

Example:

```text
User selects Claude Builder.

Hold TUTOR:

"Explain what it changed in the queue
and why this design avoids race conditions."
```

Tutor should answer using the actual working context.

---

# 16. Tutor levels

Support configurable explanation level:

```text
ELI5
Beginner
Intermediate
Developer
Senior
Expert
```

An encoder may adjust the level dynamically.

---

# 17. Tutor modes

Suggested modes:

```text
Explain
Example
Analogy
Quiz
Challenge
Review
Build Together
```

Tutor should not merely generate long lectures.

It should adapt to the user's current context.

---

# 18. Learning profile

Maintain a non-sensitive learning profile containing concepts such as:

```text
understood
learning
needs_review
unknown
```

Example:

```text
Rust ownership        learning
Tokio channels        understood
ACP                   learning
event sourcing        needs_review
```

This data is about product-learning state, not psychological profiling.

---

# 19. Knowledge gap detection

Agents may emit structured events:

```text
learning.gap_detected
```

Example:

```text
topic:
Tokio cancellation

evidence:
User requested explanation twice during
workflow debugging.

confidence:
0.81
```

Learning Manager decides whether to add it to the learning queue.

---

# 20. Learning from real work

After significant sessions, PushOS may optionally ask:

```text
You used Git worktrees heavily today.

Want a 5-minute explanation?

[YES] [LATER]
```

Learning should remain contextual.

Avoid creating unnecessary interruptions.

---

# 21. Process Optimiser

Process Optimiser is a core business assistant.

Purpose:

Find repetitive work, inefficient workflows, bottlenecks and automation opportunities.

It should operate primarily on structured PushOS events.

It must not require continuously sending all user activity to an LLM.

---

# 22. Process Optimiser inputs

Examples:

```text
action.started
action.completed

workflow.started
workflow.completed
workflow.failed

agent.started
agent.completed

approval.requested

terminal.started

workspace.selected

manual_action.repeated
```

It may also inspect workflow metadata where permission allows.

---

# 23. Process patterns

The optimiser may detect:

### Repetition

```text
Same sequence executed 8 times this week.
```

### Excessive manual handoff

```text
User repeatedly moves output from Agent A
to Agent B manually.
```

### Failure hotspot

```text
Workflow node "Deploy Preview"
failed in 34% of recent runs.
```

### Human bottleneck

```text
Four workflows are regularly waiting
for the same low-risk approval.
```

### Unnecessary expensive model usage

```text
Premium reasoning model is being used for
a deterministic formatting step.
```

### Slow workflow

```text
Testing repeatedly consumes most of the
workflow duration.
```

---

# 24. Optimisation proposals

Process Optimiser must create proposals, not silently modify behaviour.

Example:

```text
PROCESS OPTIMISER

I noticed you run:

Tests
→ Claude Review
→ Codex Fix

manually several times per day.

Suggested workflow:

REVIEW LOOP

Estimated saving:
~12 interactions/day

[CREATE]
[DETAILS]
[IGNORE]
```

---

# 25. Proposal model

```rust
struct OptimisationProposal {
    id: ProposalId,
    evidence: Vec<Evidence>,
    recommendation: Recommendation,
    confidence: f32,
    estimated_benefit: Option<Benefit>,
}
```

Evidence must be inspectable.

Avoid unsupported claims such as exact time savings unless measured.

---

# 26. Optimisation confidence

Suggestions should require enough evidence.

Do not propose automation because something happened once.

Example configurable thresholds:

```text
repeated_manual_pattern:
minimum occurrences = 3

workflow_failure:
minimum sample = 5

agent creation:
minimum repeated pattern = 3
```

---

# 27. Scheduling optimisation analysis

Do not run Process Optimiser continuously.

Possible triggers:

```text
end of work session
daily
weekly
after N events
manual
```

Default recommendation:

daily lightweight analysis plus weekly deeper review.

---

# 28. Agent Architect

Agent Architect creates proposed new agents.

It must never autonomously create high-permission agents without approval.

Inputs may come from:

- Process Optimiser
- direct user request
- workflow failures
- repeated manual work
- missing capability detection

---

# 29. Agent proposal example

```text
NEW AGENT PROPOSAL

Name:
SEO Page Reviewer

Objective:
Review newly published pages for
technical and on-page SEO issues.

Trigger:
page.publish.completed

Tools:
HTTP
SEO crawler

Worker:
Local first
Claude fallback

Permissions:
Network read only

Suggested Pad:
Growth / Pad 14

[CREATE]
[EDIT]
[CANCEL]
```

---

# 30. Agent Architect output

It should produce a complete agent definition:

```text
name
objective
inputs
outputs
trigger
tools
provider policy
permissions
memory policy
KPI
escalation policy
workflow links
recommended page
recommended binding
```

The result is configuration.

No Rust code should be necessary for normal agent creation.

---

# 31. Agent creation through voice

Example:

```text
Hold +AGENT

"Create an agent that checks new website pages
for SEO issues whenever we deploy."
```

PushOS:

1. transcribes request
2. Agent Architect constructs proposal
3. validates required tools
4. calculates requested permissions
5. shows proposal
6. human approves
7. persists agent configuration
8. optionally assigns to Push

---

# 32. Memory Curator

Optional Business Pack agent.

Purpose:

Review accumulated observations and propose useful durable knowledge.

It should not be required for PushOS operation.

It works through the generic Memory API.

Example:

```text
Observed repeatedly:

Client X prefers Friday deployments.

Promote to client preference?

[YES]
[NO]
```

---

# 33. SEO Strategist

SEO should be an example of a serious specialist assistant, not a generic chat prompt.

Possible inputs:

```text
site crawl
analytics
search console data
page content
keyword research
competitor data
deployment events
```

Tools are optional and installed separately.

---

# 34. SEO workflows

Recommended:

## Technical Audit

```text
Crawl
↓
Detect issues
↓
Classify severity
↓
Prioritise
↓
Generate actions
```

## Page Review

```text
Fetch page
↓
Technical checks
↓
Content checks
↓
Internal-link checks
↓
Recommendations
```

## Content Gap

```text
Current topics
+
competitor topics
+
search opportunity
↓
Gap model
↓
Priority recommendations
```

---

# 35. SEO Push page

Possible layout:

```text
[AUDIT] [PAGE ] [GAPS ] [KEYWORDS]
[COMP ] [LINKS] [TECH ] [REPORT  ]
```

Long press can talk directly to the relevant agent.

---

# 36. Process learning

PushOS should learn from activity using structured statistics before using LLM interpretation.

Example:

```text
Event aggregation
↓
Pattern detector
↓
Candidate pattern
↓
Optional LLM interpretation
↓
Proposal
```

Do NOT:

```text
send entire activity log to LLM every hour
```

This reduces:

- cost
- privacy risk
- latency
- unnecessary token usage

---

# 37. Pattern detector

Implement deterministic analysis for simple patterns.

Examples:

```text
frequent action sequences
high failure rates
repeated user intervention
long waiting states
frequent session switching
repeated commands
```

LLMs should explain or propose improvements after a candidate pattern exists.

---

# 38. Sequence analysis

PushOS may detect frequently repeated action sequences.

Example event sequence:

```text
workspace.select
terminal.tests
agent.review
terminal.tests
git.commit
```

Repeated many times.

Candidate:

```text
"release-check" workflow
```

The system proposes bundling it.

---

# 39. Cost optimiser

Future Process Optimiser capability:

Compare:

```text
task type
model selected
duration
token usage
outcome
retry count
```

Potential suggestion:

```text
This classification workflow has completed
successfully 41 times.

Recommendation:
move first-pass classification to local model.

Use Claude only when confidence < 0.8.
```

All cost recommendations must be based on recorded usage data.

---

# 40. Agent performance

Agents may define KPIs.

Examples:

### SEO agent

```text
issues_detected
issues_resolved
recommendations_accepted
```

### Builder

```text
tasks_completed
test_pass_rate
review_rework
```

### Process Optimiser

```text
proposals_created
proposals_accepted
measured_improvements
```

KPIs should be measurable where possible.

Avoid meaningless LLM self-ratings.

---

# 41. Agent review

Periodic Agent Architect/Process Optimiser review may detect:

```text
unused agents
duplicated responsibilities
agents producing little value
agents requiring excessive intervention
expensive worker policy
```

Then propose:

```text
merge
disable
change provider
change workflow
change trigger
change permissions
```

Again: proposals, not silent mutations.

---

# 42. Agent lifecycle

Recommended:

```text
draft
active
sleeping
disabled
deprecated
archived
```

Agent packs may install agents as:

```text
draft
```

if required external integrations are unavailable.

---

# 43. Human approval boundary

The following require explicit human approval:

```text
creating high-permission agent
changing permissions
production deployment access
financial access
external send access
deleting agent
rewriting important workflows
changing canonical business policy
automatic recurring external actions
```

---

# 44. Suggestions page

PushOS should have a first-class page:

```text
SUGGESTIONS
```

Possible categories:

```text
Process
New Agent
Workflow
Cost
Learning
Security
Cleanup
```

Example:

```text
[3 PROCESS]
[1 AGENT]
[2 LEARN]
[1 COST]
```

---

# 45. Chief of Staff relationship

Chief of Staff may aggregate outputs from:

```text
Process Optimiser
Agent Architect
Risk
KPI
Learning Manager
```

and present only the most important items.

Example:

```text
TODAY

1 workflow bottleneck
1 new-agent opportunity
2 projects blocked
0 critical risks

[BRIEF]
```

This prevents every specialist from interrupting the user independently.

---

# 46. Event subscriptions

Agents may subscribe to typed events.

Example:

```toml
[[triggers]]
event = "deployment.completed"
```

or:

```toml
[[triggers]]
schedule = "daily"
```

or:

```toml
[[triggers]]
manual = true
```

Avoid arbitrary always-running loops.

---

# 47. Pack-trigger permissions

Installing a pack with scheduled or automatic triggers must explicitly display them.

Example:

```text
This pack will run:

Daily at 08:00
Weekly on Monday
After every deployment

Estimated external model use:
Low

[ENABLE]
```

---

# 48. Provider policy

Agents specify capabilities rather than hard-coded models where possible.

Example:

```toml
[worker]
strategy = "auto"

preferred = [
    "local",
    "claude",
    "codex"
]

quality = "high"
max_cost = 0.20
```

PushOS Worker Router selects the appropriate available provider.

---

# 49. Pack portability

An Agent Pack must not assume:

```text
specific user's paths
specific API keys
specific company names
specific provider subscription
```

Use parameters.

Example:

```toml
[parameters]
website_url = ""
analytics_source = ""
project_workspace = ""
```

Studio prompts for missing values during install.

---

# 50. Pack configuration UX

PushOS Studio should show:

```text
SEO Growth Pack

Website:
[________________]

Search Console:
[ Connect ]

Analytics:
[ Connect ]

Preferred AI:
[ Auto ]

Add page:
[x] SEO

Suggested bindings:
[x] Apply
```

Installation should feel like installing an application.

---

# 51. Push bindings from packs

Packs may propose bindings but should not overwrite occupied controls silently.

If conflict exists:

```text
Pad 14 is already assigned.

SEO Pack suggests:
Technical Audit

[REPLACE]
[MOVE]
[SKIP]
```

Studio should allow automatic placement into free pads.

---

# 52. Pack composition

Users may install multiple packs.

Example:

```text
Developer
+
Agency
+
Growth
```

Agents may collaborate across packs through typed events/workflows.

Avoid duplicate agent creation where responsibilities overlap.

Studio should detect likely duplication.

---

# 53. Shared agents

A pack may declare:

```text
requires_role = "researcher"
```

instead of creating another Researcher.

PushOS resolves an existing compatible agent if one exists.

This keeps the system small.

---

# 54. Agent capabilities

Agents should define functional capabilities.

Example:

```toml
capabilities = [
    "seo.audit",
    "seo.page_review"
]
```

Another workflow may depend on:

```text
seo.page_review
```

rather than a specific agent ID.

This allows replacement/customisation.

---

# 55. Capability resolver

Workflow:

```text
needs:
security.code_review
```

Resolver chooses:

```text
Security Reviewer
```

This separates workflows from specific agent names.

---

# 56. Agent collaboration

Agents should not autonomously chat in unrestricted loops.

Preferred communication:

```text
typed event
workflow node
explicit delegation
```

Example:

```text
Builder completed
↓
code.ready_for_review
↓
Security Reviewer wakes
```

Avoid:

```text
Agent A endlessly chats with Agent B
```

---

# 57. Session analysis

Process Optimiser may inspect session metadata such as:

```text
duration
actions
errors
retries
interruptions
provider
workflow
```

Full conversation content should only be used when necessary and permitted.

Structured telemetry should be preferred.

---

# 58. End-of-session review

Optional workflow:

```text
Session ends
↓
Summarise outcomes
↓
Extract learning gaps
↓
Detect repeated manual work
↓
Detect process issue
↓
Generate proposals only if meaningful
```

The user should not receive a review after every trivial session.

---

# 59. Self-improvement safety

PushOS must never create an uncontrolled recursive system where:

```text
Agent creates agent
↓
new agent changes workflow
↓
workflow creates agents
↓
etc.
```

Any structural change must pass through:

```text
Proposal
↓
Validation
↓
Permission analysis
↓
Human approval
↓
Commit
```

---

# 60. Proposal preview

Before approving a new agent or workflow, show:

```text
What changes
Why
Evidence
Required permissions
Triggers
Expected cost
Affected bindings
Rollback path
```

---

# 61. Rollback

Agent/workflow changes must be versioned.

Provide:

```text
pushos config history
pushos config rollback
```

Studio should provide simple rollback.

Every Agent Architect-created change gets a unique change ID.

---

# 62. Audit trail

Persist:

```text
proposal.created
proposal.approved
agent.created
agent.updated
workflow.created
binding.changed
```

with correlation IDs.

Users should be able to understand how the system changed.

---

# 63. Agent Pack registry

Future public ecosystem may support:

```text
pushos pack search seo
pushos pack install publisher/seo-pro
```

Do not build the registry before local pack installation is mature.

Initial packs can live in Git repositories.

---

# 64. Git-based distribution

A pack should be installable from:

```text
local folder
GitHub repository
release archive
future registry
```

Pack code/config should be reviewable before install.

---

# 65. Signing

Future registry may support signed releases.

Do not block MVP on package signing.

Security architecture should leave room for:

```text
publisher identity
checksums
signature verification
```

---

# 66. Version compatibility

Each pack declares:

```text
minimum_pushos_version
```

Optional:

```text
maximum_pushos_version
```

Installation fails clearly on incompatible versions.

---

# 67. Testing Agent Packs

Official packs require automated tests.

Test:

```text
manifest validation
permissions
workflow validity
capabilities
missing dependency behaviour
binding conflicts
```

Agent behaviour should be evaluated using fixtures where practical.

---

# 68. Pack simulator

Official packs should run against FakePush and fake provider backends.

This allows CI without:

```text
Push hardware
Claude account
Codex account
external APIs
```

---

# 69. Official pack quality bar

Do not publish agents whose only logic is:

```text
"You are an expert SEO consultant."
```

Official assistants must provide meaningful structure through:

```text
tools
workflows
triggers
outputs
permissions
KPIs
context rules
```

---

# 70. MVP scope

Agent Packs MVP requires:

1. Pack manifest.
2. Pack validation.
3. Install from local/Git source.
4. Agent definitions.
5. Workflow definitions.
6. Page definitions.
7. Suggested bindings.
8. Permission preview.
9. User overrides.
10. Enable/disable/remove.
11. At least three official packs.

Recommended initial official packs:

```text
Developer
Business
Growth
```

---

# 71. v1 self-improvement scope

v1 should additionally contain:

```text
Tutor
Learning Manager
Process Optimiser
Agent Architect
Suggestions page
structured process telemetry
agent/workflow proposals
approval flow
configuration rollback
```

---

# 72. Suggested implementation order

Do not implement Agent Packs before the PushOS foundations exist.

Order:

```text
Push Hardware
↓
Bindings/Actions
↓
Pages
↓
Providers/Sessions
↓
Workflows
↓
Studio
↓
Agent definition model
↓
Pack format
↓
Official packs
↓
Tutor
↓
Process telemetry
↓
Process Optimiser
↓
Agent Architect
```

---

# 73. Product moat

Agent Packs matter because the value of PushOS should not be:

```text
"Here are 64 programmable buttons."
```

The stronger experience is:

```text
Install Business Pack
↓
Push becomes your business control centre
↓
agents observe structured work
↓
system identifies improvements
↓
user approves improvements
↓
PushOS becomes increasingly tailored
```

This creates value at three levels:

### Hardware interaction

Push gives agents physical presence.

### Operational intelligence

Agent Packs provide useful capabilities immediately.

### Compounding configuration

Process Optimiser and Agent Architect progressively tailor the system to how the user actually works.

---

# 74. Final product principle

PushOS should start useful and become personal.

It should never require users to design an entire AI organisation before receiving value.

Agent Packs provide the starting point.

Process Optimiser discovers inefficiency.

Agent Architect turns opportunity into structure.

Tutor turns work into learning.

Human approval keeps the system understandable and under control.