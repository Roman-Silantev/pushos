# PushOS

Status: Initial implementation specification  
Primary language: Rust  
Target hardware: Ableton Push 2  
Primary host: macOS / Apple Silicon  
License recommendation: Apache-2.0

---

# 1. Vision

PushOS turns Ableton Push 2 into a programmable physical operating system for:

- AI agents
- coding sessions
- terminals
- workflows
- business operations
- macOS functions
- Apple Shortcuts
- media
- scripts
- applications
- arbitrary automation

Push 2 is the primary runtime interface.

The Mac is primarily the execution host.

A lightweight configuration application, **PushOS Studio**, is used to configure the hardware visually.

Normal daily operation should not require PushOS Studio.

Core philosophy:

> PushOS should make interacting with dozens of AI agents and computer workflows feel like operating physical equipment rather than navigating software windows.

---

# 2. Product model

The fundamental abstraction is NOT:

```text
Pad → Agent
```

It is:

```text
Control
   ↓
Gesture
   ↓
Binding
   ↓
Action
```

Example:

```text
PAD_23
+
HOLD
+
Development Page
↓
VoicePromptAction
↓
workspace=sydclaw
role=builder
```

Another:

```text
PLAY_BUTTON
+
PRESS
↓
MediaPlayPauseAction
```

Another:

```text
PAD_8
+
PRESS
↓
WorkflowRunAction
↓
release-production
```

This architecture must remain universal.

Agents are one type of target, not the operating system itself.

---

# 3. Core design principles

## 3.1 Push-first

Normal operation happens from Push.

PushOS Studio is configuration, not the command centre.

## 3.2 Hardware-native

Use all Push 2 capabilities:

- 64 RGB pads
- display
- encoders
- touch-sensitive encoders
- regular buttons
- transport controls
- LEDs

## 3.3 Provider-independent

PushOS core must not depend directly on Claude, Codex, Cursor or any specific model vendor.

## 3.4 Event-driven

No unnecessary polling.

## 3.5 Lightweight

Avoid unnecessary distributed infrastructure.

## 3.6 Local-first

Core runtime and configuration must work without a PushOS cloud service.

## 3.7 Declarative configuration

Bindings, pages, agents and workflows should mostly be configuration rather than Rust code.

## 3.8 Human attention first

Agents should work quietly until:

- approval is required
- input is required
- something failed
- a significant risk exists
- a significant opportunity exists

---

# 4. Primary architecture

```text
                      ABLETON PUSH 2

        Pads / Buttons / Encoders / LEDs / Display
                         │
                         │ USB + MIDI
                         ▼
                ┌─────────────────┐
                │ PushOS Hardware │
                └────────┬────────┘
                         │
                         ▼
                ┌─────────────────┐
                │  Input Router   │
                └────────┬────────┘
                         │
                         ▼
Control + Gesture + Context
                         │
                         ▼
                ┌─────────────────┐
                │ Binding Engine  │
                └────────┬────────┘
                         │
                         ▼
                    Action Bus
                         │
       ┌─────────────────┼──────────────────┐
       │                 │                  │
       ▼                 ▼                  ▼
 Agent Actions      System Actions     Workflow Actions
       │                 │                  │
       ▼                 ▼                  ▼
 ACP / CLI           macOS / Media      Workflow Engine
       │
 ┌─────┼─────┐
 ▼     ▼     ▼
Claude Codex Cursor


Supporting systems:

Runtime Supervisor
Event Bus
State Store
Page Manager
Workspace Manager
Session Manager
Permission Engine
Voice Engine
Renderer
Scheduler
Plugin Registry
```

---

# 5. Technology

Core runtime:

```text
Rust
Tokio
SQLite
```

Push hardware:

```text
midir
rusb
```

Rendering:

```text
tiny-skia
image
```

Serialization/config:

```text
serde
toml
```

Observability:

```text
tracing
```

Application errors:

```text
thiserror
```

CLI:

```text
clap
```

Do not add large frameworks without demonstrated need.

---

# 6. Repository structure

