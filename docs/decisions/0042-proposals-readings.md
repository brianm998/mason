# 0042: Proposals and self-improvement: readings (s21)

Part 13 left these open.

- **A proposal is a task plus a side row.** The reducer and the event vocabulary do not change: a proposal is a `TaskFiled` (trust `OwnerAgent` for agents, outside for outsiders) and a `proposals` row (migration 15) holds title, body, evidence, source, severity, route, dedupe key, status and publish state. Approve, reject and edit act on the task while it is Proposed.
- **Duplicate:** normalised titles plus body tokens, Jaccard similarity at or above 0.6, against open proposals of the same project. A rejected proposal is also a duplicate, so a refused idea is not filed again.
- **Evidence:** at least 20 characters and a locator (a token with `/`, a file extension, or `:<line>`). Text without one is refused; edit applies the same rule.
- **Caps:** 3 per task and 20 per day per project. Refusals go back to the agent as tool errors and file nothing.
- **Policy defaults** (source x severity): agent normal Proposed, agent blocks Queued (the agent's own task cannot finish without it), outside always Proposed, owner Queued. Configurable.
- **Routing:** a proposal citing only paths in MASON's own repository goes to the self project; any path of the filing project sends it to that project; no path means the project.
- **Without write access** (`[self] access = "fork"`, `fork = "<remote>"`): the branch `mason/improve/<n>` goes to the owner's fork, and the update channel becomes `branch:<name>` so the daemon follows it. Taking over the build is not run by the test (`--check` only). A merged upstream PR returns the channel to `main`; a rejected or conflicting one files a "carry this patch?" proposal.
- **Publishing needs a look first.** `mason proposal publish N --preview` prints the redacted text and makes no network call; `--confirm` is refused until a preview was shown, and sends exactly that text. No client code or file is attached.
- **Redaction:** client names (any case), client path roots, and the whole path under a root (`[client-path]`), home paths, fenced code blocks, calls such as `f()` (`[code]`) and token-shaped strings. Redaction cannot recognise client code written as plain prose, which is why the owner reviews the preview.
