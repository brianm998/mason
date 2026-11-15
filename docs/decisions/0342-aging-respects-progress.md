# 0342: aging respects progress

**Decision.** The rank tuple is `(not interactive, band, remaining, not aged, priority, enqueued, id)`. An aged request (waited past `order.max_wait_s`) stays in band 2 with its progress. It goes ahead of requests as far along as it, whatever their priority, oldest first, and never ahead of work further along. This revises decision 0231, which put aged requests in band 1 ahead of all non-landing work.

**Why.** The owner's rule (2026-11-14) is that work further along always goes first, so tickets near landing do not gum up behind new work, with issue priority and the rest below that. Since 0231 also made progress comparable across projects (a share of each route), most of the starvation aging was for is gone. The rest is between requests that are equally far along, and aging still settles that.

**Trust.** `crates/scheduler/**` and `scripts/exit/s349.sh` are in `PROTECTED`. The owner asked for this on 2026-11-14 ("make sure that pipeline priority is followed in all cases"). No capacity, gate or limit changed. An exclusive request still drains the machine of everything ranked behind it.