```text
pushos/
│
├── Cargo.toml
├── README.md
├── SPEC.md
├── AGENTS.md
│
├── crates/
│   │
│   ├── pushos-domain/
│   │   ├── actions/
│   │   ├── bindings/
│   │   ├── controls/
│   │   ├── events/
│   │   ├── pages/
│   │   ├── permissions/
│   │   ├── sessions/
│   │   ├── workspaces/
│   │   └── workflows/
│   │
│   ├── pushos-runtime/
│   │   ├── actors/
│   │   ├── supervisor/
│   │   ├── scheduler/
│   │   └── lifecycle/
│   │
│   ├── pushos-push2/
│   │   ├── midi/
│   │   ├── usb/
│   │   ├── controls/
│   │   ├── display/
│   │   └── leds/
│   │
│   ├── pushos-bindings/
│   │   ├── resolver/
│   │   ├── gestures/
│   │   └── context/
│   │
│   ├── pushos-actions/
│   │   ├── agents/
│   │   ├── terminals/
│   │   ├── workflows/
│   │   ├── media/
│   │   ├── shortcuts/
│   │   ├── applications/
│   │   ├── shell/
│   │   ├── pages/
│   │   └── compound/
│   │
│   ├── pushos-agents/
│   │   ├── registry/
│   │   ├── router/
│   │   ├── sessions/
│   │   └── capabilities/
│   │
│   ├── pushos-acp/
│   │   ├── client/
│   │   ├── transport/
│   │   ├── process/
│   │   └── mapping/
│   │
│   ├── pushos-workspaces/
│   │   ├── manifests/
│   │   ├── git/
│   │   ├── worktrees/
│   │   └── apps/
│   │
│   ├── pushos-terminal/
│   │   ├── pty/
│   │   ├── sessions/
│   │   └── output/
│   │
│   ├── pushos-workflows/
│   │   ├── graph/
│   │   ├── executor/
│   │   ├── nodes/
│   │   └── recovery/
│   │
│   ├── pushos-voice/
│   │   ├── capture/
│   │   ├── transcription/
│   │   ├── commands/
│   │   └── routing/
│   │
│   ├── pushos-ui/
│   │   ├── renderer/
│   │   ├── pages/
│   │   ├── widgets/
│   │   ├── animations/
│   │   └── themes/
│   │
│   ├── pushos-storage/
│   │   ├── sqlite/
│   │   ├── migrations/
│   │   └── repositories/
│   │
│   ├── pushos-config/
│   │   ├── loader/
│   │   ├── validation/
│   │   └── watcher/
│   │
│   ├── pushos-plugins/
│   │   ├── registry/
│   │   └── process/
│   │
│   └── pushos-cli/
│
├── studio/
│   └── PushOS Studio
│
├── config/
├── presets/
├── agents/
├── workflows/
├── assets/
└── tests/
```

No cyclic dependencies.

---

# 7. Hexagonal architecture

Domain code owns interfaces.

Infrastructure implements them.

Examples:

```rust
trait ActionExecutor {
    async fn execute(
        &self,
        context: ActionContext
    ) -> Result<ActionResult, ActionError>;
}
```

```rust
trait AgentBackend {
    async fn start(...);
    async fn send(...);
    async fn cancel(...);
}
```

```rust
trait MediaController {
    async fn play_pause(...);
    async fn next(...);
    async fn previous(...);
}
```

```rust
trait SpeechToText {
    async fn transcribe(...);
}
```

Domain must never import:

- USB implementations
- SQLite
- Claude internals
- Codex internals
- Cursor internals
- Apple APIs

---

# 8. Controls

All Push controls receive stable IDs.

Examples:

```text
pad.0 ... pad.63

button.play
button.record
button.new
button.duplicate
button.delete
button.undo
button.session
button.user

encoder.0 ... encoder.7
```

Never expose raw MIDI notes throughout the application.

Raw hardware mapping belongs exclusively inside `pushos-push2`.

---

# 9. Gestures

Supported button/pad gestures:

```text
press
release
tap
double_tap
hold
shift_press
shift_hold
```

Encoder gestures:

```text
turn
turn_left
turn_right
touch
release
press
shift_turn
```

Gesture recognition must be centralized.

No individual feature may implement its own double-tap/hold timing.

---

# 10. Bindings

Binding model:

```rust
struct Binding {
    id: BindingId,
    control: ControlId,
    gesture: Gesture,
    scope: BindingScope,
    action: ActionDefinition,
    priority: u16,
}
```

Scopes:

```text
global
page
workspace
page+workspace
```

Resolution priority:

```text
workspace+page
workspace
page
global
```

More specific binding wins.

Conflicting bindings must be rejected by configuration validation.

---

