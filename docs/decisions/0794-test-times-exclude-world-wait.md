# 0794: recorded test times exclude the wait for a test world

Status: accepted (pending owner approval of the protected parser change)

A test's libtest time includes up to 600 s waiting for a world slot. We subtract a
`mason-world-wait <ms> <test>` line in the parser, so the store holds run times only. Alternative
(record both) rejected: nothing would use the wait. The wait is matched by test name so
parallel tests' output interleaving does not matter.
