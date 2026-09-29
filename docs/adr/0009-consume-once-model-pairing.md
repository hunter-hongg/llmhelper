# A logged model is consumed by the response that uses it

The `llmhelper` Source prices a response with the model in the response
body; when the provider does not echo it, the fallback is the model the
request asked for. Spec 0027 implemented that fallback as *most recent
request line*: a `request` line overwrites a remembered model, and any
subsequent `response` line without an echo uses whatever is remembered.
That rule is correct for the log shape it was written against — strictly
one request, then one response — and for that shape it remains byte-
identical after this change. It is wrong for the log shape the format
actually permits: the per-day log is an append-only file, and nothing
prevents two requests from interleaving in it. Under interleaving, the
remembered model is *another request's* model, and a wrong model is not a
cosmetic error — it selects the `[price.<model>]` row, so the response is
priced at the wrong rate, and a budget on the true model silently measures
a different spend.

The decision: the request model is **consumed** by the response that uses
it. `pending_request_model` is set by a `request` line, and a `response`
line takes it out with `.take()` — used as the fallback when the echo is
absent, dropped when the echo is present. A response with no echo and no
unconsumed request gets an empty Model, which finds no price, which is
`cost: None`, which is `not measured` for a budget. The tool's stated
preference (ADR 0001, spec 0027) is that an unmarked value is "not over"
and that absent data is not zero: `not measured` reported explicitly beats
a wrong figure reported confidently.

The alternatives were rejected on the merits, not parked:

- **A queue** (pair each response to the oldest unconsumed request) is
  strictly better on an interleaved log — `request(A) request(B) response
  response` pairs A→first and B→second, so both responses are priced, whereas
  consume-once leaves the second unpriced. That is its advantage, and it is
  not cheap. The log carries no request id, so a queue has no basis for
  assuming responses arrive in request order: if they do, it is correct; if
  they do not, it misprices both. Consume-once overwrites the earlier model
  instead, so the second response is unpriced rather than mispriced. The two
  orderings are indistinguishable to a reader of the log, and only one of the
  two outcomes is recoverable: `not measured` is re-measurable, a charge at
  another request's rate is not — the spend was already spent and the record
  is append-only. Consume-once does not fix the first response — it still
  carries the later request's model, as the test
  `an_interleaved_log_never_invents_a_second_model` asserts openly
  (`["mistral", ""]` for `request(gpt-4o) request(mistral) response
  response`). What it fixes is the second: a confidently wrong reuse becomes
  an honest `not measured`. That is the smaller of the two errors converted,
  not the larger.
- **A request id in the envelope** would make the pairs reconstructable and
  is the honest long-term fix. It is a format change for a field the
  writer has no source for today (the provider protocol does not return
  one), and spec 0029's scope is making the two fields the tool groups by
  trustworthy, not inventing correlation machinery. If interleaved logs
  ever become common enough that the lost measurements hurt, that is the
  spec to write.

The asymmetry in the consume-once rule itself is deliberate: for `request(A)
request(B) response response`, the first response takes `B` (the most
recent unconsumed request) and the second takes nothing. Keeping both
requests would be the queue; keeping `A` would be the old bug. Using the
most recent one, once, is the only reading that is both a rule and a
conservative one.
