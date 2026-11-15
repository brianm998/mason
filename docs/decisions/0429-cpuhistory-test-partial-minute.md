# 0429: Fix the cpuhistory test's first minute, not the rounding

Decision: the test was wrong. Its readings began at 5 s, so the first minute bucket had 11 readings and a mean of about 155, which the code rounded correctly. The loop now starts at 0 so every minute is complete. The 148..=153 range stays.

Rejected: widening the range or skipping the first bucket (hides what the test checks); changing `Bucket::point` rounding (it was correct).
