# Landing page

Static HTML landing page for the-one-mcp, hosted via GitHub Pages at:

    https://michelabboud.github.io/the-one-mcp/

## Layout

```
docs-site/
  index.html    # single-page landing
  style.css     # hand-rolled CSS, no frameworks
  README.md     # this file
```

No build step. No JS frameworks. Zero dependencies. The entire site is two files.

## Local preview

```bash
python3 -m http.server -d docs-site 8000
# then open http://localhost:8000
```

## Enabling GitHub Pages

This directory ships in the repo, but GitHub Pages still needs to be toggled on once per repo. One-time setup:

1. Go to **Settings → Pages** in the GitHub repo
2. Set **Source** to **Deploy from a branch**
3. Set **Branch** to `main` and folder to `/docs-site`
4. Save and wait ~30 seconds for the first deploy

After that, every push to `main` that touches `docs-site/` will redeploy automatically.

## Updating content

- Hero tagline & features: `index.html` (the `.hero` and `.features` sections)
- Install command: two places (`#install-cmd` span in the hero + `.install-block` in the install section) — keep them in sync
- Benchmark link: `#benchmarks` section points at `benchmarks/results.md`; update the shown URL if the repo moves
- Tool count in the feature tile (currently "184+"): bump whenever the catalog grows

## Future enhancements (not shipping in v0.10.0)

- **Demo GIF** — record a short asciinema of Claude Code + the-one-mcp in action, convert with `agg`, drop into the hero section. Target under 2 MB.
- **Catalog browser** — a small client-side HTML page that loads an exported `tools.json` from `~/.the-one/catalog.db` and provides search/filter. Would live at `docs-site/catalog-browser/`.
- **Benchmark table inline** — once `benchmarks/results.md` has real numbers, embed the markdown table directly on the landing page (not just a link).
- **Custom domain** — set up DNS if the project graduates to its own domain.
