# Architecture

This explains why PushOS is shaped the way it is. `SPEC.md` says what to build;
this says what the decisions cost and what they buy.

## The one abstraction

```
Control  →  Gesture  →  Binding  →  Action
```

Every physical event goes through all four steps, always in that order, with no
shortcuts. There is deliberately no code anywhere that says "if pad 5 then run
Claude". A pad has no idea what it does.

The price is indirection: tracing what a pad does means reading configuration
rather than following a call. The return is that a user can rebind the whole
surface without writing Rust, a new provider costs no change to the hardware
layer, and the same machinery serves agents, media, Shortcuts and workflows
alike.

## Dependencies point inward

`pushos-domain` holds the vocabulary and the interfaces. It has no knowledge of
MIDI, USB, SQLite, macOS or any model vendor, and it must stay that way. Every
other crate depends on it; it depends on none of them.

Adapters implement the domain's ports:

| Port | Real adapter | Fake |
| --- | --- | --- |
| `PushOutput` / `PushInput` | `pushos-push2` | `FakePush` |
| `MediaController` | `pushos-macos` | `FakeMedia` |
| `ApplicationLauncher` | `pushos-macos` | `FakeApplications` |
| `ShortcutRunner` | `pushos-macos` | `FakeShortcuts` |
| `ProcessRunner` | `pushos-macos` | `FakeProcesses` |
| `ActionProvider` | `pushos-actions` | `RecordingProvider` |
| `Clock` | `SystemClock` | `ManualClock` |

Every fake satisfies the same interface as the real thing, so a test exercises
production code rather than a parallel implementation. That is why the whole
core is testable with no Push 2, no agent account and no macOS permissions.

## State is owned, not shared

There is no `Arc<Mutex<AppState>>`. Each piece of state has exactly one owner
and is changed through it:

- The **input task** owns the gesture recogniser and the surface state. It is
  the only thing that mutates either, so the input path takes no lock at all.
- The **storage task** owns the database connection. Every read and write is
  ordered through it, so there is one obvious answer to who may write and when.
- The **display thread** owns the USB handle. It takes frames through a
  single-slot mailbox that keeps only the newest.
- The **MIDI writer thread** owns the output connection, because the library
  requires exclusive access to it.

Readers get immutable snapshots. The renderer, for instance, never touches live
state, which is what makes "rendering never waits on anything" enforceable
rather than aspirational.

## Where the hardware stops

`pushos-push2` is the only crate that knows a MIDI note number, a controller
number, a USB endpoint or the display's wire format. Everything above it speaks
in `ControlId` and `ControlEvent`.

The numbering table is generated from Ableton's published `Push2-map.json`, and
the encodings are verified against the worked examples in the interface manual:
the system-exclusive palette message, the sixteen-bit pixel packing and the
signal-shaping XOR. A test holds the domain's idea of which controls have lights
and the adapter's addressing table to the same answer, because a disagreement
there would silently lose a light.

Two hardware facts shape the design more than they might appear to:

- **The panel blanks itself after two seconds without a frame.** Keeping it
  awake is the adapter's job, not the renderer's, so the render loop only draws
  when something changed.
- **Lights take a palette index, not a colour.** PushOS installs its own palette
  as a uniform 5x5x5 cube, which makes colour-to-index exact arithmetic instead
  of a search, and gives the white-only buttons a sensible brightness for free.

## Timing lives in one place

`GestureRecognizer` owns every hold threshold, double-tap window and Shift
decision in the system. No feature runs its own timer, so the surface behaves
consistently everywhere.

One decision inside it is worth stating. A tap can only be confirmed once the
double-tap window has passed without a second press. Waiting that long on every
control would put a quarter-second of latency on the most common gesture on the
surface, so the recogniser only waits for controls that actually have a
double-tap binding. Everything else reports its tap the moment the control is
released. The set of such controls is derived from the binding table, so it
stays correct across a configuration reload.

The recogniser never reads a clock. Time arrives as an argument, which is why
its tests run instantly and never flake.

