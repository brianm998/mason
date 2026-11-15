# 0417: Start a command host again when it did not come up

A command that never ran was judged nothing, yet a timed-out host start failed the step and a retry hit the same condition. We retry the start in place (3 tries, 30 s apart) and record each as a host loss. Rejected: raising the limit again (already 60 min).
