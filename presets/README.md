# Presets

A preset is ordinary PushOS configuration. There is no separate format, no
installer, and nothing PushOS does to a preset that you could not do by editing
the file. That is the point: what you start from is something you can read.

```bash
pushos presets                      # what ships
pushos init                         # write the starter
pushos init --preset developer      # write another
pushos check                        # validate what you wrote
```

`pushos init` writes to your configuration directory and refuses to overwrite
an existing file unless you pass `--force`.

## What ships

| Preset | What it is for |
| --- | --- |
| `starter` | A guided tour. Everything PushOS can do, most of it commented out. |
| `blank` | One page and the transport. The least configuration that still does something. |
| `developer` | Editors, terminals, projects and tests, across six pages. |
| `ai-engineer` | Agents first: roles, workflows, push to talk and notes. |
| `automation` | Apple Shortcuts and applications. No agents, no terminals. |
| `music` | Playback, volume and the notes you take while listening. |

Five shapes rather than one per job title. An `ai-engineer` and a `researcher`
preset would be the same file with different words in it, and a list of near
duplicates is harder to choose from than a short list of genuinely different
surfaces. Start from the closest one and change it; that is what they are for.

## What every preset promises

Each of these is a test, run against the providers PushOS actually ships rather
than a list written down beside them.

- **It is valid.** No ambiguous bindings, no page that does not exist, no
  control PushOS cannot address.
- **Every action it names exists**, in bindings, spoken phrases, workflow steps
  and briefing destinations alike.
- **Every binding has the permission it needs.** Install a preset and every pad
  on it works, rather than failing under a finger with a message about a
  capability you never chose to withhold.
- **It grants nothing it does not use.** A preset that over-granted would teach
  you that the permission list means nothing.
- **You can get back.** Every preset binds something that moves between pages,
  so no surface can strand you.
- **Its commented examples work too.** Uncomment one and PushOS still starts.
  A documented example that no longer works is the same trap, discovered later.

Three things a preset cannot promise. The applications and Shortcuts it names
are examples, an agent preset needs an agent provider on your `PATH`, and the
shape of an action's parameters is checked when the action runs rather than when
the file is read. `pushos doctor` reports on the first two.

## Adding one

Put the file here, add it to `ALL` in `crates/pushos-cli/src/presets.rs` with a
one-line summary, and run `cargo test -p pushos-cli`. The tests above then apply
to it, and `pushos presets` lists it.
