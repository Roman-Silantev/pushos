# PushOS AI Builder Instructions

Read `SPEC.md` before implementing architectural changes.

PushOS is a Rust-based operating layer for Ableton Push 2.

Push 2 is the primary runtime interface.

PushOS Studio is configuration only.

---

# 1. Build only the current milestone

Do not attempt to implement the entire specification at once.

Follow the ordered phases in `SPEC.md`.

Complete acceptance tests for the current phase before starting the next phase.

If asked to implement Phase 0:

do not begin agent integration.

If asked to implement Phase 1:

do not begin workflows.

Architectural discipline is more important than apparent progress.

---

# 2. Core abstraction

The system is built around:

```text
Control
↓
Gesture
↓
Binding
↓
Action
```

Do not hard-code:

```text
pad → agent
```

Agents are only one action/provider category.

---

# 3. Architecture

Use:

```text
Rust
Tokio
SQLite
ports/adapters
message-driven concurrency
```

Domain code owns interfaces.

Infrastructure implements them.

Dependency direction points inward.

---

# 4. SOLID

Every module has one reason to change.

Avoid god objects and god files.

Do not create one:

```text
PushOSManager
```

that owns unrelated behaviour.

Instead use focused components such as:

```text
BindingResolver
GestureRecognizer
ActionDispatcher
WorkspaceRegistry
SessionRegistry
PushRenderer
WorkflowExecutor
```

---

# 5. File size

Soft target:

```text
<250 LOC
```

Review carefully when:

```text
>400 non-test LOC
```

Do not split files artificially.

Split by responsibility.

---

# 6. Functions

Prefer small functions.

Soft target:

```text
<40 lines
```

Longer functions require clear justification.

Prefer:

- explicit inputs
- explicit outputs
- immutable data
- pure logic where possible

---

# 7. No vague utility modules

Avoid:

```text
utils.rs
helpers.rs
misc.rs
common.rs
```

unless they represent a genuinely cohesive abstraction.

Name modules after their responsibility.

---

# 8. Domain types

Do not pass primitive Strings everywhere.

Prefer:

```rust
ControlId
BindingId
AgentId
SessionId
WorkspaceId
WorkflowId
```

Prefer enums over string constants.

Example:

```rust
enum AgentState {
    Sleeping,
    Working,
    WaitingInput,
    Failed,
}
```

---

# 9. Hardware isolation

Raw Push MIDI notes/CC values must remain inside the Push 2 adapter.

The rest of the system receives:

```rust
ControlEvent
```

with stable semantic IDs.

No agent/action/page code may depend on MIDI numbers.

---

# 10. Gesture isolation

Gesture timing belongs in one component.

Features must not implement their own:

```text
hold timers
double-tap timers
shift detection
```

Use `GestureRecognizer`.

---

# 11. Binding resolver

Binding precedence must remain deterministic:

```text
workspace+page
workspace
page
global
```

Do not add hidden precedence rules.

Reject ambiguous configuration.

---

# 12. Actions

All executable user bindings become actions.

Do not create Push-specific execution logic such as:

```rust
if pad == 5 {
   run_claude();
}
```

Correct model:

```text
ControlEvent
↓
BindingResolver
↓
ActionDefinition
↓
ActionDispatcher
```

---

# 13. Action providers

Keep providers isolated.

Examples:

```text
AgentActionProvider
MediaActionProvider
ShortcutActionProvider
ApplicationActionProvider
ShellActionProvider
WorkflowActionProvider
```

Adding a provider must not require changing Push hardware code.

---

# 14. Agents

Prefer ACP where supported.

Do not automate Claude/Codex/Cursor graphical interfaces when structured agent interfaces exist.

Provider-specific events are normalized before entering domain logic.

---

# 15. Agent state

Core uses only normalized state:

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

Provider-specific metadata can remain available separately.

---

# 16. Logical sessions

Default bindings should support logical targets.

Example:

```text
workspace=sydclaw
role=builder
```

Do not assume a provider session ID lives forever.

Exact session bindings are supported but should not be the only method.

---

# 17. Terminals

Use managed PTYs.

Do not open visible Terminal windows as the normal execution model.

Terminal.app or another terminal emulator may be opened only when the human explicitly requests inspection.

---

# 18. Worktrees

Parallel autonomous coding agents should normally use separate Git worktrees.

Do not allow two agents to mutate the same working tree accidentally.

Workspace Manager owns worktree allocation.

---

# 19. Concurrency

Race conditions are unacceptable.

Do not use a giant:

```rust
Arc<Mutex<AppState>>
```

as the primary architecture.

Prefer state ownership.

Examples:

```text
PushActor
WorkspaceActor
AgentActor
WorkflowActor
StorageWriter
```

Commands are processed sequentially per mutable aggregate.

---

# 20. Shared state

Expose immutable snapshots through appropriate channels.

Do not perform business logic while holding locks.

Keep lock scope extremely small when a lock is genuinely required.

---

# 21. Tokio tasks

Every spawned task must have an owner.

Do not create important detached tasks.

Long-running tasks must support:

```text
cancellation
timeout
failure reporting
shutdown
```

---

# 22. Channels

Use bounded channels.

Do not create unbounded event queues for high-frequency data.

Coalesce low-value updates where appropriate.

Never lose important transitions.

---

# 23. Rendering

Rendering must never wait on:

```text
LLM
network
SQLite
filesystem
speech transcription
subprocess
```

Renderer consumes snapshots.

Rendering remains deterministic and fast.

---

# 24. Performance

Avoid unnecessary allocation in:

