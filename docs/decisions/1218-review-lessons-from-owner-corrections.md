# 1218: the review proposes lines from repeated owner corrections

Status: accepted (s1228)

## Decision

Part 7 of the nightly review reads `corrections.json` and proposes one line for a correction made on two or more tasks. A proposal is a normal proposed task of category `lesson` filed through the review's existing path, so it is deduplicated against every earlier task and needs the owner's approval. The cap is 3 lessons per review (`MAX_LESSONS`), inside the review's `max_proposals`. `lesson` is optional: the six required sections are unchanged.
