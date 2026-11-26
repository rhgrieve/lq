# Upstream

`lq` is a Rust port of MIT-licensed code from https://github.com/Shopify/theme-tools:

- `lq check`: `@shopify/theme-check-common`, `theme-check-node`, `theme-graph` and `liquid-html-parser` at `main`,
  commit `af2e1a784a1f9a454beececb9a987a8271c02d57` (after the theme-check-node 4.0.2 release)
- `lq fmt`: `@shopify/prettier-plugin-liquid` 1.11.3, with `@shopify/liquid-html-parser` 2.11.0

`specs/upstream` is a copy of that commit's language-neutral specs.

The printing algorithm is ported from Prettier 3.9.10 (MIT). `data/docs` is a snapshot of
https://github.com/Shopify/theme-liquid-docs (MIT).

Biome (MIT) formats embedded JS, TS and CSS. `lq` differs from upstream where listed in `DIVERGENCES.md`.

`lq` is not affiliated with or endorsed by Shopify.

Release tarballs include `THIRD_PARTY_LICENSES.md`, with the license text of everything compiled into the binary. Run
`scripts/third-party-licenses.mjs` to generate it.
