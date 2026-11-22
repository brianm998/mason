# 0938: A refused landing configuration is not an outside failure

Stage s945. Landing's "landing is not configured" verdict was classed Outside and retried. The
configuration does not change between retries, so it is now classed Code and the task is sent back
with the reason. Other `landing failed:` reasons (push, fetch, auth, CI) stay Outside.
