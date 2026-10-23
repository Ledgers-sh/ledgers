# Demo logos

These are the Factory Zero venture marks, copied from the Factory Zero website
repository (`apps/site/assets/logos`, and `packages/ui/assets/favicon.svg` for
the Factory Zero mark). They are Factory Zero's own marks, used here only for the
Factory Zero demo book: the business units, the two companies and the group.

- `*.svg` are the vendored sources, unchanged.
- `*.png` are 512 x 512 transparent renders of each SVG, made in Chromium (the
  browser the app runs in). The Worker stores only the PNGs: SVG is never stored
  or served, because an SVG can carry script. `crates/ledgers-server` embeds the
  PNGs with `include_bytes!`.

To re-render after changing an SVG, load each file in a page at 512 x 512 and
screenshot it with a transparent background.
