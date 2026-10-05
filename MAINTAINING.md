# Maintaining Pipedeck

For the maintainers: making a release, and keeping the website and the
pictures up to date.

## Making a release

1. Set `version` in the root `Cargo.toml`, and move the changelog's
   *Unreleased* section under the new version.
2. Commit, and tag that commit: `git tag -a v0.2.0 -m "Pipedeck 0.2.0"`.
3. Push the commit, then the tag: `git push origin main v0.2.0`.

The tag builds the .deb, the .rpm and the AppImage, and puts them on a
release whose notes are that version's section of the changelog. The
workflow stops if the tag and `Cargo.toml` disagree.

To try the packages before tagging, start the *Packages* workflow by hand
from the Actions tab: it builds all three and keeps them on the run, with no
release. `packaging/package.sh deb|rpm|appimage` builds one into `dist` on
your own machine, on the system it is for: Debian 13 for the .deb and the
AppImage, Fedora 42 for the .rpm.

## The website

`site/` is [pipedeck.2c2t.dev](https://pipedeck.2c2t.dev/). The *Site*
workflow publishes it on Cloudflare Pages, with the icon and the pictures in
`.github/`, whenever one of them changes on `main`; it also draws WebP
copies of the pictures at the sizes the page shows them. The headers it is
served with, its Content Security Policy among them, are in
`site/_headers`. The typefaces are the site's own, in `site/fonts/`, so it
loads nothing from another server.

The pages are made by `site/build.py` from a template in `site/pages/`
and a file of texts per language in `site/i18n/`: English at the root,
French, German, Spanish and Italian under `/fr/`, `/de/`, `/es/` and
`/it/`. A change of wording goes in every language's file; a text missing
from one is shown in English, and the build lists it. To add a language, copy `en.json`, translate
it, and add its code to `LANGUAGES` in `build.py`. To look at the site
before pushing, build it with `python3 site/build.py _site`.

The legal notice, `site/pages/legal.html`, served at `/legal`, names the
host and the contact address,
contact@2c2t.dev; it changes with either.

The workflow needs two repository secrets, `CLOUDFLARE_API_TOKEN`, a token
allowed to edit Cloudflare Pages, and `CLOUDFLARE_ACCOUNT_ID`. Without them
it publishes nothing.

## Code analysis

The *Sonar* workflow analyses the repository on 2c2t's SonarQube Server
after every push to `main`: the Rust, with Clippy's findings and the
coverage of the tests and of the smoke test, run against a PipeWire with
no sound card in the workflow's container; the plugins, the website and
the workflows. It needs the
`SONAR_TOKEN` and `SONAR_HOST_URL` secrets; the project's key is in
`sonar-project.properties`.

## The pictures

The README and the website show the mixer and the equaliser, drawn from a
made-up mixer that leaves yours alone. To draw them again after the window
changes, on GTK's Broadway backend so nothing shows on screen:

```sh
gtk4-broadwayd :5 &
export GDK_BACKEND=broadway BROADWAY_DISPLAY=:5
cargo run -p pipedeck --example screenshot .github
cargo run -p pipedeck --example screenshot .github --light
```

`.github/social-preview.png` is the picture GitHub shows when the repository
is linked; it is uploaded by hand in the repository's settings, under
*Social preview*.
