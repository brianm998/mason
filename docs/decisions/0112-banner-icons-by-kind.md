# 0112: Banner icons are chosen by kind, and a frozen landing gets a kind

**Decision.** Banner icons come only from the banner's identity: the client's own banner ids, or the daemon's `Banner.kind`. The mapping is data in each client (`BannerIcons`), tested against the full list of kinds the daemon emits. An unknown kind shows text only.

**Frozen landing.** It had no banner at all (only `LandingView.frozen` on the landing page), so the header now adds a `landing_frozen` banner per freeze in scope. Clients and conformance see one more banner item while a freeze is active.

**No icon, on purpose.** `provider_extra_billing` and `provider_warning` are notes rather than holds; the ticket's mapping lists only the five hold kinds for the provider icon, so these two show text alone. Giving them the provider icon later is a one-line change in each client.

**Trust.** `clients/features.toml` is protected; only a feature entry was added.

**Not verified:** I built and ran nothing directly; see the stage note.