# 11. Actions

Actions are the central execution abstraction.

Initial action categories:

```text
AgentAction
SessionAction
TerminalAction
WorkspaceAction
WorkflowAction
MediaAction
ShortcutAction
ApplicationAction
ShellAction
PageAction
SystemAction
VoiceAction
CompoundAction
HTTPAction
```

Push hardware must never need custom logic for each category.

---

# 12. Action result

Every action returns normalized state.

```rust
struct ActionResult {
    status: ActionStatus,
    message: Option<String>,
    display: Option<DisplayIntent>,
}
```

Possible status:

```text
started
completed
waiting
failed
cancelled
```

---

# 13. Compound actions

A binding may execute multiple actions.

Example:

```text
Start Work
↓
Set Focus Mode
↓
Open Cursor
↓
Select Sydclaw Workspace
↓
Start Claude Builder
↓
Play focus playlist
```

Compound execution modes:

```text
sequential
parallel
```

Sequential execution may stop on failure.

---

# 14. Pages

Pages change the physical meaning of controls.

Suggested presets:

```text
Home
Development
Clients
Agents
Business
Sales
Music
Learning
Custom
```

Pages are configuration.

Users may create unlimited pages.

---

# 15. Page persistence

Each workspace remembers:

```text
last_page
selected_agent
selected_session
selected_view
```

Returning to the workspace restores previous context.

---

# 16. Dynamic pages

Actions/events may suggest temporary pages.

Example:

```text
DEPLOYMENT FAILED

[OPEN INCIDENT]
[IGNORE]
```

These should not normally steal focus automatically.

---

# 17. Push screen

PushOS controls the full Push 2 display.

UI should be purpose-built for 960×160.

Do not attempt to reproduce desktop applications.

Display responsibilities:

- current page
- current workspace
- selected control
- selected agent
- current action
- agent status
- workflow progress
- terminal summary
- voice transcription
- alerts
- approvals
- temporary media overlay

---

# 18. Rendering architecture

```text
immutable UI snapshot
↓
Page
↓
Widgets
↓
Framebuffer
↓
USB display
```

Rules:

- preallocate framebuffer
- no network work on renderer
- no database work on renderer
- render static screens only when dirty
- animations target maximum around 30 FPS
- coalesce status updates

---

# 19. Push LEDs

LED state reflects meaning and status.

Suggested defaults:

```text
dim      available/idle
blue     working
yellow   waiting
green    complete
red      failed
purple   workflow/communication
white    selected
```

Users may configure themes.

Colour cannot be the sole indicator.

---

# 20. PushOS Studio

PushOS Studio is a lightweight configuration UI.

It is NOT required for normal operation.

Main functions:

- virtual Push 2
- select any physical control
- assign actions
- configure gestures
- configure pages
- configure workspaces
- configure agents
- configure workflows
- configure colours/icons
- view connected sessions
- drag-and-drop bindings
- test a binding
- import/export presets

---

# 21. Studio layout

Recommended:

```text
┌─────────────────────────────────────────────┐
│ PushOS Studio                    Push 2 ●    │
├──────────────┬──────────────────────────────┤
│ Pages        │                              │
│              │      Virtual Push 2          │
│ Home         │                              │
│ Development  │      64 pads + buttons       │
│ Clients      │                              │
│ Music        │                              │
├──────────────┼──────────────────────────────┤
│ Sources      │ Selected Control             │
│ Agents       │                              │
│ Sessions     │ Tap: ...                     │
│ Workflows    │ Hold: ...                    │
│ Shortcuts    │ Double tap: ...              │
│ Media        │                              │
│ Apps         │ LED: ...                     │
└──────────────┴──────────────────────────────┘
```

---

# 22. Studio technology

Studio should remain separate from core.

Recommended implementation:

```text
Tauri
Rust backend
small TypeScript UI
```

The Studio process communicates with PushOS through a local API/socket.

Do not expose the core runtime by embedding it inside the Studio application.

Closing Studio must not stop PushOS.

---

# 23. Source of truth

Studio does not create a proprietary database format.

Human-readable configuration remains the source of truth.

Example:

```toml
[[bindings]]
control = "button.play"
gesture = "press"
scope = "global"
action = "media.play_pause"

[[bindings]]
control = "pad.23"
gesture = "tap"
page = "development"
action = "workspace.select"
target = "sydclaw"

[[bindings]]
control = "pad.23"
gesture = "hold"
page = "development"
action = "voice.prompt"
target = "workspace:sydclaw/role:builder"
```

