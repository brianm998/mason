# 1106: the client of an override is read from the hello and written into its detail

Task #1144. On 2026-11-25 a pause was lifted and the record, `resume | <owner> | /resume`, could
not say whether the command line or the monitor did it, or which pause it was.

- **Where the name comes from.** The command line and the monitors both reach the API over the
  WebSocket, so neither sets a header of its own. Each already sends `Hello.client`; the daemon
  turns it into `x-mason-client` on the requests it proxies. No protocol field is added. Any
  name starting `mason-monitor` is `monitor`.
- **A note, not an identity.** A client can call itself anything (reduced to a short plain
  word). Nothing is allowed or refused by it. `mcp` is accepted like any other name; nothing
  sends it today.
- **In the detail, not a new column.** No schema change, and `attention::intervene` (protected)
  keeps its signature. The cost is that the last-lift line parses `/resume via X: lifted #...`;
  the writer and the reader are side by side in `http.rs`.
- **Only the overrides the middleware records** carry a client. Rows written elsewhere
  (answers, task edits, SSH key choices) are unchanged.
- **`attention.rs` is not touched.** `Store::remove_pauses` keeps its signature because
  `attention::resume` returns its count. The `/resume` handler calls the new `take_pauses` and
  then admits and pokes, as `attention::resume` does, so that the rows listed are exactly the
  rows deleted.
- **Pause and resume wait for their record.** Other overrides are still recorded in the
  background. If the pool refuses the job the record is lost, as before.
- **The lift banner is a warning**, so it shows as a dismissible row; an info banner would
  collapse into "1 notice" and not explain a vanished pause on the spot. It leaves the header
  at the next rebuild after ten minutes.
- A pause the pipeline lifts itself (the push-auth hold) records no intervention, as before.
