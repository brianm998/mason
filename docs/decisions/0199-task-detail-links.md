# 0199: Issue and CI references on the task detail page are links, chosen by the clients from one function

Task numbers and CI references were plain text. The owner wanted `#N` to open that task and every CI reference to open in the browser.

**Decision.** The daemon records the pages it knows (the batch pull request, each failed check) per task and sends them as `TaskDetailView.ci_links` (`label`, `url`, `kind`), http(s) only, with the label equal to the text the notes use. Each client has one pure `Linkify` function (tested alike) that turns text into plain, task and URL segments. `#N` is a task only if the model knows N and the text before it is not `PR`, `pull request`, `Batch`, `debt`, `issue` or `run`; a CI reference is a link only when a URL is known, and plain text otherwise. An unknown `#N` stays plain.

**Why not link in the daemon.** The notes are stored prose read by the CLI and the agents; markup in them would leak. A label the client can match leaves the stored text unchanged.

**Trust.** Edits `landing.rs` (protected) to record links, and `clients/features.toml` (protected) for the feature entry; neither changes a verdict, a ladder step or a gate. The fake host returns jobs only when asked (a `with-jobs` file), so landing tests that expect no per-job judgement are unchanged.