Studio edits this configuration safely.

---

# 24. Hot reload

Config updates:

```text
watch
↓
parse
↓
validate
↓
construct candidate
↓
atomic swap
```

Invalid configuration never replaces active configuration.

---

# 25. Presets

Public repository should provide presets.

Initial examples:

```text
Developer
AI Engineer
Agency Owner
CEO
Researcher
Content Creator
Music + AI
Automation
Blank
```

Presets are normal configuration.

---

# 26. Agent integration

Use Agent Client Protocol where practical.

Core model:

```text
PushOS
↓
AgentBackend
↓
ACP Adapter
↓
Claude / Codex / Cursor / other ACP agents
```

Provider-specific behaviour stays inside adapters.

---

# 27. Agent capabilities

Capability negotiation should support concepts including:

```text
resume
cancel
permissions
models
reasoning_effort
worktrees
subagents
usage
background_tasks
terminal
```

Unsupported capabilities should be hidden or disabled.

---

# 28. Normalized agent state

```text
sleeping
queued
starting
working
waiting_input
waiting_approval
paused
completed
failed
cancelled
offline
```

Providers map their internal events to this model.

---

# 29. Agent identity

An agent definition describes a role.

Example:

```toml
id = "builder"
name = "Builder"

objective = """
Implement approved development work safely and concisely.
"""

[worker]
preferred = ["claude", "codex"]

[permissions]
shell = true
git_write = true
deploy_production = false
```

Agents are configuration.

---

# 30. Logical agent binding

Default binding should target logical roles rather than fragile session IDs.

Example:

```text
workspace=sydclaw
role=builder
```

PushOS resolves the currently appropriate session.

---

# 31. Exact session binding

Users may also explicitly pin:

```text
session=abc123
```

Useful for temporary/manual layouts.

---

# 32. Active sessions

Sessions may represent:

- Claude Code
- Codex
- Cursor Agent
- terminal
- background process

Session Manager tracks:

```text
provider
workspace
role
status
pid
started_at
last_activity
provider_session_id
```

---

# 33. Terminal sessions

Use managed PTYs.

Do not create dozens of visible Terminal windows.

Each terminal session has:

```text
id
name
cwd
command
pid
workspace
status
output_tail
```

Bindings can target terminals directly.

Examples:

```text
pad.12 tap
→ select terminal backend

pad.12 hold
→ voice instruction to Claude working alongside backend

pad.12 double tap
→ open terminal window attached to session
```

---

# 34. Workspace

Workspace represents persistent project context.

Example:

```toml
id = "sydclaw"
name = "Sydclaw"
repo = "~/Projects/sydclaw"

[apps]
cursor = "~/Projects/sydclaw"

[[agents]]
role = "builder"
provider = "claude"

[[agents]]
role = "reviewer"
provider = "codex"
```

Workspace may include:

- repository
- worktrees
- agent sessions
- terminal sessions
- workflows
- application targets
- environment
- notes/memory source

---

# 35. Git worktrees

Parallel coding agents should generally work in isolated worktrees.

Workspace Manager owns worktree lifecycle.

No two autonomous coding agents should accidentally edit the same working tree concurrently unless explicitly configured.

---

# 36. Cursor

Separate:

```text
Cursor Agent
```

from:

```text
Cursor Desktop
```

Cursor Agent is an execution backend.

Cursor Desktop is an external human inspection/editing surface.

Push actions may:

- open project
- focus project
- open worktree
- open file

---

# 37. Workflows

Workflow engine supports:

```text
Agent
Tool
Condition
Parallel
Join
Wait
Delay
Approval
Emit
Subgraph
End
```

Example:

```text
Claude Plan
↓
Codex Build
↓
Tests
↓
Pass?
├─ No → Codex Fix ─┐
└─ Yes → Review    │
                   │
             Claude Review
```

---

# 38. Workflow durability

Persist transitions before executing subsequent nodes.

Workflow must survive:

- PushOS restart
- provider crash
- Mac restart where possible
- temporary network failure

Loops require:

```text
max_iterations
timeout
budget
stop_condition
```

---

# 39. Workflow bindings

A pad may represent a workflow.

Examples:

```text
BUILD
SHIP
SECURITY REVIEW
MORNING CEO
CLIENT REPORT
BACKUP
```

