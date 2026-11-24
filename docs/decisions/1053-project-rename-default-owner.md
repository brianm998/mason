# 1053: project rename defaults the caller to USER

Decision: when MASON_AS is unset, `mason project rename` sets it from USER, as `mason remote` does.
The daemon still refuses any name that is not the owner (403).
