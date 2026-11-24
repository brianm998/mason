# 1055: exit scripts follow the code

The exit tests follow the daemon's ceiling (9999) for ad-hoc numbers, and checkpoint order is numeric, not textual. No assertion was weakened.