LED displays workflow state.

---

# 40. Media

Media is a first-class action provider.

Initial macOS functions:

```text
play_pause
next_track
previous_track
volume_up
volume_down
set_volume
```

Apple Music support should be implemented through macOS-supported mechanisms.

Do not couple media control to Push UI code.

---

# 41. Media overlay

Media actions may temporarily display:

```text
APPLE MUSIC

Artist
Track

03:12 ━━━━━━━━━━━━━───── 04:50
```

Then restore previous page automatically.

---

# 42. Recommended global controls

Default preset may include:

```text
PLAY
media play/pause

RECORD
push-to-talk

UNDO
undo last reversible PushOS action

NEW
new agent/action

SESSION
workspace/session navigation

USER
PushOS/Ableton mode or configured function
```

These remain configurable.

---

# 43. Apple Shortcuts

Treat Apple Shortcuts as another ActionProvider.

Actions:

```text
list
run
cancel
```

Examples:

```text
Office Lights
Focus Mode
Start Meeting
Lock Office
HomeKit Scene
End Workday
```

Use safe process argument passing.

Do not construct shell strings from user input.

---

# 44. Application actions

Support:

```text
launch
focus
open_path
open_url
```

Examples:

```text
Cursor
Terminal
Safari
Obsidian
Slack
Spotify
Apple Music
```

---

# 45. Shell actions

Shell actions exist but require stronger permissions.

Prefer direct executable + args representation.

Example:

```toml
program = "cargo"
args = ["test"]
cwd = "~/Projects/project"
```

Avoid:

```text
"cargo test && something"
```

unless explicitly using a trusted script.

---

# 46. HTTP actions

Optional general automation mechanism.

Support:

```text
GET
POST
PUT
DELETE
```

Credentials use Keychain references.

Never store secrets in configuration.

---

# 47. Voice

Push voice control is first-class.

Default interaction:

```text
hold configured control
↓
record
↓
release
↓
transcribe locally
↓
route
↓
execute
```

Context may automatically include:

```text
current page
workspace
selected agent
selected session
selected workflow
```

---

# 48. Voice command layers

Layer 1:

deterministic commands.

Examples:

```text
stop
cancel
home
approve
reject
next page
pause music
```

Layer 2:

semantic requests.

Examples:

```text
Tell Claude to continue authentication.
```

```text
Ask Codex to review what Claude changed.
```

```text
Run tests and tell me only if they fail.
```

---

# 49. Destructive voice actions

Require explicit confirmation for actions such as:

```text
production deploy
merge
force push
delete
external send
financial write
```

Voice transcription alone is insufficient authorization.

---

# 50. Memory

PushOS must NOT depend on Obsidian.

Provide a small generic Memory API.

Initial implementation:

```text
SQLite metadata
+
optional Markdown directories
+
FTS5
```

Memory is useful for:

- business knowledge
- project decisions
- agent context
- learning context

but it must remain modular.

---

# 51. Memory providers

Future providers may include:

```text
Markdown
Obsidian
Notion
Confluence
Google Drive
SharePoint
custom database
```

These are plugins.

Obsidian is optional.

---

# 52. Runtime persistence

SQLite stores operational state.

Recommended tables:

```text
bindings
pages
workspaces
agents
sessions
terminal_sessions
workflows
workflow_runs
jobs
events
approvals
settings
plugins
```

Use:

```text
WAL
foreign keys
transactions
busy timeout
```

---

# 53. Event bus

Typed events only.

Examples:

```text
control.input
binding.resolved
action.started
action.completed
action.failed

agent.started
agent.waiting
agent.completed

session.started
session.closed

workflow.started
workflow.completed

approval.requested
approval.resolved

push.connected
push.disconnected

config.updated

media.changed
```

---

# 54. Event envelope

```rust
struct EventEnvelope {
    id: EventId,
    correlation_id: CorrelationId,
    causation_id: Option<EventId>,
    timestamp: DateTime<Utc>,
    source: SourceId,
    payload: DomainEvent,
}
```

Use correlation IDs across multi-step actions.

---

# 55. Concurrency

No giant globally shared application mutex.

Prefer actor-style ownership.

Examples:

```text
PushActor
BindingActor
WorkspaceActor
AgentActor
WorkflowActor
StorageWriter
```

