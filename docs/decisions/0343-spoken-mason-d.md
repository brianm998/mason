# 0343: Speak "mason D", write `masond`

Decision: spoken sentences spell the daemon name "mason D" because the speech voice mangles "masond". Written text (banners, logs, CLI) keeps `masond`. Done by changing the three strings, pinned by a test in each client; no shared helper.
