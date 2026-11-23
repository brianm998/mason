# 0968: An owner's retry note is kept for the task and shown to the reviewer

Stage s975. `retry-note.txt` is shown once to whichever agent step runs next and deleted. A
retry from develop therefore never showed the owner's decision to review (#918, #911): the
reviewer failed the change against the original task text.

Decision:
- The one-shot note stays as it is.
- A note given by the owner (retry endpoint, CLI `--message`, a diagnosis applied by hand; not
  `auto` retries) is also appended to `owner-notes` in the task directory, with time and step.
- Every review prompt, and every develop prompt that did not just get the one-shot note, ends
  with "Owner decisions on this task", saying the notes override the task text where they conflict.
  This changes what the reviewer is told, so it needs the owner's approval.
- The notes are removed when the task lands or is canceled, or by `mason task notes <id> --clear`
  (owner only). `mason task notes <id>` and `mason task show` list them.
