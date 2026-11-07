# Notchisland landing page

A single static page: `index.html` (inline CSS, no JavaScript, no external requests),
`demo.mp4` (1920×1080 H.264, ~24 s), `demo.gif` (small fallback) and `assets/`
(screenshots as WebP, `poster.jpg` for the video, `og.png` 1200×630 for link previews).

Preview locally:

```sh
cd site && python3 -m http.server 8000   # then open http://localhost:8000
```

## Publishing with GitHub Pages

GitHub Pages' "Deploy from a branch" mode only offers the repository root (`/`) or
`/docs` as the folder, so `/site` can't be picked there directly. Two options:

### Option A — GitHub Actions (keeps the `site/` folder)

1. Add this file as `.github/workflows/pages.yml`:

   ```yaml
   name: Deploy site to Pages

   on:
     push:
       branches: [main]
       paths: ["site/**"]
     workflow_dispatch:

   permissions:
     contents: read
     pages: write
     id-token: write

   concurrency:
     group: pages
     cancel-in-progress: true

   jobs:
     deploy:
       runs-on: ubuntu-latest
       environment:
         name: github-pages
         url: ${{ steps.deployment.outputs.page_url }}
       steps:
         - uses: actions/checkout@v4
         - uses: actions/configure-pages@v5
         - uses: actions/upload-pages-artifact@v3
           with:
             path: site
         - id: deployment
           uses: actions/deploy-pages@v4
   ```

2. In the repository: **Settings → Pages → Build and deployment → Source: GitHub Actions**.
3. Push to `main` (or run the workflow by hand from the Actions tab).

### Option B — move it to `docs/`

`git mv site docs`, then **Settings → Pages → Source: Deploy from a branch →
Branch: `main`, folder: `/docs`**. (The repo already has an untracked `docs/` folder, so prefer Option A unless you merge them.)

## After the first deploy

The site will be at `https://amdjedbelgacem.github.io/Notchify/`. The canonical URL and
the `og:image` / `twitter:image` tags in `index.html` use that absolute address (link
previews need an absolute image URL). If you use a custom domain or rename the repo,
update those four URLs at the top of `index.html`.

## Regenerating media

The screenshots come from the app's self-test (`notchforge --self-test <dir>`), which
renders every view with sample data. The video is a slideshow of 8 framed screenshots
with captions, a slow zoom and crossfades:

```sh
# per slide (each a 1920×1080 PNG): upscale, slow zoom, 3.5 s at 30 fps
ffmpeg -framerate 30 -loop 1 -t 3.5 -i s0.png ... \
  -filter_complex "[0:v]scale=3840:2160,zoompan=z='1+0.035*on/105':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d=1:s=1920x1080:fps=30,format=yuv420p[v0]; ...
                   [v0][v1]xfade=transition=fade:duration=0.6:offset=2.9[x1]; ..." \
  -c:v libx264 -crf 23 -preset slow -pix_fmt yuv420p -movflags +faststart demo.mp4

# GIF fallback
ffmpeg -i demo.mp4 -vf "fps=6,scale=560:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=80:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5" demo.gif
```
