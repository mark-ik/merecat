# merecat

The cat that lives on the mere.

Merecat is a graph-workspace browser: an infinite canvas of nodes, panes, and
web surfaces over a semantically related content graph. It is the reference
host for [mere](https://github.com/mark-ik/mere), the library that composes
the graph substrate (chartulary, stemma, scholia), persistence (muniment,
codicil), retrieval and inference (sibylla, vates), and identity (personae)
into one lake of content.

The name is the English calque of *meerkat*: Dutch *meer* + *kat*, lake-cat.
Mere is the lake; merecat is the animal you meet at it.

## Build and run

```sh
cargo run     # the merecat window
cargo test    # unit tests
```

Merecat pulls `mere` and the genet engine family as git dependencies; a plain
`cargo build` fetches them. Headed self-drive receipts live under `scenarios/`.

## Status

Working reference host. Merecat obviated mere's former `meerkat` crate on
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

The live plan is `design_docs/2026-07-10_merecat_architecture_plan.md`; the
founding brief is `design_docs/2026-07-08_merecat_founding.md`.

## License

MIT OR Apache-2.0.
