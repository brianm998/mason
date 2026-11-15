# 0419: pay every open debt when its task lands

Skipped checks used to be paid back automatically only when marked deferred. Bulk skips were not, so debts piled up and defects hid until the single develop run. Now landing pays all of the task's open debts. Skipping stays allowed; the cost is one extra suite run per skipped task at the base.
