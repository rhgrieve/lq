# Upstream specs

Shopify/theme-tools' language-neutral specs, copied unchanged from `specs/` at
af2e1a784a1f9a454beececb9a987a8271c02d57, the commit `lq check` targets. They are MIT
licensed: see [LICENSE.md](LICENSE.md).

| Specs | Run by |
|---|---|
| `checks/` | `cargo test -p lq-check --test upstream_specs`, in `--upstream-bugs` mode |
| `graph/` | `cargo test -p lq-check --test graph_specs` |
| `parser/` | `node scripts/parity/specs-parser.mjs specs/upstream`, against `lq debug-ast --parser check` |

To re-pin, copy `specs/` from the new commit over these directories.
