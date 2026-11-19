# 0598: exit-script grep options go before the separator

After `--` grep treats every word as an operand, so `--exclude=...` there is a file name and has no effect. s576 is fixed and s605 fails any exit script that repeats the pattern. No check was removed or weakened.