## One thing animates, and it stops

A still screen is drawn once and left alone. The waiting screen is the single
exception: it redraws at the specification's ceiling of thirty frames a second
while it is up, and stops the moment it goes away. An end-to-end test asserts
both halves, because an animation that never ends would keep an idle machine
awake for nothing.

The frame counter lives in the render task rather than in the snapshot. If time
passing republished state, the input pipeline would be woken thirty times a
second to be told that nothing had happened.

The mascot itself is drawn as geometry. It is written in quadrant block
characters, and every one of those is a rectangle, so PushOS draws the
rectangles directly: crisp at any size, no dependency on the typeface carrying
those glyphs, and each quadrant separately addressable for the sweep.

## Configuration is replaced, never edited

A reload parses, validates and builds a complete new configuration before
swapping it in. An invalid edit leaves the running surface exactly as it was,
and a snapshot taken before a reload stays readable while the action holding it
finishes.

Validation collects every problem rather than stopping at the first, and each
one names the binding it came from. These files are written by people and by
Studio, and neither can act on "invalid configuration".

Two things are refused rather than resolved:

- **Ambiguity.** Two bindings that precedence cannot separate is an error. An
  operator has to be able to predict the surface by reading the file.
- **Unknown keys.** A misspelled field is an error, not a binding that silently
  does nothing.

## Studio is a separate process, and stays one

PushOS Studio embeds no runtime. It reaches a running PushOS over a Unix socket
in the state directory, owner-readable only, with a newline-delimited JSON
protocol small enough to drive by hand when something is wrong. Closing Studio
stops nothing, and a crash in Studio cannot take the surface down.

The socket carries a protocol version and a client refuses to talk to a runtime
speaking a different one, rather than sending edits that might be read
differently from how they were meant.

Two decisions about what the socket will do are worth stating:

- **Testing a binding runs a binding**, not an arbitrary action. A client can
  only trigger something the operator has already configured, and the
  dispatcher's permission check still applies.
- **Edits go through the same path a hand edit does.** The configuration is
  parsed with a format-preserving reader, changed, validated as a whole, and
  only then written, each file replaced atomically. A file Studio has saved is
  still a file its author recognises, and an edit that would break the surface
  changes nothing on disk.

The runtime is reloaded after a successful edit, so Studio cannot report success
while the operator's pads carry on unchanged.

## Failures are classified, not stringified

Every error carries a class: retryable, validation, permission,
user-action-required, or component failure. The class decides whether a caller
may retry, must ask the operator, or should tear a component down. Nothing
converts an error to a string and hopes.

A non-zero exit from a shell action is not an error at all. It is the program's
answer, and it comes back as a failed action with the exit code.

## Safety decisions worth knowing about

- **There is no way to express a shell command line.** A shell action names a
  program and its arguments separately and they stay separate to the syscall.
  Arguments therefore need no escaping, because nothing will reinterpret them. A
  program field that looks like a command line is refused rather than misread.
- **Only `http` and `https` URLs may be opened.** `open` will start a registered
  handler for any scheme, so allowing arbitrary schemes would quietly turn
  "open a link" into arbitrary execution.
- **AppleScript is fixed text.** Values are passed to the script's run handler,
  never spliced into source about to be compiled. The one value the language
  cannot take as a variable, an application name, is validated against a narrow
  character set.
- **Every media query guards on the player already running**, because an
  unguarded `tell` would launch a music player just to read the volume. Pressing
  play is the deliberate exception.
- **Nothing is permitted unless configured.** Permission is checked in the
  dispatcher rather than inside providers, so a provider is never the only thing
  standing between a binding and a capability.

## What is deliberately absent

No cloud service. No message broker. No container runtime. No vector database.
No Electron. No mandatory note-taking integration. Local, small and predictable
is the point, and each of those would be added only against a measured
requirement.

Media control uses AppleScript rather than the `MediaRemote` framework: that
framework has been gated behind a private entitlement since macOS 15.4 and
returns nothing to unentitled callers.
