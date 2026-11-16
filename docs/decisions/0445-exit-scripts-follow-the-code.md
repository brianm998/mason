# 0445: an exit script follows what the code does now, and says why it fails

Seven scripts went red because later work (s312 worktree setup, s292 parsers, s426 debts, decision 0225's ledger fix tasks) changed what they exercised, not because that work was wrong. Each was reproduced and read against the code first; the code was left alone and the script or test was pointed at the new behaviour with its intent kept (s54: a landing still releases the chain in order; s19: a parser plugin at protocol 1 is refused; s393: a debt closes by a green at a base holding the task; s396: only the touched crates ran). Nothing was weakened that tested the point: the s393 test accepts two wordings of the proof of the same fact, and the s396 test compares the set of crates instead of the count of runs.

A script that can end through `set -e` carries an `ERR` trap naming the line and command, so a red exit script always says why.
