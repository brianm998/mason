# 0655: Task numbers in the Compose monitor go through LinkedText

Any string that can name a task is shown with `LinkedText`, not `Text`. A scan test over `ui/*.kt` enforces it with a short allowlist of ids that are not tasks (debt, finding, issue, PR, batch). The sidebar wait line sits outside the row's click target so a named task opens its own page. The scan is line based by choice: it is cheap and catches the common regression.
