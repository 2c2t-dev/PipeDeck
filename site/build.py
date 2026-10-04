#!/usr/bin/env python3
"""Builds the website into a folder: every page of pages/ in every language
of i18n/, and the site's other files as they are.

    python3 site/build.py OUT

English is at the root, the other languages in a folder of their own
(/fr/, /de/…). A page's text is {{key}} in its template, looked up in the
language's file; a key missing from one language stops the build, so no
page goes out with a hole in it.
"""

import json
import re
import shutil
import sys
from pathlib import Path

SITE = Path(__file__).resolve().parent
ORIGIN = "https://pipedeck.2c2t.dev"
# The order of the language menu; the first is the root's.
LANGUAGES = ["en", "fr", "de", "es", "it"]
# Not copied as they are: what the pages are made from.
SOURCES = {"pages", "i18n", "build.py"}


def prefix(lang):
    return "" if lang == LANGUAGES[0] else f"{lang}/"


def address(lang, page):
    """Where a page in a language is, from the root. Cloudflare Pages serves
    a page without its .html, and redirects there from the name with it."""
    name = "" if page == "index.html" else page.removesuffix(".html")
    return "/" + prefix(lang) + name


def render(template, strings, page):
    def fill(match):
        key = match.group(1)
        if key not in strings:
            raise SystemExit(f"{page}: no text for {{{{{key}}}}} in {strings['lang']}")
        return strings[key]

    # Twice, so a text may itself hold {{home}} and the like.
    return re.sub(r"\{\{(\w+)\}\}", fill, re.sub(r"\{\{(\w+)\}\}", fill, template))


def main():
    out = Path(sys.argv[1]).resolve()
    out.mkdir(parents=True, exist_ok=True)
    for item in SITE.iterdir():
        if item.name in SOURCES or item.name.startswith("."):
            continue
        target = out / item.name
        if item.is_dir():
            shutil.copytree(item, target, dirs_exist_ok=True)
        else:
            shutil.copy2(item, target)

    texts = {lang: json.loads((SITE / "i18n" / f"{lang}.json").read_text()) for lang in LANGUAGES}
    pages = sorted(p.name for p in (SITE / "pages").glob("*.html"))
    urls = []
    for page in pages:
        template = (SITE / "pages" / page).read_text()
        for lang in LANGUAGES:
            strings = dict(texts[lang])
            strings["lang"] = lang
            strings["home"] = address(lang, "index.html")
            strings["url"] = ORIGIN + address(lang, page)
            strings["alternates"] = "\n".join(
                [
                    f'<link rel="alternate" hreflang="{other}" href="{ORIGIN}{address(other, page)}">'
                    for other in LANGUAGES
                ]
                + [f'<link rel="alternate" hreflang="x-default" href="{ORIGIN}{address(LANGUAGES[0], page)}">']
            )
            strings["languages"] = "\n".join(
                f'<li><a href="{address(other, page)}" hreflang="{other}" lang="{other}"'
                + (' aria-current="page"' if other == lang else "")
                + f'>{texts[other]["language_name"]}</a></li>'
                for other in LANGUAGES
            )
            target = out / prefix(lang) / page
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(render(template, strings, page))
            urls.append(ORIGIN + address(lang, page))

    (out / "sitemap.xml").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        + "".join(f"  <url><loc>{url}</loc></url>\n" for url in urls)
        + "</urlset>\n"
    )
    print(f"{len(urls)} pages in {', '.join(LANGUAGES)} written to {out}")


if __name__ == "__main__":
    main()
