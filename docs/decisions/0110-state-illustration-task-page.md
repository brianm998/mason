# 0110: State illustration on the task page

The picture is chosen only by `row.state`; an id the manifest lacks draws nothing. It sits in the title row (an HStack/Row), so the rows below move only by the height it needs. The size is the token `mascot_illustration` (96) times `theme.scale`; on the phone `@ScaledMetric` stands in for the theme. Images are drawn whole (`scaledToFit` / `ContentScale.Fit`), never tinted, and are decorative. `Items.swift` / `Items.kt` are untouched. `clients/features.toml` (protected) only gains its entry; no check is weakened.
