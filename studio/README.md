# PushOS Studio

Configuration for PushOS. Not required for normal operation.

Studio talks to a **running** PushOS over a local socket. It embeds no runtime
of its own, so closing Studio does not stop PushOS, and a crash in Studio cannot
take the surface down.

## What it does

- Shows every control the running PushOS reports, as a virtual Push 2
- Marks which controls are bound on the page you are editing, with a count
- For a selected control, shows every gesture and what each one does
- Assigns an action, a target and a caption, and saves
- Runs a binding once so you can see what it does before committing to it
- Removes a binding

Everything on screen comes from PushOS. Studio hard-codes no list of actions,
gestures or controls, so it cannot offer something that will not work.

## What it does not do

Studio does not own your configuration. Your TOML files remain the source of
truth, and Studio edits them in place: comments, ordering and hand alignment
survive a save. An edit that would produce an unusable surface is refused, with
each problem listed, and nothing is written.

## Running it

Start PushOS first:

```bash
pushos run
```

Then, from this directory:

```bash
npm install
npm run tauri dev
```

To build the application:

```bash
npm run tauri build
```

## Working on the interface

With no PushOS running and no Tauri bridge present, `npm run dev` serves the
interface in a browser against sample data, so the layout can be worked on
without hardware or a running runtime. That mode announces itself in the header,
and the sample data is loaded on demand so a production build never carries it.

```bash
npm run dev      # http://localhost:5183
npm run check    # types
```

## The icon

`icons/icon.png` is drawn by `icons/generate.py`, using only the standard
library, so it can be changed without a graphics application:

```bash
python3 icons/generate.py icons/icon.png
npx tauri icon icons/icon.png
```

## Licence

Apache-2.0, the same as the rest of PushOS.
