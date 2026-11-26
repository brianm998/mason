# 1168: Later review rounds answer earlier findings

Decision: the daemon, not the model, enforces the round-2+ rules in a validator on `submit_review`, because it alone knows the last reviewed head. A new blocking finding must name a file and line changed since that head, or be of kind `wont_build` or `wrong_for_every_input`; otherwise the reviewer resubmits it as minor. An earlier blocking finding is open until a later round answers it `fixed` or `withdrawn`.
