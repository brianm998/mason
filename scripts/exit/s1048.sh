#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1048 exit test: a diagnosis ends in a decision card of 2 to 4 options with one recommended;
# the card is stored on the row; one diagnosis per failure and six per task; the owner can ask for
# one on a failed task. The daemon test uses the fake adapter: it fails a task, asks for a
# diagnosis, checks the row's card, fails the task again and checks that a new row appears.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-adapter-api --lib -- a_diagnosis_is_a_card_of_two_to_four_options
cargo test -q -p mason-daemon --lib -- diagnose::tests
cargo test -q -p mason-daemon --test it diagnose_retry::
echo "s1048 OK"
