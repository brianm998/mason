<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1216: a read-only validation session before a review send-back

A `changes` review costs a full develop, test and review round and rests on one reviewer who cannot run anything. A read-only session now checks each blocking finding first and may say `not_confirmed`; the prompt says that is acceptable. Unconfirmed findings become minor notes; if none is confirmed the review counts as an approval and `review_overruled` is logged. Any failure of the session keeps the reviewer's verdict. `[review] validate = false` restores the old behaviour. It cannot turn a red test green, only a send-back into an approval with notes.
