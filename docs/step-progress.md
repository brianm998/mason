# Live progress files (suites and script steps)

A step that runs a command can say how far it is while it runs. Two files, one format.

## Format

One JSON object per line, appended as the work goes on. Every field is optional:

```json
{"done": 12, "total": 40, "unit": "tests", "message": "auth suite"}
```

* `done`, `total`: whole numbers. They become the step's *reported* figures.
* `unit`: what is counted ("tests", "files"). Optional.
* `message`: the step's latest one-line "what I am doing now".

A line that is not a JSON object (or whose fields have the wrong type) is shown as the message. Blank lines are skipped. A last line without its newline counts once the command ends. The file is read as it is appended, about four times a second.

## A test suite: `progress`

A suite names the file in `.mason/mason.toml` (`progress = "target/progress.jsonl"`, relative to the worktree). MASON deletes it before each attempt. What it reports becomes that suite's sub-count; a `done` the suite reports replaces the live test count for that suite.

Without a progress file a suite still shows live test counts: for the line formats (TAP, nextest, pytest, xctest, Swift Testing, Gradle and regex) MASON counts passed, failed and skipped as output lines arrive, with the same per-line rules as the final parse. JUnit, Playwright and Cucumber are whole documents, so they show lines until the end. The live count is for progress only; the verdict is the full parse at the end.

The expected total is the test count of the last green run of this suite on this base commit (labelled *expected*), else the suite's `min_tests` (labelled *minimum*), else none. A test step with several suites has one sub-count per suite and its own count is their sum.

## A script step: `MASON_PROGRESS_FILE`

Beside `MASON_RESULT_FILE` (the JSON result written at the end), a script step gets `MASON_PROGRESS_FILE`, in the same format. A script that writes nothing still shows the lines and bytes of its output.

```sh
echo '{"done": 1, "total": 3, "message": "building"}' >> "$MASON_PROGRESS_FILE"
```
