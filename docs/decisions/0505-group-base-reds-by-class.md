# 0505: group base reds by test class and commit

Status: accepted (s512)

Context: one fix task per failing test turned one cause into five tasks, five cold builds and conflicting edits.

Decision: tests red together on one base commit and sharing a class get one task; a later red of that class and commit is pointed at the open task. Different classes, or names without a class, keep one task each. Not a gate, guard, budget or trust file.

Consequence: a joined red is not named in the task body (no edit event). The agent fixing the class is expected to run the whole class.
