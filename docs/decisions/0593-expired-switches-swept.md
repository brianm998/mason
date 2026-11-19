# 0593: expired overrides are swept, quarantines stay

The sweep deletes expired overrides each pass and files no bug. Expired quarantine rows stay as ledger history (`Store::ledger()`), and the `expired_switches` detector reports overrides only. The finding and metric names are unchanged.
