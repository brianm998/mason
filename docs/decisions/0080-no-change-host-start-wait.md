# 0080: Task #74 needs no code change

The four identical failures came from the pre-536a832 start deadline. That commit already waits out a frozen launch (60 min) and kills a host it gives up on, so a second change would only duplicate it. The detector's counted attempts are historical; if the signature recurs with a "within 3600s" message, the cause is a freeze longer than an hour and needs a new ticket.
