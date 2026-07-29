# turnstone

Turnstone is a local-first browser for shared, addressable places where people
talk, author, publish, and bring other webs into view. Its canvas relates
people, conversations, shared places, documents, services, and remote
resources in one durable graph. HTTP and the smolweb protocols are resource
and exchange lanes within that peer web rather than the product's boundary.

Turnstone is the reference host for
[mere](https://github.com/mark-ik/mere), the offline graph library and
repository home of the peer-domain packages it composes. Mere supplies graph
truth and portable projections; Murm supplies invited exchange; Gemot governs
durable moots; the Commons profile supplies shared graph and channel
convergence; Knot owns authored documents; and Genet presents local and remote
content.

The name is the English calque of *meerkat*: Dutch *meer* + *kat*, lake-cat.
Mere is the lake; turnstone is the animal you meet at it.

## Build and run

```sh
cargo run     # the turnstone window
cargo test    # unit tests
```

Turnstone pulls `mere` and the genet engine family as git dependencies; a plain
`cargo build` fetches them. Headed self-drive receipts live under `scenarios/`.

## Status

Working reference host. Turnstone obviated mere's former `meerkat` crate on
2026-07-18: the behavioral deletion matrix went green and meerkat left mere's
tree, so the browser host now lives here as its own binary over the `mere`
library.

What runs today: the graph canvas (pan / zoom / isometric, deterministic
layout strategies); a summonable omnibar (find / go / actions lanes);
back, forward, reload; live web content on two engine lanes (the genet stylo
lane and the clean-room `genet.livery` lane) with a per-node viewer override;
retargeting panes (Roster, Trail, Gloss, Inspector, Apparatus) and a
platen-tiled Workbench; multi-window lenses with identity-preserving pane and
tile tear-out; and multi-session (`sessions/<id>/` with a switcher and
restart restore). Every capability carries a self-driving scenario receipt
(the shared genet-probe driver) plus an accessibility projection.

The peer-web product spine is not yet wired end to end. Personae identity,
Murm, Gemot, shared graph/chat convergence, Knot communal sync, and Retinue
carriage have separate executable receipts; Turnstone still lacks the live
place port that composes them. The bounded integration plan is the
[peer-web reframe](design_docs/2026-07-28_turnstone_peer_web_reframe.md).

The live plan is `design_docs/2026-07-10_turnstone_architecture_plan.md`; the
founding brief is `design_docs/2026-07-08_turnstone_founding.md`.

## Screenshots

<p align="center">
  <img src="assets/screenshots/gloss-minimap.png" alt="Turnstone graph canvas beside its Gloss minimap" width="900"><br>
  <sub>Gloss keeps the graph's wider shape visible while the canvas stays focused on the current node.</sub>
</p>

<p align="center">
  <img src="assets/screenshots/workbench-split.png" alt="Turnstone Workbench with two graph nodes rendered as tiled web documents" width="900"><br>
  <sub>The Workbench tiles graph nodes into live document surfaces beside the canvas.</sub>
</p>

## Graphshell endpoint

Turnstone exposes its first local Graphshell projection through the library's
`remote_projection` adapter. Mere cartography maps the live graph into a
Scenograph spiral and routed relations; Graphshell resolves separately
transferred cards and returns advertised intents through Servitor. The G3
receipt and exact acceptance boundary are recorded in
`docs/2026-07-22_g3_graphshell_endpoint_receipt.md`.

## License

MIT OR Apache-2.0.
