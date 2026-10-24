# Documentation

After every relevant change (new features, refactors, new files, behaviour
changes) update the documentation without waiting to be asked, as part of
the task. The rule is where to write it, not whether.

| What you wrote | Where it goes |
|---|---|
| An invariant that holds across the whole repo, whose violation is silent | root `CLAUDE.md`: a short entry with a pointer to the detail |
| How a subsystem works, and the traps for whoever touches it | the `CLAUDE.md` or the file under `docs/` in the directory that holds the code of that subsystem |
| Why a choice went the way it did: a rejected alternative, an incident, behaviour of an external system found empirically | a new file in `docs/decisions/`, `YYYY-MM-DD-slug.md`, verdict in the first paragraph |
| A change that affects the end user (setup, usage, structure) | `README.md` |

Hygiene:

- Adding is not the only operation. If a change makes a paragraph obsolete,
  rewrite or remove it: do not add a new one that contradicts it. If a design
  is replaced, its story moves to `docs/decisions/`; only what holds today
  stays in the living file.
- An entry in the root `CLAUDE.md` is the rule, how it fails, and a pointer:
  three or four lines, never the mechanism. Budget: about 400 lines, as a
  tripwire, not a target. Do not restate in an area's `CLAUDE.md` a rule the
  root already states: both are loaded together.
- An area's `CLAUDE.md` is an index over its `docs/`: what applies across the
  area, and a table routing to the file that covers what is being changed.
- Do not use `@path/file.md` imports: they are expanded at startup and save
  no context. Cite the path in plain prose.
