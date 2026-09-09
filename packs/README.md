# Packs

A pack is a directory of ordinary PushOS configuration with a manifest on the
front. Installing one copies it in; removing one deletes what was copied and
nothing else. There is no package database and no lock file: what is installed
is what is in `packs/` under your configuration directory, which you can list
with `ls` and read with any editor.

```bash
pushos pack show packs/review      # what it is, and what it would change here
pushos pack install packs/review   # after you have read that
pushos pack list                   # what is installed
pushos pack disable review         # keep it, stop loading it
pushos pack remove review          # delete what was copied in
```

## What installing means

Nothing is written until you have seen the review. It says what the pack adds,
what capabilities it is asking for, which of those you already allow, and
whether it would install cleanly at all.

```text
Code Review Pack 1.0.0 (review)
  A reviewer who reads what was written, a page to watch them from, and a
  workflow that builds, tests and asks before anything ships.

adds:
  1 agent
  1 workflow
  1 page
  7 bindings

would grant:
  + shell.execute
  ? filesystem.read (optional; the pack works without it)

it would install here
```

A `!` rather than a `+` marks something PushOS cannot undo, such as deploying
to production or sending something outside the machine. Those are worth reading
twice.

**A pack cannot grant itself anything.** That is enforced, not promised: a pack
whose own files contain a permissions section is refused, and so is one that
arrives carrying the file PushOS writes grants into. What a pack may do is what
you agreed to, and the grant is written to `00-granted.toml` inside the
installed pack so you can see where it came from. Delete that file and the
grant is withdrawn; the pack stays installed.

## Shape

```text
review/
├── pack.toml            the manifest: what it is and what it asks for
├── README.md
├── agents/              roles
├── workflows/           work that runs itself
├── pages/               what the controls mean
└── bindings/            what the pads do
```

Every `.toml` under the pack except the manifest is loaded as configuration, at
any depth. The directory names are a convention for people, not a rule PushOS
enforces.

Packs load before your own files, so where a setting can only have one value,
what you wrote wins. A pack is a starting point you were offered, not something
that overrules you.

## What a shipped pack promises

Each of these is a test, run against the providers PushOS actually ships.

- **It installs onto a plain configuration**, which also proves it makes no
  ambiguous binding and names no page or control that does not exist.
- **It asks for exactly what its bindings need.** Asking for less means a pad
  that fails under a finger; asking for more teaches you to skim the review.
- **It stays on its own pages.** A pack binding a control globally would change
  what a surface you already set up does, which is not what installing means.
- **Removing it takes only its own things**, including the grant.

## Writing one

Start by copying `review/`. The manifest needs an `id`, a `name` and a
`version`; everything else is optional. Declare what you need under
`[requires]`, put your configuration in the directories above, and check it
with `pushos pack show <directory>` before you install it anywhere.

Two things worth knowing. Scope your bindings to a page your pack declares, so
installing cannot change what an existing control means. And do not give agent
roles a `preferred` provider list: a pack should not decide which vendor fills
a role. Say what you expect under `[requires] providers` instead, and PushOS
shows that at install.
