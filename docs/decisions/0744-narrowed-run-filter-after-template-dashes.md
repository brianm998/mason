# 0744: a group's binary arguments join the template's `--` section

A narrowed group such as `-p x --test it -- limits::` put its `--` before the template's flags that follow `{tests_for_change}`, so `--config` reached the test binary. `expand` now splits the group and appends its binary arguments to the template's own `--` tail (or adds one). The template is not reordered: a trailing `-- -Z …` after a group's `--` would turn into filters. No trust file changed.
