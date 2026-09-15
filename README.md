# PushOS

An open-source programmable operating layer that turns an Ableton Push 2 into a
physical command centre for AI agents, coding sessions, workflows and computer
automation.

Push 2 is the interface. The Mac is the execution host. Normal daily operation
should not require a desktop window.

> Status: early. The hardware layer, the binding kernel, the display and the
> first action providers are built and tested. Agents, workspaces, workflows and
> voice are the next milestones. See [Where this is up to](#where-this-is-up-to).

## The idea

The abstraction is not `pad → agent`. It is:

```
Control  →  Gesture  →  Binding  →  Action
```

A pad produces a control identity. The recogniser turns input into a gesture.
The resolver picks the one binding that applies to the current page and
workspace. The dispatcher runs an action. Agents are one kind of action among
media, applications, Shortcuts, shell commands and workflows.

That indirection is the whole product. It is what lets a pad mean "open Cursor"
on one page and "run the release workflow" on another, without a line of Rust.

![The Push 2 display: a workspace and page in the status bar, eight labelled
columns matching the buttons above and below the screen, and a message
line](docs/images/display.png)

The eight columns line up with the eight buttons directly above the display and
the eight directly below it, so each column says what its two buttons do. Render
one yourself without any hardware:

```bash
cargo run -p pushos-ui --example preview -- frame.ppm overlay
```

While PushOS is waiting for the surface, and for a moment when it arrives, the
panel shows an animated mascot in Claude's orange. It is drawn as geometry
rather than as text, because the block characters it is written in are just
rectangles, so it stays crisp at any size.

![The Push 2 display showing an animated orange block-character mascot above the
word PushOS](docs/images/splash.png)

It is the only thing in PushOS that redraws on a timer, it stops on its own, and
a test holds it to that.

## What works today

```bash
pushos presets   # list the configurations PushOS ships with
pushos init      # write one of them
pushos pack      # install and manage packs of agents, workflows and pages
pushos check     # validate it without running anything
pushos doctor    # report on hardware, configuration and host integrations
pushos run       # run it
pushos app       # install it as a Mac app that starts at login
pushos status    # ask a running PushOS what it is doing
pushos bindings  # list what it has bound
pushos sessions  # list the agents and terminals it is driving
pushos workspaces # list the projects it knows about
```

- **All 141 Push 2 controls**, addressed by name. `pad.0`, `button.play`,
  `encoder.master`. The numbering table is generated from Ableton's published
  hardware map, and no MIDI number appears anywhere else in the system.
- **Gestures**: press, release, tap, double tap, hold, and their Shift variants,
  plus encoder turns and touch. One component owns all the timing.
- **Pages**, so the same 64 pads mean different things in different contexts.
- **Bindings** with deterministic precedence: `workspace+page`, `workspace`,
  `page`, `global`.
- **Actions**: page navigation, applications, Apple Shortcuts, media control,
  shell commands, terminals, projects and workflows.
- **The display**, drawn natively for 960x160.
- **Hot reload**, validated. A bad edit is reported and ignored; the running
  surface is untouched.
- **A simulated surface**, so you can work on pages and bindings with no
  hardware attached: `pushos run --fake`. It says so everywhere it appears; a
  stand-in is never presented as a Push 2.
- **PushOS Studio**, a Mac app for configuring a running PushOS, which it talks
  to over a local socket. Install it with `npm run app` in
  [`studio/`](studio/).
- **Agents**, over the Agent Client Protocol. Verified against Claude and Codex.
  A pad names a role, not a session, so it still means something tomorrow.
- **Terminals** PushOS runs itself, with no window opening. They keep running
  when nothing is looking, and the last thing each one said is on the display.
  A pad names the terminal, not the process.
- **Assigning a session to a control** in Studio: everything running is listed,
  and either its name or the exact session can be bound to any control.
- **Workflows** that run themselves and survive a restart: plan, build, test,
  review, with somewhere to go when the tests fail. A step is written down
  before the step after it runs, and a run waiting on a decision is still
  waiting when PushOS comes back.
- **Push to talk**, on this Mac and nowhere else. Hold a control and PushOS
  listens; let go and it works out what you said. There is no wake word: your
  finger decides, and the display says so while it is down. A short phrase you
  wrote means exactly one thing and runs it, and anything else goes to the agent
  you were already working with. Anything irreversible waits for a press,
  because transcription is not authorisation.
- **Every coding session on this Mac**, on pads, including the ones you started
  yourself. PushOS finds them in tmux, in Terminal, and in Claude Code's own
  list of its sessions, so a Claude Code session in VS Code or Cursor shows up
  too. Claude Code says what each of its sessions is doing, so a pad blinks
  amber when one is waiting for you rather than when its screen happens to end
  in a numbered list; anything else, Codex included, is read from its screen.
  PushOS shows the last thing each one said and types into whichever you
  selected. One in an editor's panel it can show and take you to, but not type
  into, and it says so.
- **A session per pad**, kept by tmux. The first press starts a named session in
  a folder and runs your agent in it; every press after that comes back to it.
  It keeps running when its window is closed and when PushOS restarts, and it
  opens in any terminal, VS Code's included, with `tmux attach -t` and its name.
- **Sequences**: several actions behind one gesture. "Start work" can open an
  editor, a terminal and a playlist from one pad. Every step goes through the
  same checks a press does, runs in order and stops at a failure unless marked
  optional, and a sequence that could run itself forever is refused when the
  file is read.
- **Packs**, installable directories of agents, workflows, pages and bindings.
  Nothing is written until you have seen what one adds and what it is asking
  for, and a pack cannot grant itself anything: that is enforced rather than
  promised. Removing one deletes what was copied in and nothing else. A running
  PushOS lists, reviews and installs them over its control socket, and a pack
  left in `available/` beside your configuration is offered without being
  loaded. The Operator Pack puts fifty-six roles across seven rows of pads.
- **Notes**, kept as Markdown files in directories you chose. One pad writes
  down what you just dictated, one finds it again, one puts what it found in
  front of the agent you are working with. PushOS keeps an index so search is
  quick; delete it and the notes are still there, because the files are the
  truth and the index is derived.
- **Projects**, where one pad represents a whole coding project. Selecting it
  brings the project's bindings into force, starts agents and terminals in its
  directory with the providers it prefers, and puts you back on the page you
  were last on. A project that runs several coding agents can give each its own
  git working tree, so two of them never edit the same checkout.

## Installing

PushOS needs a Rust toolchain. The version is pinned in `rust-toolchain.toml`
and installed automatically by `rustup`.

```bash
git clone https://github.com/Roman-Silantev/pushos
cd pushos
cargo build --release
./target/release/pushos init
./target/release/pushos doctor
```

`libusb` is built from source as part of the build, so there is nothing to
install first.

The program itself is under 10 MB. What the build leaves behind is larger, and
is kept deliberately small: about 1 GB for the tests, 1.6 GB with a release
build beside them, and it does not grow when you rebuild. `cargo clean` gives
all of it back. The optional `whisper` speech engine is the exception, since it
brings a machine-learning stack with it, and it is only built when you ask for
it with `--features whisper`.

macOS will ask for permission the first time PushOS controls an application or
runs a Shortcut. `pushos doctor` reports what is reachable and what is not.

### As an app that starts at login

Run from Terminal, PushOS borrows Terminal's permissions and stops when its
window closes. Installed as an app, it has its own, starts when you log in,
and is restarted if it ever crashes:

```bash
./target/release/pushos app install   # build the app, sign it, start it
./target/release/pushos app status    # installed, at login, signed, running
./target/release/pushos app uninstall # remove it; configuration stays
```

It lives in `~/Applications/PushOS.app` with no window and no Dock icon, runs
the configuration it was installed with, and logs to
`~/Library/Logs/PushOS/pushos.log`. Install again after rebuilding to update it.

macOS keeps an app's permissions only while its signature stays the same. Make
a signing certificate once and every install is signed with it, so the
microphone and Terminal permissions you grant are kept:

1. Open Keychain Access.
2. From the Keychain Access menu choose Certificate Assistant, then Create a
   Certificate.
3. Name it `PushOS Local`, with Identity Type Self Signed Root and Certificate
   Type Code Signing.
4. Run `pushos app install` again. macOS asks once whether `codesign` may use
   the certificate; choose Always Allow.

Without one, each install is signed for that build only and macOS asks for the
permissions again after the next one.

### PushOS Studio

Studio is the app for seeing and changing what every control does. PushOS runs
without it, and closing it does not stop PushOS. Building it needs Node.js as
well as Rust:

```bash
cd studio
npm install
npm run app
```

That builds Studio, puts it in `~/Applications/PushOS Studio.app` beside PushOS
so Spotlight finds it, and removes what the build left behind. The app is 3 MB
and builds in under a minute; `npm run app -- --keep-build` keeps the build
directory for a quicker rebuild. Run it again to update.

## Configuring

Configuration is human-readable TOML and is the source of truth. There is no
hidden database.

Start from a preset rather than a blank file:

```bash
pushos presets
pushos init --preset developer
```

Six ship, from `blank` to `ai-engineer`, and each is tested against the
providers PushOS actually ships: every action exists, every binding has the
permission it needs, and nothing is granted that is not used. See
[`presets/`](presets/).

Add to a surface with packs, which bring their own agents, workflows and pages:

```bash
pushos pack show packs/review
pushos pack install packs/review
```

Nothing is written until you have seen what a pack adds and what it asks for,
and a pack cannot grant itself anything. See [`packs/`](packs/).

```toml
[[pages]]
id = "development"
name = "Development"

[[bindings]]
control = "button.play"
gesture = "press"
action = "media.play_pause"

[[bindings]]
control = "pad.0"
gesture = "tap"
page = "development"
action = "app.launch"
target = "Cursor"
label = "Cursor"

[[bindings]]
control = "pad.0"
gesture = "hold"
page = "development"
action = "shell.run"
params = { program = "cargo", args = ["test"], cwd = "~/Projects/pushos" }

# A terminal PushOS runs itself. Tap opens it, and pressing again selects the
# one already running rather than starting a second.
[[bindings]]
control = "pad.24"
gesture = "tap"
page = "development"
action = "terminal.open"
label = "Tests"
params = { name = "tests" }

[[bindings]]
control = "pad.24"
gesture = "hold"
page = "development"
action = "terminal.run"
target = "name:tests"
params = { text = "cargo test" }

# A project. One pad selects it, and everything afterwards happens in it.
[[workspaces]]
id = "sydclaw"
name = "Sydclaw"
root = "~/Projects/sydclaw"
home_page = "development"

[[bindings]]
control = "button.upper_5"
gesture = "press"
action = "workspace.select"
target = "sydclaw"
label = "Sydclaw"

# A binding scoped to a project only fires while that project is selected.
[[bindings]]
control = "pad.32"
gesture = "tap"
workspace = "sydclaw"
action = "terminal.run"
target = "name:tests"
params = { text = "npm test" }

# Push to talk. One control, bound twice: press to listen, release to stop.
[voice]
engine = "apple"
request = "agent.prompt"

[[voice.commands]]
phrase = "stop"
action = "agent.stop"

# Held rather than run. Only a press releases it.
[[voice.commands]]
phrase = "ship it"
action = "workflow.start"
target = "ship"
confirm = true

[[bindings]]
control = "button.select"
gesture = "press"
action = "voice.listen"

[[bindings]]
control = "button.select"
gesture = "release"
action = "voice.transcribe"

# Notes. Markdown files in a directory you chose; the index is derived.
[memory]
brief = "agent.prompt"

[[memory.sources]]
id = "notes"
path = "~/Notes/pushos"
writable = true

[[bindings]]
control = "pad.56"
gesture = "tap"
action = "memory.recent"

# Finds notes and puts them in front of the agent you are working with.
[[bindings]]
control = "pad.57"
gesture = "tap"
action = "memory.brief"
```

### One session per pad

A pad can keep a worker. The first tap starts a tmux session under a name, in a
folder, types a command into it and opens a Terminal window onto it. Every tap
after that brings the same session back, opening a window again if you closed
it, and selects it, so what you dictate next goes there.

```toml
[permissions]
granted = ["shell.execute"]

[sessions]
watch = true

[[bindings]]
control = "pad.56"
gesture = "tap"
action = "session.open"
label = "Client 1"
params = { name = "client-1", cwd = "~/Work/client", command = "claude" }

# The same worker from anywhere else on the surface.
[[bindings]]
control = "button.upper_1"
gesture = "press"
action = "session.focus"
target = "name:client-1"
```

It needs tmux, which `brew install tmux` provides and `pushos doctor` checks
for. `window = false` starts a worker without opening a window. The command is
typed into a shell rather than run in its place, so when the agent exits you are
left in the same folder instead of losing the window. To watch one from VS Code,
run `tmux attach -t client-1` in its terminal.

### What a role's agent may do

A role can say what the agent filling it may do, and PushOS holds it to that:

```toml
[[agents]]
id = "markets"
name = "Markets"
objective = "Watch the markets and report what moved. Research only."
permissions = ["filesystem.read", "network"]
```

That agent can read and look things up, and cannot run a command, change a
file, commit or push. It is held there in three places:

- **When it starts.** A Claude agent is started without the tools the role does
  not allow, and cannot be switched into the mode that asks about nothing. A
  Codex agent that may not write starts read-only.
- **When it asks.** A request to do something the role does not allow is
  refused on the spot, without asking you, and the display says so.
- **Everything else** it asks about still comes to you.

Leave `permissions` out and the role narrows nothing, so the agent works with
its own settings. List nothing, `permissions = []`, and it may do nothing any
permission describes. Git is enforced per command for Claude: a role with
`shell.execute` but not `git.push` still cannot push.

Two rules the configuration enforces, both so that you can predict what a pad
does by reading the file:

- **Ambiguity is refused.** Two bindings on the same control, gesture, scope and
  priority is an error, not a coin toss.
- **Nothing is permitted unless it is listed.** There is no wildcard grant, and
  `shell.execute` is off until you turn it on. So is `microphone.listen`, and
  macOS asks separately the first time you hold the control. Writing a note to
  disk needs `filesystem.write`.

Run `pushos check` after editing. Every problem is reported at once, each naming
the binding it came from.

## Leaving it on all day

A Push 2 can sit on a desk showing your sessions for a twelve-hour day, and
PushOS is built to let it without wearing it out.

The pads, the buttons and the light behind the screen are LEDs. LEDs lose
brightness with the hours they spend driven hard, and nothing restores that
afterwards. The screen is an LCD, which can keep a faint trace of a picture
held for hours; that usually fades, but it is better not to cause it.

So PushOS never runs the hardware at full. After ten minutes untouched it dims,
still readable. After thirty the screen goes black and every light goes out
except the pads asking for you, which stay on, dimmed. Anything new asking for
you wakes the surface. A press on a dark surface only wakes it, and does
nothing else, so reaching for it blind cannot set anything off.

```toml
[surface]
brightness = 70           # percent, while in use
dim_after_minutes = 10    # 0 never dims
sleep_after_minutes = 30  # 0 never goes dark
```

`pushos check` prints what is in force. On USB power alone the Push 2 limits
its own brightness far below any of this, which is why the screen looks dim
without its power supply; the settings matter once it is plugged in.

The Push 2 has its own power supply and keeps whatever it was last shown for as
long as that supply is on, even with nothing driving it. So PushOS also puts
every light and the screen out whenever it stops being able to look after them:

- **When it stops.** Ctrl-C, closing the terminal window it runs in, `kill`
  and logging out all turn the lights off and hand the Push back. Only
  `kill -9`, which no program can answer, leaves them as they were.
- **When the Mac sleeps.** macOS tells PushOS before it sleeps, and PushOS puts
  the Push dark first. When the Mac wakes, the surface comes back dimmed, so a
  Mac that wakes in the night to fetch mail does not light the desk; touch it to
  brighten it.

## Architecture

```
crates/
├── pushos-domain      the vocabulary and the ports. No infrastructure at all.
├── pushos-push2       MIDI, USB and the display. The only place numbers live.
├── pushos-bindings    gesture recognition and binding resolution
├── pushos-actions     the dispatcher and the built-in providers
├── pushos-ui          rendering for the display and the lights
├── pushos-config      loading, validation and hot reload
├── pushos-storage     local SQLite, with one write owner
├── pushos-agents      agent roles, live sessions and the routing between them
├── pushos-acp         the Agent Client Protocol adapter
├── pushos-terminal    managed pseudo-terminals and what PushOS knows of them
├── pushos-workflows   work that runs itself, and what it remembers
├── pushos-workspaces  projects, what each restores, and its working trees
├── pushos-voice       push to talk: capturing, recognising and routing speech
├── pushos-memory      notes as Markdown files, and finding them again
├── pushos-packs       installable packs: reading, reviewing, installing
├── pushos-api         the local control socket and its protocol
├── pushos-runtime     the event bus, supervision and wiring
├── pushos-macos       the macOS half: processes, media, apps, Shortcuts
├── pushos-testkit     FakePush and a fake for every other port
└── pushos-cli         the `pushos` binary

studio/                a separate configuration application (Tauri)
```

Dependencies point inward. The domain owns the interfaces; adapters implement
them. A new action provider requires no change to the hardware adapter, the
resolver or the renderer, and a new host means one new crate.

`docs/ARCHITECTURE.md` explains the decisions behind that shape.

## Testing

The whole core is testable with no hardware, no agent provider and no macOS
permissions.

```bash
cargo test --workspace
```

`FakePush` satisfies the same interfaces as the real adapter, so tests exercise
the production path rather than a parallel one. The concurrency scenarios the
specification calls out are covered explicitly: a hold interrupted by a
disconnect, a release arriving after a reset, a configuration reload while a
press is in flight.

Some things cannot be faked and are not. The pseudo-terminal adapter is tested
against real processes: a program is started, typed into, read from, stopped and
its exit status collected. Three bugs were found that way and by no other means.
Worktree isolation is likewise proved against a real repository, because the
question is whether the operator's own git does what PushOS asks of it:

```bash
cargo run -p pushos-workspaces --example isolate -- <repository> <where to put trees>
```

Speech is the same: an engine that transcribes a fixture correctly in a unit
test would prove nothing about the one macOS actually runs. Both are checked
against real audio, which on a Mac you already have a way of making:

```bash
say -o /tmp/said.aiff "stop the build and show me the failing test"
afconvert -f WAVE -d LEI16@16000 -c 1 /tmp/said.aiff /tmp/said.wav
cargo run -p pushos-voice --example transcribe -- /tmp/said.wav
```

Notes are the same story: the claim is that they are files, so the tests write
real files into real directories and search them through real SQLite with FTS5
turned on. A note that only existed in a fake would prove nothing about the one
you can open in your own editor.

## Where this is up to

Built and tested:

| Milestone | What it covers |
| --- | --- |
| Phase 0 | Push 2 hardware: controls, lights, display, reconnect, FakePush |
| Phase 1 | The action and binding kernel |
| Phase 2 | The display: pages, widgets, notices, overlays |
| Phase 3 | Page, application, Shortcut, media and shell actions |
| Phase 4 | PushOS Studio and the control socket it talks to |
| Phase 5 | Agents over the Agent Client Protocol |
| Phase 6 | Sessions and terminals, and assigning one to a control |
| Phase 7 | Projects: one pad for a whole codebase, with isolated worktrees |
| Phase 8 | Durable workflows that survive a restart |
| Phase 9 | Push to talk, recognised on this Mac |
| Phase 10 | Notes: Markdown on disk, searchable, and briefings for agents |
| Phase 11 | Presets: six tested surfaces to start from |
| Phase 12 | Packs: installable agents, workflows and pages, with a review |
| Compound actions | Sequences of actions behind one gesture, refused if they could loop |
| Hardware care | Brightness below full, dimming and sleep when untouched, dark when PushOS stops or the Mac sleeps |
| Every session | tmux, Terminal and Claude Code's own session list merged into one; a tmux session per pad |
| Footprint | A 4.8 MB program, and a build directory that stays under a gigabyte |

`SPEC.md` and `AGENT_PACKS_SPEC.md` hold the full plan.

## Contributing

`AGENTS.md` is the working agreement, for people and for AI builders alike. The
short version: build one milestone at a time, keep modules small and named after
their responsibility, own your state rather than sharing a lock, and give every
race condition a regression test.

Before opening a pull request:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

## Licence

Apache-2.0. See `LICENSE`.

Inter is used under the SIL Open Font Licence; see `assets/fonts/`.

Ableton and Push are trademarks of Ableton AG. This project is not affiliated
with or endorsed by Ableton.
