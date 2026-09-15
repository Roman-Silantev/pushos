# Operator Pack

Fifty-six roles, one to a pad, laid out so a hand learns where each one lives.

```text
row 1   Build       architect  implementer  reviewer  tester  debugger  refactorer  documenter  releaser
row 2   Clients     onboarder  scoper  status  handover  support  escalations  retro  offboarder
row 3   Growth      prospector  outreach  follow up  proposal  pricing  pipeline  case study  partners
row 4   Money       invoices  chaser  expenses  runway  contracts  compliance  tax  suppliers
row 5   Markets     markets  portfolio  earnings  macro  competitors  industry  diligence  analyst
row 6   Comms       inbox  replies  meetings  notes  newsletter  social  editor  translator
row 7   Mine        day  week  learning  reading  health  home  travel  journal
row 8               left for whatever is already running here
```

Tap starts a role. Hold selects it without starting it, so the encoders and the
row under the screen act on it.

A role's pad shows what its agent is doing: pulsing while it works, blinking
amber while it waits for you, blinking red if it failed. A role with nothing
happening glows like any other pad, so the lights that change are the ones worth
looking at, and one waiting for you stays lit when the surface goes dark.

## The bottom row

Nothing in this pack binds pads 56 to 63. That row belongs to the terminals you
already have open, which the `sessions` preset puts there and PushOS watches
without having started them. Install both and the grid reads as one thing: the
work you can start above, the work already running below.

## A role is not a process

Fifty-six pads do not mean fifty-six programs. A role is a description of a job
and a pad is how you start one; what runs is what you have started. Two or three
at once is a working day, and the machine decides the ceiling, not the grid.

## What each role may do

Every role lists its permissions, and PushOS holds its agent to them. All of
them read. Only the roles that draft or record something write files, only the
roles that build, test or check history run commands, and only the implementer,
refactorer and releaser commit. None of them sends anything, pushes, deploys or
touches money: those stay with you. Change a role's `permissions` in its file
under `agents/` if your work needs more.

## Markets

The markets row reads, searches and reports, and nothing else: none of its roles
can run a command, and only the portfolio keeps a record. They do not place
orders, they do not sign in to a broker, and they do not tell you what to buy or
sell. Every decision stays with you, and wiring a role to a broker is not
something this pack does or helps with.

The health role is the same shape: it keeps the record you gave it and reports
what changed. It does not diagnose and it does not advise on treatment.

## Installing

Drop it in `available/` next to your configuration and it shows up in
`pushos pack list` and in Studio, without being loaded. Or install it straight
from here:

```bash
pushos pack show packs/operator      # what it is, and what it would change here
pushos pack install packs/operator   # after you have read that
```

Installing tells you the one line to add to reach the page. A pack is not
allowed to bind a control outside its own pages, because installing something
should not change what a surface you already built does, so opening the page is
the one thing you add yourself.
