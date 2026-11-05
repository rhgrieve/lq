# Upstream

`lq` is a Rust port of MIT-licensed code from https://github.com/Shopify/theme-tools, pinned at tag
`@shopify/theme-check-node@3.31.0`:

- `@shopify/liquid-html-parser` 2.11.0
- `@shopify/theme-check-common`, `theme-check-node`, `theme-graph` 3.31.0
- `@shopify/prettier-plugin-liquid` 1.11.3

The printing algorithm is ported from Prettier 3.9.10 (MIT). `data/docs` is a snapshot of
https://github.com/Shopify/theme-liquid-docs (MIT).

Biome (MIT) formats embedded JS, TS and CSS. `lq` differs from upstream where listed in `DIVERGENCES.md`.

`lq` is not affiliated with or endorsed by Shopify.

Release tarballs include `THIRD_PARTY_LICENSES.md`, with the license text of everything compiled into the binary. Run
`scripts/third-party-licenses.mjs` to generate it.
