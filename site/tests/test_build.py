"""The website's build: the real site built into a scratch folder, and a
copy of it with texts taken out, to see what the build makes of that."""

import io
import json
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

SITE = Path(__file__).resolve().parent.parent
LANGUAGES = ["en", "fr", "de", "es", "it"]


def build(site: Path, out: Path) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(site / "build.py"), str(out)],
        capture_output=True,
        text=True,
        check=False,
    )


class TheSite(unittest.TestCase):
    """The real site, built in this process, where its coverage is read."""

    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory()
        cls.out = Path(cls.scratch.name)
        sys.path.insert(0, str(SITE))
        import build as site_build

        with mock.patch.object(sys, "argv", ["build.py", str(cls.out)]), redirect_stdout(io.StringIO()) as said:
            site_build.main()
        cls.said = said.getvalue()

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    def test_it_builds_every_page_in_every_language(self):
        self.assertIn("10 pages", self.said)
        self.assertNotIn("for want of a translation", self.said)
        for lang in LANGUAGES:
            folder = self.out if lang == "en" else self.out / lang
            for page in ("index.html", "legal.html"):
                html = (folder / page).read_text()
                self.assertIn(f'<html lang="{lang}">', html)
                self.assertNotIn("{{", html, f"{lang}/{page} has a text left out")

    def test_each_page_names_its_other_languages(self):
        html = (self.out / "fr" / "legal.html").read_text()
        for lang in LANGUAGES:
            path = "" if lang == "en" else f"{lang}/"
            self.assertIn(f'hreflang="{lang}" href="https://pipedeck.2c2t.dev/{path}legal"', html)
        self.assertIn('hreflang="x-default" href="https://pipedeck.2c2t.dev/legal"', html)
        self.assertIn('<link rel="canonical" href="https://pipedeck.2c2t.dev/fr/legal">', html)

    def test_the_sitemap_lists_every_page(self):
        sitemap = (self.out / "sitemap.xml").read_text()
        self.assertEqual(len(re.findall("<loc>", sitemap)), 2 * len(LANGUAGES))
        self.assertIn("<loc>https://pipedeck.2c2t.dev/de/</loc>", sitemap)

    def test_the_structured_data_is_json_in_the_pages_language(self):
        html = (self.out / "es" / "index.html").read_text()
        block = re.search(r'<script type="application/ld\+json">\s*(.*?)\s*</script>', html, re.S).group(1)
        self.assertNotIn("</", block)
        app = json.loads(block)["@graph"][0]
        self.assertEqual((app["@type"], app["inLanguage"], app["url"]), ("SoftwareApplication", "es", "https://pipedeck.2c2t.dev/es/"))

    def test_the_sites_own_files_come_as_they_are(self):
        for name in ("style.css", "script.js", "_headers", "robots.txt", "404.html", "fonts/inter-latin.woff2"):
            self.assertTrue((self.out / name).is_file(), name)
        for source in ("build.py", "i18n", "pages", "tests"):
            self.assertFalse((self.out / source).exists(), f"{source} is not published")


class MissingTexts(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.site = Path(self.scratch.name) / "site"
        shutil.copytree(SITE, self.site, ignore=shutil.ignore_patterns("tests", "__pycache__"))

    def tearDown(self):
        self.scratch.cleanup()

    def drop(self, lang: str, key: str):
        path = self.site / "i18n" / f"{lang}.json"
        texts = json.loads(path.read_text())
        del texts[key]
        path.write_text(json.dumps(texts))

    def test_one_missing_from_a_language_is_shown_in_english_and_listed(self):
        self.drop("it", "download")
        done = build(self.site, Path(self.scratch.name) / "out")
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("it: in English for want of a translation: download", done.stdout)
        english = json.loads((self.site / "i18n" / "en.json").read_text())["download"]
        self.assertIn(english, (Path(self.scratch.name) / "out" / "it" / "index.html").read_text())

    def test_one_missing_from_english_stops_the_build(self):
        self.drop("en", "download")
        done = build(self.site, Path(self.scratch.name) / "out")
        self.assertNotEqual(done.returncode, 0)
        self.assertIn("no text for {{download}}", done.stderr)


if __name__ == "__main__":
    unittest.main()
