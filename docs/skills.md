# Skills

A skill is a short instruction file that teaches the agent how to do one thing —
call a particular API, follow a house convention, drive a tool it would
otherwise have to guess at. Skills are read only when they are relevant, so a
long one costs nothing until it is needed.

## What a skill is on disk

A skill is a directory whose root holds a `SKILL.md`:

```
~/.myra/skills/
  myrarouter-image/
    SKILL.md
```

`SKILL.md` is YAML frontmatter followed by markdown:

```markdown
---
name: "myrarouter-image"
description: "Generate images from a text prompt. Use when the user asks for a picture, illustration or logo."
---

# Image generation

## Endpoint

`POST $MYRAROUTER_URL/v1/images/generations`
...
```

`description` is required — it is what the agent reads to decide whether the
skill applies, so write it as *when to use this*, not *what this is*. `name` is
optional and defaults to the directory name.

Where skills are read from, in order of specificity:

| Location | Scope |
| --- | --- |
| `<project>/.agents/skills` | this repository |
| `~/.myra/skills` | this machine |
| `/etc/myra/skills` | this host, set by an administrator |

## Installing from the gateway

The MyraRouter gateway publishes a skill catalog, and `myra skills` installs
from it. There is no host or key to configure: the commands use whichever
gateway the CLI is already signed in to.

```bash
myra skills list                 # installed and available, side by side
myra skills list --installed     # only this machine; no network
myra skills list --json          # machine-readable

myra skills install myrarouter-chat
myra skills install myrarouter-chat myrarouter-image

myra skills sync                 # install everything published, update the rest
myra skills sync --dry-run       # show what would change

myra skills remove myrarouter-chat
```

`list` marks each skill:

- **installed** — on this machine and published by the gateway
- **available** — published, not installed yet
- **local** — on this machine, not in the catalog. Usually hand-written.
  `sync` never touches these.

Installing a skill that is already present replaces the whole directory, which
is how an update happens — so local edits to a catalog skill are overwritten by
the next `install` or `sync`. Edit it in the dashboard instead, or copy it to a
new name that the catalog does not publish.

`sync` only adds and updates; it never deletes. A skill dropped from the catalog
stays on the machine until you remove it yourself.

## Writing your own

Create the directory and the file:

```bash
mkdir -p ~/.myra/skills/my-skill
$EDITOR ~/.myra/skills/my-skill/SKILL.md
```

`myra doctor` reports any `SKILL.md` that failed to load.

To share one with a team, add it in the MyraRouter dashboard under **Skills**
and publish it — it then shows up in `myra skills list` for everyone signed in
to that gateway.
