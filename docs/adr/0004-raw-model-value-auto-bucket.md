# Model grouping uses the raw recorded value, including `auto`

Claude Code frequently records the routing alias `"auto"` as the message model, not a resolved model id, and the JSONL provides no resolved id to recover. We group by the raw model string as recorded and show `auto` as-is rather than guessing a real model.

This is a deliberate deviation from the obvious path: a reasonable reader would want to "resolve" `auto` to a concrete model for cleaner grouping. We reject that because (a) the data carries no resolved id and (b) maintaining an external alias→model map would be guessing. OpenCode, by contrast, records resolved ids (e.g. `big-pickle`), so its Model grouping is accurate. `auto` is documented in the glossary as a known routing-alias bucket so future maintainers do not "fix" it.
