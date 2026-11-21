# 0774: the idle debt pass picks suites, backs off after a red, and takes proof from CI

Decision: debts are settled by suite proof, so the pass runs each needed suite once per tip, holds a red suite until a landing touches its red modules or 24 h pass, and accepts a develop CI run as proof when all of a suite's mapped `ci_jobs` succeeded on it. Rejected: keying runs by debt scope (the same suites ran under three names), clearing a red by time alone (a fix would wait a day), and reading the conclusion of a whole CI run (an unrelated red job would hide a green suite).
