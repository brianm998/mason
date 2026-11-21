<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0768: a test that needs a batch of two forms it on admission

A test that needs two tasks in one batch sets `max_size = 2` and a long window, so the batch forms when the second task is admitted. It never relies on a short window timer, because a busy machine can outlast it and split the pair.
