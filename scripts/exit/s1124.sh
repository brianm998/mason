#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1124 exit test: the live list of what runs where (decision 1113).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1124.md
# A scratch daemon and a node on loopback: a task's check runs on the node, `mason hosts` lists
# the task under node:b with its pipeline state and `mason task show` prints a `running on:` line
# naming the node; the node gives it back and it is listed here only; once it has ended both are
# empty. A GitHub run is listed from its hold.
cargo test -q -p mason-daemon --test nodes a_check_in_flight_is_listed_under_its_host
# The registry's guard, the host list and its project filter, and what a host id says.
cargo test -q -p mason-daemon --lib running::
# The home view names the node and keeps every host under a project filter.
cargo test -q -p mason-daemon --test scheduling an_agent_whose_check_is_on_a_node
# The protocol carries the hosts and an older client skips them; the command line's words.
cargo test -q -p mason-protocol hosts_round_trip
cargo test -q -p mason-pipeline --bin mason hosts::
echo "OK s1124"
