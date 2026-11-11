# 0096: A frozen base is an alert, read from the freezes table

`alerts_now` raises `frozen:<project>` (kind `landing`) for each row in `freezes`, built from the row and the fix task's state in the world, so it cannot disagree with what `form_and_start` enforces and it clears with the row. A failed or canceled fix task never lifts the freeze, so the text then says so and names `mason task retry` and `mason task force`. `attention.rs` is a trust file; the change only reads the freeze and adds an alert, and `landing.rs` is untouched.
