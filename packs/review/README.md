# Code Review Pack

Adds a reviewer role, a page to watch it from, and a workflow that builds,
tests and stops for a person before anything ships.

```bash
pushos pack show packs/review      # what it is and what it asks for
pushos pack install packs/review   # after you have read that
```

It expects an agent provider on your `PATH`. `claude` and `codex` are
recognised by name. Without one, the page and the workflow still install and
`pushos doctor` says what is missing.

## What it adds

- **`reviewer`**, a role whose objective is to read a diff and say what is
  wrong with it.
- **A `review` page**, with pads to start the reviewer, start the workflow,
  and answer it.
- **A `review` workflow**: build, test, read, then ask. The asking step is the
  one that makes the rest safe, because a workflow that could ship without a
  person would be a workflow nobody should install.

## What it does not do

It does not grant itself anything, deploy anything, or send anything. The one
capability it asks for is running a subprocess, which is what a terminal step
needs.
