# `request` reads usage data to decide whether to send; it still does not write it

Spec 0008's Further Notes set one load-bearing constraint on the request
command: "Keep `request` read-only with respect to local usage data; it does
not write into Source DBs." Every `request` feature since has held that line —
reasoning capture, request logging, split exit codes, the interactive TUI all
operate on the request payload and the provider's response, never on the
corpus the other commands measure.

Spec 0027 bends the letter without breaking it: the budget gate makes the
*send* decision depend on a read of the local corpus. A `[budget]` on
`opencode` is evaluated against OpenCode's SQLite database, and if it is over,
`request` exits 3 without contacting the provider. The new `llmhelper` Source
reads the request log directory — data `request` itself wrote, under spec
0012's opt-in `--log` — so a budget can also be placed on `request`'s own past
spend. Nothing is written to any Source, and nothing is written to the corpus;
the constraint as stated still holds. What changes is the *posture*: `request`
is no longer blind to usage data, and a read now has a side effect — refusing
to spend.

The alternative was a spend ledger: `request` would append its own token
counts to a dedicated local file and the reporting commands would read that.
That is rejected. It is a second source of truth for the same spend, it
duplicates the log spec 0012 already established, and most importantly it is
`request` *writing usage data* — exactly the thing 0008 forbade, and forbade
for a good reason: a measurement command's numbers must never be producible
by the command being measured, because a user who distrusts `usage` then has
no independent figure to check it against. Reading the log keeps the writer
(`--log`, already trusted to be an exact copy of what was sent) and the reader
(measurement, unchanged) as two separate commands over one append-only file.

Two consequences are accepted deliberately. First, the gate sees spend
accumulated *before* the current request, never the current request — its own
cost appears in the next invocation's gate, which is the only correct
accounting and makes the loop convergent rather than self-blocking. Second, an
unlogged request is invisible to the gate, so a budget on `llmhelper` with
`--log` off warns on stderr rather than silently implying the spend figure is
complete: the same "absent data is not zero" rule ADR 0001 established for
Claude's missing cost, applied to missing pricing and missing logging.

The reporting commands keep spec 0015's annotation-only posture — no nonzero
exit on over-budget. The gate is a separate, opt-in behavior on the one
command that spends money, where refusing is the useful action. A report that
refuses to print is still useless.
