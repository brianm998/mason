# 0446: Monitor-only conditions are not routable from the Settings API

`paused`, `debt` and `provider_hold` stay unrouted, as `notify.rs` documents. The settings API refuses them with a message saying they are monitor-only; unknown conditions keep their own refusal. Making them routable would need the router's defaults and comment changed together, and nobody asked for that.
