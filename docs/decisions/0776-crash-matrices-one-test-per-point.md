# 0776: crash matrices become one test per point

Decision: generate one `#[test]` per crash point with a macro, instead of chunked threads inside one test. A test is the smallest unit the shard script places; the 20-minute single tests bounded every shard plan, and the 7-thread chunks started daemons without world slots.
Consequence: new points must be added to the macro list; the list-equality tests fail otherwise. Recorded times (#778) and shard lists (#701) see new names.