Mutating commands are processed sequentially per aggregate.

Expose immutable snapshots to readers.

---

# 56. Async requirements

Every long-running operation:

- has explicit ownership
- supports cancellation where meaningful
- has timeout where external IO is involved
- reports failure
- is supervised

Never fire-and-forget important operations.

---

# 57. Backpressure

Use bounded channels.

Coalesce:

- repeated progress updates
- animation ticks
- terminal output UI updates

Never drop:

- approvals
- failures
- state transitions
- workflow commits
- security-relevant events

---

# 58. Idempotency

External side-effect actions receive an execution ID.

Examples:

```text
deploy
send
payment
Shortcut
HTTP write
```

Retries may not duplicate non-idempotent operations.

---

# 59. Permissions

Permission categories:

```text
filesystem.read
filesystem.write

shell.execute

network

git.commit
git.push
git.merge

deploy.preview
deploy.production

application.launch

media.control

shortcuts.execute

external.send

financial.read
financial.write
```

Least privilege by default.

---

# 60. Secrets

Use:

1. provider login where available
2. macOS Keychain
3. environment variables

Never persist plaintext secrets in TOML.

---

# 61. Plugin system

Extension classes:

```text
ActionProvider
AgentProvider
MemoryProvider
VoiceProvider
PageProvider
```

Initial plugins may be subprocess-based.

A broken plugin must not crash PushOS core.

---

# 62. Push reconnect

PushOS must survive:

```text
USB unplug
USB reconnect
sleep
wake
```

Push runtime stays alive when hardware disappears.

When hardware returns:

```text
reconnect
↓
restore LEDs
↓
restore page
↓
restore display
```

---

# 63. Provider recovery

PushOS must tolerate:

```text
Claude crash
Codex crash
Cursor crash
terminal crash
voice provider crash
```

Provider processes are supervised.

Retry must be bounded.

---

# 64. Performance targets

Core idle target:

```text
near-zero CPU
<150 MB RAM
```

excluding external agents and Studio.

Interaction targets:

```text
pad LED feedback        <50 ms
page change             <100 ms
local action dispatch   <50 ms
status render           <100 ms after event
```

These are engineering targets.

---

# 65. Source file rules

Soft target:

```text
<250 LOC
```

Review threshold:

```text
>400 non-test LOC
```

Do not split cohesive code artificially simply to meet LOC targets.

One module = one clear responsibility.

---

# 66. Naming

Avoid generic modules:

```text
utils
helpers
common
manager
misc
```

Prefer:

```text
BindingResolver
GestureRecognizer
SessionRegistry
WorkflowExecutor
PushRenderer
```

---

# 67. Errors

Typed errors.

Never use Strings as the primary error model.

Classify external failures:

```text
retryable
validation
permission
user_action_required
component_failure
```

---

# 68. Testing

Core tests must not need Push hardware.

Provide:

```text
FakePush
FakeAgentBackend
FakeActionProvider
FakeSpeechProvider
FakeMediaProvider
FakeStorage
```

---

# 69. Concurrency testing

Required scenarios:

```text
double press
press while page changes
hold then disconnect
cancel while agent completes
provider reconnect
duplicate event
workflow restart
config reload during action
Push reconnect during render
```

Every discovered race-condition bug receives a regression test.

---

# 70. FakePush

FakePush models:

```text
64 pads
buttons
encoders
display
LED state
```

It is a testing/development tool.

Do not turn FakePush into the production interface.

---

# 71. Setup

Target eventually:

```bash
brew install pushos
pushos init
pushos run
```

`pushos init` detects:

```text
Push 2
Claude
Codex
Cursor
Apple Shortcuts
microphone/STT
```

---

# 72. Diagnostics

Provide:

```bash
pushos doctor
```

Check:

```text
Push USB
Push MIDI
display
SQLite
config
agent adapters
microphone permissions
Shortcuts
plugin health
```

---

# 73. Phase 0: Push hardware

Build only:

```text
connect
disconnect/reconnect
pads
buttons
encoders
LEDs
display
animations
FakePush
```

Acceptance:

- every physical input produces normalized `ControlEvent`
- every LED is individually addressable
- arbitrary frames render
- reconnect restores state
- fake hardware passes same interface tests

Do not implement agents yet.

---

# 74. Phase 1: Action and Binding kernel

Implement:

