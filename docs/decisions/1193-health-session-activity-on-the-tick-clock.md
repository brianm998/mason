# 1193: Health session activity on the tick clock

Status: accepted

`run_session` stamps the end of a session's turn and its return to `open` with the `now` that `tick` was given, not the wall clock. In production they are the same. With an injected clock the wall-clock stamp left a waiting session far behind the tick, so `expire_waiting` ended it as timed out at once and its handled keys became fresh again.
