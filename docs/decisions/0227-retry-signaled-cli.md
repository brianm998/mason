# 0227: retry a signaled CLI once

Status: accepted

A CLI killed by SIGTERM/SIGKILL gave no result and failed the step as a crash. It is now resumed once; a second signal fails the step as before.
