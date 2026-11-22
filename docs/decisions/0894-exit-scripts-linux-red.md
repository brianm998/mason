# 0894: exit scripts red on Linux remote runs

- **s592**: already passes on develop; run-exit-scripts.sh has the weighted shard code. The failing run used an older script. No change.
- **s452 / s09 section 2**: `steps_of` matched every `"name":` key of `task show --json`, so a non-step name (`checks`) came first. It now reads only the `steps` array.
- **s748 / s750**: `mason hold` (crates/cli/src/resources.rs) exits on stdin EOF, a read error, or pid exit. Its stderr was not captured. The scripts now save it with the exit status and print both, plus `ps` of the holder, in fail(). The line-162 check is unchanged. The Linux cause is not established; the next red run will show it.
- **s13**: not investigated here (needs the Linux runner's Compose dump). Cause unknown; compare() still reports extra rows and was not loosened.
- **s77**: no log for run 44814043622 exists under /Volumes/sp/mason/mason/t/729. Unexplained until that log is found.
