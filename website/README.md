# ManojHustler

Apps I made for myself, free for everyone. A plain static site: no build step. Upload the folder to GitHub Pages as-is.

## Turn on the Support button

Open `data/site.json` and fill in any of these under `support`. Blank ones are hidden.

| Field | What to put | Good for |
|---|---|---|
| `upiId` | Your UPI ID, e.g. `manoj@okaxis` | Visitors from India. Shows a QR code and a Copy button (plus "Open UPI app" on phones). No fees. |
| `kofi` | `https://ko-fi.com/yourname` | Anyone abroad. Card or PayPal, no account needed. 0% platform fee on one-time tips. |
| `githubSponsors` | `https://github.com/sponsors/manoj7019` | Developers. 0% fee, monthly or one-time. Supporters need a GitHub account. |
| `patreon` | `https://patreon.com/yourname` | Only if you want monthly memberships with perks. Takes 8–12% plus processing fees. |

## Turn on reviews

Reviews use giscus (GitHub Discussions). Enable Discussions on the repo, make a "Reviews" category, install the giscus app, then copy `repoId` and `categoryId` from https://giscus.app into `data/site.json`. Until then the Reviews section stays hidden.

## Add a new app

1. Add a 256×256 icon to `assets/`.
2. Add an entry to `data/apps.json` (and the same card to `index.html` so it shows up without JavaScript too).
3. Copy `cutdeck/` to a new folder named after the app's `id` and edit the text, colours (`body[data-page="…"]` in `css/site.css`) and download links.
4. Give the app its own release tag prefix (like `bobplayer-v` or `readalong-v`). Download buttons use `data-release-prefix` + `data-release-asset` to point at the newest matching GitHub Release automatically; the `href` is only the fallback.
5. Add the page to `sitemap.xml`.

## Rename the site

The name "ManojHustler" appears in the `<title>`s, the header of each page, and `siteName` in `data/site.json`.
