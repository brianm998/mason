# 1227: Monitors show Add Project's warnings

The monitors dropped `warnings` and `notes` from the onboarding answers. They now read the plan's
`warnings` (review screen, above Add) and the apply's `warnings` and `notes` (result screen, apart
from the progress log). Warnings the setup only wrote to progress (`warning: ...` lines, including
the `true` test command) are also returned in `warnings`. No trust file was changed.
