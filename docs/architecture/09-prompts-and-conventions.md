# Prompts and Conventions

`arbor-prompts` builds the text each Tier sees. Two things go into a prompt: a per-Tier template,
and the project's own guidance documents, called the Conventions.

## The templates

There is one template per Tier, plus one for the Scaffolder's Check proposal during `arbor init`:
`planner`, `scaffolder`, `writer` and `check_proposal`. Each is Markdown with two minijinja blocks:

- `instructions`: the role, the inputs, the output contract and the rules. It may use only the
  template's static variables, the ones that do not change during a Session.
- `inputs`: the Session inputs, such as the Request, the Plan, the Task or the Work Unit.

A template is checked when it is loaded. It must parse, use only its own variables, and render both
blocks. The `instructions` block is rendered twice with different sample inputs and must come out
identical, which proves it does not depend on the Session inputs. A project can override any
template by putting a file at `.arbor/prompts/<name>.md`. A bad override fails at startup rather
than mid-Session.

## The caching order

The parts of a prompt are ordered so the stable prefix comes first and the changing part last, which
is what lets OpenRouter cache the prefix. In order:

1. The template's `instructions`.
2. The Conventions (Planner and Scaffolder only).
3. The Session inputs.
4. The transcript, appended by the agent loop as the run goes.

The Writer's prompt has no Conventions, because the Writer sees only its Context Pack.

## Conventions

The Conventions are the project's own guidance documents, given to the Planner and Scaffolder so
their output fits the project. `Conventions::load` reads these files from the repository root, in
order, when they exist:

1. `AGENTS.md`
2. `CLAUDE.md`
3. `CONTEXT.md`
4. the README (`README.md`, `README`, `README.rst` or `README.txt`)
5. any extra files listed in `context.files` in `arbor.toml`

The whole set is capped at 4000 estimated tokens. Files are kept whole in that order until one does
not fit; that file is cut at a line boundary and the rest are named in a closing note so the model
can read them itself with `read_file`.

A Conventions file that resolves outside the repository is refused, so a cloned repository cannot
send other local files to the model. The check follows links with `canonicalize`, and it refuses
`context.files` entries that are absolute or climb out with `..`.

## Token estimation

Arbor estimates tokens without a tokenizer: one token per two and a half bytes of UTF-8, rounded up.
The estimate is deliberately simple and deterministic. It was measured against the input tokens
OpenRouter reported, where real text ran 2.5 to 2.9 bytes per token. Using the low end means the
estimate errs on the side of overcounting. The same estimate is used for the Conventions cap and for
the Writer Cap.

## The prompt type

A rendered prompt is a `Prompt` with the `instructions`, an optional Conventions block, and the
`inputs`. `Prompt::messages` turns it into the message list the client sends. `Prompt::template`
records which template it came from, so a run type can assert at start that it got the right prompt.