```text
ControlId
Gesture
Binding
BindingScope
ActionDefinition
BindingResolver
ActionDispatcher
Page
Context
```

Acceptance:

```text
pad → action
button → action
encoder → action
global/page/workspace precedence
```

This is the architectural foundation of PushOS.

---

# 75. Phase 2: Push UI

Implement:

```text
pages
widgets
selected control
status bar
notifications
temporary overlays
animations
```

Acceptance:

multiple configured pages can be navigated entirely from Push.

---

# 76. Phase 3: simple action providers

Implement:

```text
PageAction
ApplicationAction
MediaAction
ShortcutAction
ShellAction
```

Acceptance example:

```text
Play button → Apple Music play/pause
Pad → open Cursor
Pad → run Shortcut
Pad → run cargo test
```

---

# 77. Phase 4: PushOS Studio

Implement the minimal configuration application.

Required:

- connect to running PushOS
- display virtual Push
- select control
- assign action
- assign gesture
- select scope/page
- save config
- trigger hot reload

Do not build advanced visual polish yet.

---

# 78. Phase 5: ACP agents

Integrate:

```text
Claude
Codex
Cursor
```

Required:

```text
start
resume
prompt
cancel
status
approval
```

Acceptance:

three separate Push controls can operate three concurrent agent sessions.

---

# 79. Phase 6: sessions and terminals

Implement:

```text
SessionRegistry
PTY
logical role bindings
exact session bindings
```

Acceptance:

active Claude or terminal session can be assigned to any Push control through Studio.

---

# 80. Phase 7: workspaces

Implement:

```text
manifest
repo
worktrees
sessions
applications
context
```

Acceptance:

one Push pad can represent an entire coding project.

---

# 81. Phase 8: workflows

Implement durable workflow runtime.

Acceptance workflow:

```text
Claude plan
↓
Codex implementation
↓
tests
↓
Claude review
```

Must survive runtime restart.

---

# 82. Phase 9: voice

Implement:

```text
push-to-talk
local STT
context routing
deterministic voice commands
agent prompting
confirmation
```

Acceptance:

user can manage a coding workflow without keyboard input.

---

# 83. Phase 10: flexible memory

Implement lightweight optional memory.

Do not make this block the main product.

---

# 84. Phase 11: public presets

Release useful templates.

Example Developer preset:

```text
Home
Development
Agents
Projects
Music
Automation
```

Include intuitive default bindings.

---

# 85. Phase 12: ecosystem

Later:

```text
Apple Watch
plugin marketplace
shared presets
community agent packs
community workflows
```

These must not delay core Push experience.

---

# 86. Definition of MVP

MVP means:

1. Push 2 fully controlled.
2. Every relevant physical input configurable.
3. Pages work.
4. Gestures work.
5. Bindings work.
6. PushOS Studio can edit bindings.
7. Apple Music play/pause works from Push.
8. Apple Shortcut can run.
9. shell action can run.
10. Claude works.
11. Codex works.
12. Cursor Agent works.
13. active agent/session can be assigned to a pad.
14. terminal session can be assigned to a pad.
15. hold-to-talk can prompt selected agent.
16. Push shows live agent state.
17. runtime survives Push disconnect.
18. no normal desktop UI required during operation.

---

# 87. Definition of v1

v1 additionally requires:

```text
workspaces
worktrees
durable workflows
permissions
compound actions
presets
plugins
voice
hot reload
diagnostics
installer
documentation
```

---

# 88. What we should NOT build

Do not initially build:

```text
cloud control plane
SaaS
user accounts
enterprise RBAC server
Kafka
Redis
Postgres
Kubernetes
Electron runtime
proprietary agent protocol
custom coding harness
large vector database
mandatory Obsidian integration
```

---

# 89. Main architectural test

A contributor should be able to add:

a new agent provider

without modifying:

```text
Push driver
renderer
binding resolver
```

A contributor should be able to add:

a new action provider

without modifying:

```text
agent runtime
workflow engine
```

A user should be able to create:

a new page

without writing Rust.

A user should be able to bind:

a new workflow

without writing Rust.

If these conditions fail, reconsider the architecture.

---

# 90. Core product identity

PushOS is:

> An open-source programmable operating layer that turns Ableton Push 2 into a physical command centre for AI agents, coding sessions, workflows and computer automation.

The Push experience is the product.

Agents make it exceptionally powerful.

Actions + Bindings + Context + Pages make it universal.