# 0752: list the change's files in the run snapshot

Decision: MASON writes `.mason/changed-files.txt` into the remote snapshot; the exit-script helpers read it when present. Rejected: `fetch-depth: 0` (slower every run, protected workflow change). Not verified on GitHub yet.
