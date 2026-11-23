# 0956: Prove a debt's task landed from the base's merge

The idle debt pass and `settle_debts` only touch a debt whose task is proven landed. The proof was the task's branch, which is deleted after landing, so debts from before the commit was recorded at landing stayed open. Decision: when the branch is gone, read the oldest `Merge branch '<branch>'` merge in the base's history and use its second parent. A debt with no such merge is still never closed.