```text
input path
LED updates
render loop
event routing
```

Do not optimize blindly.

Measure before adding complexity.

---

# 25. SQLite

SQLite is local runtime truth.

Use:

```text
WAL
foreign keys
transactions
busy timeout
```

Avoid multiple uncontrolled writers.

Prefer a clear write ownership strategy.

---

# 26. Workflows

Persist successful transitions before beginning subsequent nodes.

Workflow restart must not repeat completed side effects.

Loops require explicit termination controls.

Never create uncontrolled autonomous LLM loops.

---

# 27. Idempotency

External side effects require execution IDs where duplication would be harmful.

Examples:

```text
deployment
external send
HTTP write
Shortcut
financial action
```

Never blindly retry non-idempotent operations.

---

# 28. Errors

Use typed errors.

Do not:

```text
swallow errors
log-and-ignore critical failures
unwrap external IO
convert all errors into strings
```

Error boundaries classify:

```text
retryable
validation
permission
user-action-required
component failure
```

---

# 29. Permissions

Least privilege by default.

Never silently enable:

```text
production deployment
git force push
external messaging
financial write
arbitrary filesystem write
arbitrary shell execution
```

Provider-native permissions and PushOS permissions both apply.

Stricter policy wins.

---

# 30. Shell safety

Never concatenate untrusted strings into shell commands.

Prefer:

```rust
Command::new(program)
    .args(args)
```

rather than:

```text
sh -c "..."
```

Use shell execution only when explicitly required.

---

# 31. Secrets

Never persist secrets in configuration.

Use:

```text
provider authentication
macOS Keychain
environment reference
```

Never log secrets.

---

# 32. Configuration

Bindings, agents, pages and workflows should be declarative where practical.

Config updates:

```text
parse
↓
validate
↓
build candidate
↓
atomic replace
```

Invalid configuration leaves current state untouched.

---

# 33. PushOS Studio

Studio is not the source of truth.

Studio edits PushOS configuration.

Do not move configuration into an opaque Studio-only database.

Closing Studio must never stop PushOS runtime.

---

# 34. Plugins

Plugin failure must not crash core.

Keep plugin boundaries explicit.

A plugin cannot receive broad permissions automatically.

---

# 35. Media

Media control is an ActionProvider.

Do not put Apple Music-specific code in:

```text
renderer
binding resolver
Push driver
```

---

# 36. Apple Shortcuts

Shortcuts are another ActionProvider.

Use safe process invocation.

Respect cancellation/timeouts.

---

# 37. Voice

Use deterministic parsing for deterministic commands.

Do not invoke an LLM simply to understand:

```text
stop
cancel
home
approve
reject
pause
```

Semantic routing can use an agent/model.

---

# 38. Dangerous voice actions

Do not execute destructive actions solely from an uncertain speech transcript.

Require explicit confirmation.

---

# 39. Memory

Do not make Obsidian mandatory.

Memory interfaces must remain provider-independent.

Do not introduce vector databases until a demonstrated use case requires one.

---

# 40. Logging

Use structured `tracing`.

Include relevant:

```text
correlation_id
action_id
agent_id
session_id
workspace_id
workflow_id
```

Avoid noisy logging in high-frequency render/input loops.

---

# 41. Testing

Core tests must not require hardware.

Provide fakes for:

```text
Push
AgentBackend
ActionProvider
MediaController
SpeechToText
Storage
```

---

# 42. Regression tests

Every bug involving:

```text
race condition
event duplication
state corruption
provider reconnect
Push reconnect
workflow recovery
config hot reload
```

must receive a regression test.

---

# 43. Concurrency scenarios

Explicitly test:

```text
double tap racing page switch
hold interrupted by disconnect
provider completion during cancel
config reload during action
duplicate external event
workflow restart after persisted transition
USB reconnect during animation
```

---

# 44. Hardware milestone discipline

Phase 0 must prove the Push hardware abstraction before integrating agents.

Required:

```text
all controls
all LEDs
display
animations
disconnect
reconnect
FakePush
```

Do not build around assumptions that have not been tested on the real Push 2.

---

# 45. Avoid overengineering

Do not introduce:

```text
Redis
Kafka
Postgres
Kubernetes
Temporal
LangGraph
Electron runtime
distributed services
```

without a measured requirement.

Simple local architecture is intentional.

---

# 46. Avoid rebuilding mature tools

PushOS should orchestrate:

```text
Claude
Codex
Cursor
Git
macOS
Apple Shortcuts
```

not replace them.

---

# 47. Before finishing a task

Run:

```text
cargo fmt
cargo clippy
cargo test
```

plus relevant integration tests.

Then inspect specifically for:

```text
race conditions
cancellation bugs
duplicate actions
unbounded queues
hidden blocking IO
permission escalation
oversized modules
dependency violations
```

---

# 48. Completion report

At the end of each implementation task report:

```text
Implemented
Tests added
Architecture decisions
Known limitations
Next milestone
```

Do not claim completion if acceptance criteria have not passed.

---

# 49. Primary engineering objective

The codebase must remain understandable by a competent individual engineer.

A new contributor should be able to identify:

```text
where hardware lives
where bindings live
where actions live
where agent adapters live
where workflow execution lives
```

within minutes.

---

# 50. Final rule

When adding functionality, ask:

> Is this a new Control, Gesture, Binding, Action, Context, Provider, Workspace or Workflow?

Place it in the correct abstraction.

Do not bypass the architecture because implementing a direct special case is faster.

PushOS must remain small, fast, predictable and extensible.