# 0111: the mascot picture is chosen from daemon state in the model

Decision: each client's core model (`homeMascot`) picks the Home picture, in the order not connected, needs attention, something running, no tasks, otherwise all clear. Views only draw it.

Why: both clients must agree, and the model is testable without a UI. A slow or quiet task changes nothing, so slow never looks like an alarm. Unknown ids are never produced here and the loader returns nothing for them.

Consequence: the no-match search text is not in the conformance items (those files must stay unchanged); it is a constant in each core, compared by the s125 exit script.
