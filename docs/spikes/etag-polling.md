# Spike: ETag polling

Status: measured (live GitHub API, read-only, against brianm998/mason; authenticated)

## Question
Do conditional GitHub requests behave as Part 3 says, including rate-limit accounting?

## Method
`spikes/etag-polling/probe.sh` reads `repos/<repo>/issues`, records the ETag, repeats the request 5 times with `If-None-Match`, and reads `/rate_limit` (itself free) before and after. It also requests a nonexistent issue.

## Result
```
etag: "7f24cd03bbceb2d9b09b26c5693ac98c8513692220ae5ea0911d30bb835a4ceb"
HTTP/2.0 304 Not Modified
HTTP/2.0 304 Not Modified
HTTP/2.0 304 Not Modified
HTTP/2.0 304 Not Modified
HTTP/2.0 304 Not Modified
core remaining: start=4796 after-unconditional=4795 after-5-conditional=4791
missing resource:
HTTP/2.0 404 Not Found
poll-interval header: 
```
- One unconditional request cost 1 core request (4821 to 4820). Five conditional requests returned `304 Not Modified` and cost 0.
- A missing resource gives a clean `404`, which is the permanent-error class Part 3 says to stop polling on.
- No `X-Poll-Interval` header came back for this endpoint, so backoff has to be our own.
- `gh api` exits non-zero on `304`. Production code will use HTTP directly and treat 304 as success.

## Conclusion
Conditional polling works as Part 3 describes and 304s are free against the primary limit, so polling only items something is waiting on, with our own backoff, is affordable. Secondary (abuse) limits were not exercised. No fallback needed.
