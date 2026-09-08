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
pushos init      # write a starting configuration
pushos check     # validate it without running anything
pushos doctor    # report on hardware, configuration and host integrations
pushos run       # run it
pushos status    # ask a running PushOS what it is doing
pushos bindings  # list what it has bound
```

- **All 141 Push 2 controls**, addressed by name. `pad.0`, `button.play`,
  `encoder.master`. The numbering table is generated from Ableton's published
  hardware map, and no MIDI number appears anywhere else in the system.
- **Gestures**: press, release, tap, double tap, hold, and their Shift variants,
  plus encoder turns and touch. One component owns all the timing.
- **Pages**, so the same 64 pads mean different things in different contexts.
- **Bindings** with deterministic precedence: `workspace+page`, `workspace`,
  `page`, `global`.
- **Actions**: page navigation, applications, Apple Shortcuts, media control and
  shell commands.
- **The display**, drawn natively for 960x160.
- **Hot reload**, validated. A bad edit is reported and ignored; the running
  surface is untouched.
- **A simulated surface**, so you can work on pages and bindings with no
  hardware attached: `pushos run --fake`. It says so everywhere it appears; a
  stand-in is never presented as a Push 2.
- **PushOS Studio**, a configuration application that talks to a running PushOS
  over a local socket. See [`studio/`](studio/).

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

macOS will ask for permission the first time PushOS controls an application or
runs a Shortcut. `pushos doctor` reports what is reachable and what is not.

## Configuring

Configuration is human-readable TOML and is the source of truth. There is no
hidden database.

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
```

Two rules the configuration enforces, both so that you can predict what a pad
does by reading the file:

- **Ambiguity is refused.** Two bindings on the same control, gesture, scope and
  priority is an error, not a coin toss.
- **Nothing is permitted unless it is listed.** There is no wildcard grant, and
  `shell.execute` is off until you turn it on.

Run `pushos check` after editing. Every problem is reported at once, each naming
the binding it came from.

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

## Where this is up to

Built and tested:

| Milestone | What it covers |
| --- | --- |
| Phase 0 | Push 2 hardware: controls, lights, display, reconnect, FakePush |
| Phase 1 | The action and binding kernel |
| Phase 2 | The display: pages, widgets, notices, overlays |
| Phase 3 | Page, application, Shortcut, media and shell actions |
| Phase 4 | PushOS Studio and the control socket it talks to |

Next, in order: agent integration over the Agent Client Protocol, sessions and
terminals, workspaces, durable workflows, and voice. `SPEC.md` holds the full
plan.

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
