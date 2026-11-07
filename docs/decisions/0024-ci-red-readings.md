# 0024: Readings for the red-CI fix

- The s17.sh expectation follows the hold wording of decision 0019 (`removed assertion at <file>:<line>`). It keeps the task prefix, `WAITING FOR YOU` and the file name, so it is no looser than before.
- Compose captures the `BoxWithConstraints` width in that scope before the nested `Box`; no explicit receiver is needed.
- Landing mode and the project test command are not changed here.
