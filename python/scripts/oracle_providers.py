"""The oracle of the Bible providers (S5): what the Python providers do with recorded-shape responses, for the Rust providers to repeat.

Written by ``export_oracle.py`` into ``oracle/providers/`` (never by hand). Every response here is made up (synthetic text, no key, no real call):
the HTTP side is ``httpx.MockTransport``, the keys are a fixed fake, and the Bible text is "Synthetic ..." sentences.

Files (one JSON object per line, sorted keys):

* ``html_text.jsonl``   API.Bible verse content -> text (``BeautifulSoup(..., "html.parser").get_text(" ", strip=True)`` and the leading ``[16]`` marker removed)
* ``json_errors.jsonl`` ``httpx.Response.json()`` on a body: the value is valid, or the exact exception text
* ``usfm.jsonl``        ``LocalUsfmProvider`` over the folders ``oracle/providers/usfm/*`` and ``python/tests/fixtures/usfm/synthetic``
* ``api_bible.jsonl``, ``youversion.jsonl``, ``esv.jsonl``   one provider against canned HTTP: requests it makes, results, error texts
* ``keys.jsonl``        ``credentials._from_keys_file`` on the text of a keys file, for two names
* ``registry.jsonl``    ``ProviderRegistry._fetch`` over several providers and a real (sqlite) cache: provider, source (live / cache / live (not stored)), verses, errors

Paths of a USFM folder appear in messages and in ``provider_version_id``: the exporter writes ``{path}`` and ``{resolved}`` there, and the Rust test does the same.
"""
from __future__ import annotations

import asyncio
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

import httpx
from bs4 import BeautifulSoup

SYNTHETIC_KEY = "synthetic-key-0001-not-real"
ORIGIN = "https://oracle.test"
ENV_NAMES = {"api_bible": "VERSEVISION_ORACLE_API_BIBLE_KEY", "youversion": "VERSEVISION_ORACLE_YOUVERSION_KEY", "esv": "VERSEVISION_ORACLE_ESV_KEY"}
AUTH_HEADERS = ("api-key", "x-yvp-app-key", "authorization")


# ----------------------------------------------------------------------------------------------------------- html text
def _html_cases() -> list[str]:
    cases = [
        "", " ", "\n", "Synthetic verse.", "  Synthetic   verse  with   spaces.  ", "Line one.\nLine two.\n\nLine three.", "\tTabbed\ttext\t",
        "[16] Synthetic verse sixteen.", "[16-17] Synthetic bridged verse.", "[16]Synthetic no space.", "[16]   Synthetic many spaces.", "[ 16] not a marker", "[x16] not a marker",
        "[16] [17] two markers", "[1] ", "[16]", "[16] ", "text [16] inside", "[٣] Arabic digit marker",
        "<p>Synthetic paragraph.</p>", "<p class=\"p\"><span data-number=\"16\" class=\"v\">16</span>Synthetic verse.</p>", "<p>[16] First</p><p>[17] Second</p>",
        "<p>One</p>\n<p>Two</p>", "<p>One<br>Two</p>", "<p>One<br/>Two</p>", "<p>One<br />Two</p>", "one<b>two</b>three", "one <b>two</b> three", "<i>one</i><i>two</i>",
        "<div><p>Nested <em>emphasis <strong>strong</strong></em> end.</p></div>", "<ul><li>a</li><li>b</li></ul>", "<sup>1</sup>Synthetic<sup>2</sup>verse",
        "<span class=\"nd\">Lord</span>'s", "<p>unclosed paragraph", "<p>one<p>two", "</p>stray close", "<b>bold never closed", "text</b>after stray",
        "<p>Quote &amp; ampersand</p>", "&amp;", "&amp", "&lt;tag&gt;", "&quot;quoted&quot;", "&#39;apostrophe&#39;", "&apos;x&apos;", "&nbsp;", "a&nbsp;b", "&nbsp;lead", "trail&nbsp;",
        "&mdash;", "&ndash;", "&hellip;", "&ldquo;Q&rdquo; &lsquo;q&rsquo;", "&copy; 2024", "&copy 2024", "&copy2024", "&reg;", "&eacute;&egrave;", "&Eacute;",
        "&unknown;", "&unknown", "&unknown text", "&;", "& alone", "AT&T", "Tom & Jerry", "a &b; c", "&amp;amp;", "&AMP;", "&Amp;", "&notanentity;", "&notin;", "&not", "&notit;",
        "&a-b", "&a.b", "&a-", "&a-b;", "&x", "&1", "& 1", "&&", "&&amp;",
        "&#65;", "&#65", "&#65 x", "&#x41;", "&#X41;", "&#x41", "&#0;", "&#1;", "&#9;", "&#10;", "&#13;", "&#127;", "&#128;", "&#133;", "&#141;", "&#150;", "&#151;", "&#159;", "&#160;", "&#8212;",
        "&#xD800;", "&#xDFFF;", "&#1114111;", "&#1114112;", "&#xFFFE;", "&#xFFFF;", "&#xFDD0;", "&#99999999999999999999;", "&#12a;", "&#12a", "&#xzz;", "&#x;", "&#;", "&#", "&#a", "&#x", "&#xg",
        "&#65abc def", "&#65abc\nmore text", "&#65;&#66;&#67;", "&#x1F600;", "&#128512;", "x&#65", "&#65\n", "&#00065;", "&#x0041;", "&#x00000041;",
        "<!-- a comment -->text", "text<!-- c -->more", "<!-- unterminated comment", "<!---->x", "<!--->x", "<!-- a --!>x", "<!--a-->b<!--c-->d", "<!-- a -- b -->x",
        "<!DOCTYPE html>text", "<!doctype html><p>x</p>", "<!DOCTYPE", "<!DOCTYPE html", "<!ELEMENT x>text", "<!x>text", "<!>text", "<![CDATA[cdata text]]>after", "<![CDATA[unterminated", "before<![CDATA[in]]>after",
        "<?xml version=\"1.0\"?>text", "<?php echo 1 ?>text", "<?unterminated",
        "<script>var x = 1 < 2;</script>text", "<script>alert('x')</script>", "a<script>b</script>c", "<style>p { color: red }</style>text", "<script>unterminated script", "<script>a</scr</script>b", "<SCRIPT>A</SCRIPT>b",
        "<script type=\"text/javascript\">x</script >y", "<script/>text</script>", "<template>hidden</template>shown", "<rt>ruby</rt>base", "<rp>(</rp>base", "<ruby>base<rt>reading</rt></ruby>",
        "<textarea>area &amp; text</textarea>after", "<title>t &lt; u</title>after", "<xmp><b>raw</b></xmp>after", "<iframe>frame</iframe>after", "<noembed>x</noembed>after", "<plaintext>a<b>c</b>",
        "<noscript>no <b>script</b></noscript>after",
        "a < b", "a <b", "a <", "<", "<<", "a<3", "<3", "1 < 2 > 0", "a > b", "<a", "<a ", "<a href", "<a href=", "<a href=\"x", "<a href=\"x\"", "<a href=\"x\">link</a>", "<a href=x>y</a>",
        "<a href='x>y'>z</a>", "<a href=\"x>y\">z</a>", "<a title=\"a > b\">link</a>", "<a b=c d>e</a>", "<a b = c>e</a>", "<a b=\"1\"c=\"2\">e</a>", "<a/b>e</a>", "<a / >e</a>", "<a/>e", "<a b/>e", "<br/>", "<br>text",
        "<p\nclass=\"a\">text</p>", "<p class=\"a\"\n>text</p>", "<P CLASS=\"A\">Text</P>", "<p>Text</P>", "<p>Text</p >", "<p>Text</ p>", "</>text", "</ >text", "</3>text", "</p", "</", "</p x>text", "<p/ >text",
        "<1>text", "<1 >text", "<-x>text", "<x-y>text</x-y>", "<x:y>text</x:y>", "<ns:p>text</ns:p>", "<a\u00e9>t</a\u00e9>",
        "caf\u00e9 \u201cquoted\u201d \u2014 dash", "\u00a0nbsp\u00a0", "\u2003em\u2003space\u2003", "a\u200bb", "a\u00a0\u00a0b", "x\u2028y", "x\u0085y", "\x1fx\x1f", "\x0bx\x0c", "\ufeffBOM text",
        "<p>\u00a0</p>", "<p> </p><p>x</p>", "<p>a</p> <p>b</p>", "<p>a</p>\u00a0<p>b</p>", "a <!-- c --> b", "a<!-- c -->b",
        "<p>[16] Synthetic <span class=\"wj\">words of</span> verse &mdash; sixteen&hellip;</p>", "<p class=\"b\">\n  <span class=\"v\">16</span>  Synthetic   verse.\n</p>",
        "<p><sup class=\"v\">16</sup> Synthetic <i>italic</i> verse.</p>", "<note>footnote</note>text", "<p>x</p><p>y</p><p>z</p>" * 3, "x" * 500,
        "<a href=\"a&amp;b\">t</a>", "<a href=\"a&b\">t</a>", "<a title=\"&#65;\">t</a>", "<img src=x alt=\"hello\">after", "<input value=\"1\">after",
        "<table><tr><td>a</td><td>b</td></tr></table>", "<h1>Head</h1>Body", "<hr>x", "<p>a<hr>b</p>",
        "<![endif]-->x", "<!--[if IE]>x<![endif]-->y", "<a href=\"x\" href=\"y\">dup</a>", "<p>a</p></div></p>b",
    ]
    return cases


def html_text_oracle() -> list[dict]:
    from versevision.bibles.api_bible import _LEADING_VERSE_MARKER
    rows = []
    seen = set()
    for html in _html_cases():
        if html in seen:
            continue
        seen.add(html)
        soup = BeautifulSoup(html, "html.parser").get_text(" ", strip=True)
        rows.append({"html": html, "soup": soup, "text": _LEADING_VERSE_MARKER.sub("", soup)})
    return rows


# ----------------------------------------------------------------------------------------------------------- json errors
def _json_cases() -> list[bytes]:
    texts = [
        "", " ", "\n", "x", "<html>Bad gateway</html>", "{", "}", "[", "]", "{}", "[]", "null", "true", "false", "0", "-", "-0", "01", "1.", "1.5", "1e", "1e5", "-1.5E+3", ".5", "+1", "nul", "nulll", "tru", "'single'",
        "\"str", "\"str\"", "\"a\\nb\"", "\"a\nb\"", "\"a\tb\"", "\"\\x\"", "\"\\u12\"", "\"\\u12g4\"", "\"\\ud800\"", "\"\\ud800\\u0041\"", "\"\\ud83d\\ude00\"", "\"\\", "\"\\u", "\"\\u1234",
        "{\"a\":1}", "{\"a\":1,}", "{\"a\":1 \"b\":2}", "{\"a\" 1}", "{\"a\":}", "{\"a\"}", "{a:1}", "{'a':1}", "{\"a\":1", "{\"a\":1,", "{\"a\":[1,2}", "{\"a\":[1,2,]}", "{,}", "{\"a\":1}}", "{\"a\":1} x", "{\"a\":1}\n\n",
        "[1,2", "[1,2,", "[1 2]", "[1,,2]", "[,]", "[1,]", "[[]", "[]]", "[1,2]x", "[\"a\" \"b\"]", "[tru]", "[nul]",
        "{\"data\":[{\"id\":\"x\"}", "{\"data\": [", "{\"data\":", "{\"data\"", "{\"data\":[{\"id\":\"x\",}]}",
        "NaN", "Infinity", "-Infinity", "[NaN]", "{\"a\":NaN}", "\u00e9", "\"\u00e9\"", "{\"\u00e9\":1}", "\n\n{\"a\":\n1,\n}", "  {\"a\"  :  1  ,  }", "\r\n[1,\r\n2,\r\n", "[\n1,\n2\n3]",
        "{\"data\": []}", "{\"data\":[{\"id\":\"b\",\"abbreviation\":\"TST\"}]}", "{\"a\":\"\\u00e9\"}", "\"\\/\"", "{\"a\":\"b\":\"c\"}", "{\"a\":1,\"b\"}", "{\"a\":1,\"b\":}", "[1,2,3,", "{\"a\":{\"b\":}}",
        "{\"a\":\"\u0001\"}", "\"\u001f\"", "\"\u007f\"",
    ]
    out = [t.encode("utf-8") for t in texts]
    out += [b"\xef\xbb\xbf{\"a\":1}", b"\xef\xbb\xbf", b"\xff", b"\xff\xfe", b"{\"a\":\"\xff\"}", b"\xc3", b"\xc3(", b"abc\xe2\x82", b"\xe2\x28\xa1", b"\x80abc", b"\xf5\x80\x80\x80", b"\xc0\x80"]
    return out


def json_errors_oracle() -> list[dict]:
    rows = []
    for body in _json_cases():
        try:
            httpx.Response(200, content=body).json()
            message = None
        except ValueError as exc:
            message = str(exc)
        row: dict[str, Any] = {"message": message}
        try:
            row["body"] = body.decode("utf-8")
            if body.startswith(b"\xef\xbb\xbf"):
                row["hex"] = body.hex()
                del row["body"]
        except UnicodeDecodeError:
            row["hex"] = body.hex()
        rows.append(row)
    return rows


# ----------------------------------------------------------------------------------------------------------- keys file
def keys_oracle() -> list[dict]:
    from versevision import credentials
    contents = [
        "", "VV_A=1\n", "VV_A=1\nVV_A=2\n", " VV_A = 1 \n", "VV_A=\"quoted\"\n", "VV_A='single'\n", "VV_A=\"'mixed'\"\n", "VV_A='\"x\"'\n", "# VV_A=1\n", "VV_A=1 # trailing\n", "VV_A\n", "VV_A=\n",
        "VV_A=\"\"\n", "=1\n", "VV_A==1\n", "VV_A=b=c\n", "\ufeffVV_A=1\n", "VV_A=1\r\nVV_B=2\r\n", "VV_A=1\rVV_B=2\r", "\n\n  \nVV_A=1", "VV_A=1\x0cVV_B=2", "VV_A=1\u2028VV_B=2", "export VV_A=1\n",
        "VV_A = \"1 2\"\n", "vv_a=1\n", "VV_A=\u00e9\n", "VV_A=1\n\tVV_B=2\n", "VV_A = ' ' \n", "#c\nVV_B=2\n\nVV_A=\"k\"\n# end", "VV_A=\"unclosed\n", "VV_A=\"\"\"x\"\"\"\n", "VV_A=''\nVV_A=2\n", "VV_A=\nVV_A=2\n",
        "VV_A = \"a\" # c\n", "VV_A=\u00a01\u00a0\n", "VV_A=\u30001\u3000\n",
    ]
    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "keys.env"
        saved = os.environ.get("VERSEVISION_KEYS")
        os.environ["VERSEVISION_KEYS"] = str(path)
        try:
            for content in contents:
                path.write_bytes(content.encode("utf-8"))
                rows.append({"content": content, "values": {name: credentials._from_keys_file(name) for name in ("VV_A", "VV_B", "vv_a")}})
        finally:
            if saved is None:
                os.environ.pop("VERSEVISION_KEYS", None)
            else:
                os.environ["VERSEVISION_KEYS"] = saved
    return rows


# ----------------------------------------------------------------------------------------------------------- USFM folders
USFM_FOLDERS: dict[str, dict[str, str]] = {
    "basic": {
        "JHN.usfm": "\\id JHN\n\\c 3\n\\p\n\\v 16 Synthetic verse sixteen.\n\\q1 \\v 17 Synthetic poetry line.\n\\v 18 Synthetic verse eighteen.\n",
        "ROM.sfm": "\\id ROM Synthetic Romans\n\\h Romans\n\\c 8\n\\p\n\\v 28 Synthetic text with a note\\f + \\fr 8:28 \\ft footnote text\\f* inside, and more.\n"
                   "\\v 29-30 Bridged verse text.\n\\q2 continued poetry line\n\\v 31 After the bridge\n\\x - \\xo 8:31 \\xt Rom 1:1\\x* text.\n",
    },
    "formatting": {
        "PSA.txt": "\ufeff\\id PSA\n\\c 23\n\\q1 \\v 1 The \\nd Lord\\nd* is my \\add shepherd\\add*;\n\\q2 I shall not want.\n\\q1 \\v 2 He makes me lie down\n\\q2 beside the waters.\n\\p\n\\v 3 He restores\\fig x\\fig* my soul.\n"
                   "\\c 24\n\\p\n\\v 1 The earth is the LORD's, and \\w all|strong=\"H3605\"\\w* it contains.\n",
        "MAT.usfm": "\\id MAT\n\\c 5\n\\s1 Heading line\n\\p\n\\v 3 Blessed are the poor in spirit,\ncontinued on the next line\nand a third line.\n\\v 4 Blessed are those who mourn.\n\\q Plain q marker\n\\v 5 Meek verse\n\\qr right aligned\n\\v 6 Six\n\\pi indented paragraph\n\\v 7 Seven\n\\p text after p marker\n\\v 8 Eight\n",
        "LUK.usfm": "\\id LUK\r\n\\c 2\r\n\\p\r\n\\v 1 CRLF verse one.\r\n\\v 2 CRLF verse two\r\ncontinued.\r\n",
        "notes.md": "\\id GEN\n\\c 1\n\\v 1 Not a USFM suffix.\n",
        "README": "\\id GEN\n\\c 1\n\\v 1 No suffix at all.\n",
        "NOID.usfm": "\\c 1\n\\v 1 No id line here.\n",
        "lower.usfm": "\\id gen\n\\c 1\n\\v 1 Lowercase id is not an id.\n",
        "EXO.USFM": "\\id EXO\n\\c 1\n\\v 1 Upper case suffix.\n\\v 3 Gap before three.\n",
        "1CO.usfm": "\\id 1CO\n\\c 13\n\\p\n\\v 4 Love is patient; love is kind.\n\\v 5 It does not envy.\n\\c 14\n\\v 1 Pursue love.\n",
    },
    "dup-and-odd": {
        "A_first.usfm": "\\id JHN\n\\c 1\n\\v 1 From the first file.\n\\v 1 Replaced inside the file.\n",
        "B_second.usfm": "\\id JHN\n\\c 1\n\\v 2 From the second file; it replaces the whole book.\n",
        "REV.usfm": "\\id REV\n\\v 1 Verse before any chapter.\n\\c 0\n\\v 2 Chapter zero.\n\\c 22\n\\v 21 The last verse.\n\\v 20 Out of order.\n",
        "TIT.usfm": "\\id TIT extra words\n\\c 1\n\\v 1 Verse one \\\\ with backslashes \\ and a lone \\\n\\v 2 \\\\ only\n\\v 3\n\\v 4  \n\\v 5 five\n",
    },
    "empty": {},
}


def _write_usfm_folders(out: Path) -> None:
    base = out / "providers" / "usfm"
    if base.exists():
        import shutil
        shutil.rmtree(base)
    for name, files in USFM_FOLDERS.items():
        folder = base / name
        folder.mkdir(parents=True, exist_ok=True)
        for filename, content in files.items():
            (folder / filename).write_bytes(content.encode("utf-8"))
        if not files:
            (folder / ".gitkeep").write_bytes(b"")


# ---------------------------------------------------------------------------------------------------------- run helpers
def _span(raw: list[int]):
    from versevision.references.models import VersePoint, VerseSpan
    start = VersePoint(chapter=raw[0], verse=raw[1])
    end = VersePoint(chapter=raw[2], verse=raw[3]) if len(raw) > 2 else None
    return VerseSpan(start=start, end=end)


def _scrub(value: Any, repo_root: Path, rel_path: str | None) -> Any:
    """``{path}`` for the folder as written in the config, ``{resolved}`` for its resolved absolute path."""
    if isinstance(value, str):
        if rel_path is not None:
            resolved = str((repo_root / rel_path).resolve())
            value = value.replace(resolved, "{resolved}")
            value = value.replace(rel_path, "{path}")
        return value
    if isinstance(value, list):
        return [_scrub(v, repo_root, rel_path) for v in value]
    if isinstance(value, dict):
        return {k: _scrub(v, repo_root, rel_path) for k, v in value.items()}
    return value


def _dump_verse(verse) -> dict:
    return verse.model_dump(mode="json")


async def _run_calls(provider, calls: list[dict], repo_root: Path, rel_path: str | None) -> list[dict]:
    from versevision.errors import BibleProviderError, VerseVisionError
    results: list[dict] = []
    for call in calls:
        try:
            if call["op"] == "versions":
                results.append({"ok": [v.model_dump(mode="json") for v in await provider.list_versions()]})
            elif call["op"] == "health":
                results.append({"ok": (await provider.health_check()).model_dump(mode="json")})
            elif call["op"] == "fetch":
                verses = await provider.fetch_passage(call["version"], call["book"], [_span(s) for s in call["spans"]])
                results.append({"ok": [_dump_verse(v) for v in verses]})
            else:
                raise AssertionError(call)
        except (BibleProviderError, VerseVisionError) as exc:
            results.append({"error": str(exc), "class": type(exc).__name__})
    return [_scrub(r, repo_root, rel_path) for r in results]


# -------------------------------------------------------------------------------------------------------------- USFM
def _usfm_scenarios() -> list[dict]:
    def s(name, folder, config, calls):
        return {"name": name, "dir": folder, "config": {"version": "TST", **config}, "calls": calls}

    fetch = lambda version, book, *spans: {"op": "fetch", "version": version, "book": book, "spans": list(spans)}  # noqa: E731
    versions = {"op": "versions"}
    health = {"op": "health"}
    rows = [
        s("basic-range", "oracle/providers/usfm/basic", {"copyright": "Synthetic test text", "attribution": "Synthetic attribution", "name": "Synthetic Bible"},
          [versions, health, fetch("TST", "JHN", [3, 16, 3, 18]), fetch("tst", "JHN", [3, 17]), fetch("TST", "ROM", [8, 28], [8, 31]), fetch("TST", "ROM", [8, 29, 8, 31]),
           fetch("TST", "ROM", [8, 30]), fetch("TST", "ROM", [8, 28, 8, 40]), fetch("TST", "JHN", [3, 18, 3, 16]), fetch("TST", "JHN", [3, 16, 4, 1]), fetch("TST", "JHN", [3, 99]),
           fetch("TST", "GEN", [1, 1]), fetch("KJV", "JHN", [3, 16]), fetch("TST", "JHN"), fetch("TST", "JHN", [3, 16], [3, 16], [3, 17])]),
        s("basic-no-metadata", "oracle/providers/usfm/basic", {}, [versions, fetch("TST", "JHN", [3, 16, 3, 17])]),
        s("basic-lower-version", "oracle/providers/usfm/basic", {"version": "tst", "name": ""}, [versions, fetch("TST", "JHN", [3, 16]), fetch("tst", "JHN", [3, 16])]),
        s("formatting", "oracle/providers/usfm/formatting", {"copyright": "c", "version": "fmt"},
          [versions, health, fetch("FMT", "PSA", [23, 1, 23, 3]), fetch("FMT", "PSA", [24, 1]), fetch("FMT", "MAT", [5, 3, 5, 8]), fetch("FMT", "LUK", [2, 1, 2, 2]), fetch("FMT", "GEN", [1, 1]),
           fetch("FMT", "EXO", [1, 1]), fetch("FMT", "EXO", [1, 1, 1, 3]), fetch("FMT", "EXO", [1, 3]), fetch("FMT", "1CO", [13, 4, 13, 5]), fetch("FMT", "1CO", [13, 5, 14, 1])]),
        s("duplicates-and-odd", "oracle/providers/usfm/dup-and-odd", {"version": "odd"},
          [versions, health, fetch("ODD", "JHN", [1, 1]), fetch("ODD", "JHN", [1, 2]), fetch("ODD", "REV", [22, 20, 22, 21]), fetch("ODD", "REV", [22, 21]), fetch("ODD", "REV", [1, 1]),
           fetch("ODD", "TIT", [1, 1, 1, 5]), fetch("ODD", "TIT", [1, 3])]),
        s("synthetic-fixture", "python/tests/fixtures/usfm/synthetic", {"version": "NIV", "copyright": "Synthetic test content — not Scripture", "name": "Synthetic NIV acceptance fixture"},
          [versions, health, fetch("NIV", "JHN", [3, 16]), fetch("niv", "PSA", [23, 1, 23, 2]), fetch("NIV", "ROM", [8, 28]), fetch("NIV", "PHP", [4, 13]), fetch("NIV", "REV", [21, 4])]),
        s("empty-folder", "oracle/providers/usfm/empty", {}, [versions, health, fetch("TST", "JHN", [3, 16])]),
        s("missing-folder", "oracle/providers/usfm/does-not-exist", {}, [versions, health, fetch("TST", "JHN", [3, 16])]),
    ]
    return rows


def usfm_oracle(repo_root: Path) -> list[dict]:
    from versevision.bibles.local_usfm import LocalUsfmProvider
    from versevision.config import LocalUsfmProviderConfig
    rows = []
    old = os.getcwd()
    os.chdir(repo_root)
    try:
        for scenario in _usfm_scenarios():
            config = LocalUsfmProviderConfig(path=Path(scenario["dir"]), **scenario["config"])
            provider = LocalUsfmProvider("oracle_usfm", config)
            results = asyncio.run(_run_calls(provider, scenario["calls"], repo_root, scenario["dir"]))
            rows.append({**scenario, "results": results})
    finally:
        os.chdir(old)
    return rows


# ------------------------------------------------------------------------------------------------------ HTTP scenarios
class Script:
    """Canned responses by path: ``routes = {path: [response, ...]}``; the nth request gets the nth response, the last one repeats."""

    def __init__(self, routes: dict[str, list[dict]]):
        self.routes = routes
        self.count: dict[str, int] = {}
        self.log: list[dict] = []

    def handler(self, request: httpx.Request) -> httpx.Response:
        path = request.url.path
        entry = {
            "method": request.method, "path": path,
            "query": sorted([k, v] for k, v in request.url.params.multi_items()),
            "headers": {h: request.headers[h] for h in AUTH_HEADERS if h in request.headers},
        }
        self.log.append(entry)
        responses = self.routes.get(path)
        if not responses:
            return httpx.Response(404, json={"message": "no such route"})
        index = min(self.count.get(path, 0), len(responses) - 1)
        self.count[path] = self.count.get(path, 0) + 1
        spec = responses[index]
        if spec.get("timeout"):
            raise httpx.ReadTimeout("timed out", request=request)
        status = spec.get("status", 200)
        headers = spec.get("headers", {})
        if "json" in spec:
            return httpx.Response(status, json=spec["json"], headers=headers)
        if "text" in spec:
            return httpx.Response(status, content=spec["text"].encode("utf-8"), headers=headers)
        if "hex" in spec:
            return httpx.Response(status, content=bytes.fromhex(spec["hex"]), headers=headers)
        return httpx.Response(status, headers=headers)


def _client(config, script: Script) -> httpx.AsyncClient:
    return httpx.AsyncClient(base_url=config.base_url.rstrip("/"), timeout=config.timeout_seconds, transport=httpx.MockTransport(script.handler))


def _with_env(env: dict[str, str], function):
    saved = {name: os.environ.get(name) for name in env}
    saved_keys = os.environ.get("VERSEVISION_KEYS")
    os.environ["VERSEVISION_KEYS"] = str(Path(tempfile.gettempdir()) / "versevision-oracle-no-such-keys.env")
    try:
        for name, value in env.items():
            os.environ.pop(name, None)
            if value is not None:
                os.environ[name] = value
        return function()
    finally:
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value
        if saved_keys is None:
            os.environ.pop("VERSEVISION_KEYS", None)
        else:
            os.environ["VERSEVISION_KEYS"] = saved_keys


def _provider_class(kind: str):
    from versevision.bibles.api_bible import ApiBibleProvider
    from versevision.bibles.esv import EsvProvider
    from versevision.bibles.youversion import YouVersionProvider
    from versevision.config import ApiBibleProviderConfig, EsvProviderConfig, YouVersionProviderConfig
    return {"api_bible": (ApiBibleProvider, ApiBibleProviderConfig), "youversion": (YouVersionProvider, YouVersionProviderConfig), "esv": (EsvProvider, EsvProviderConfig)}[kind]


def run_http_scenario(kind: str, scenario: dict, repo_root: Path) -> dict:
    cls, config_cls = _provider_class(kind)
    env_name = ENV_NAMES[kind]
    config_fields = {"api_key_env": env_name, "base_url": ORIGIN + scenario.get("base_path", "/v1"), **scenario.get("config", {})}
    script = Script({path: list(responses) for path, responses in scenario["routes"].items()})

    def run():
        config = config_cls(**config_fields)
        provider = cls("oracle_" + kind, config)
        provider._client = _client(config, script)

        async def go():
            try:
                return await _run_calls(provider, scenario["calls"], repo_root, None)
            finally:
                await provider.aclose()
        return asyncio.run(go())

    results = _with_env({env_name: SYNTHETIC_KEY if scenario.get("key", True) else None}, run)
    return {**scenario, "kind": kind, "key": scenario.get("key", True), "results": results, "requests": script.log}


def _api_version(id_, abbreviation, **extra) -> dict:
    item = {"id": id_, "dblId": "0123456789abcdef", "abbreviation": abbreviation, "abbreviationLocal": abbreviation, "name": f"Synthetic Bible {abbreviation}",
            "nameLocal": f"Synthetic Bible {abbreviation}", "description": "Synthetic test translation", "descriptionLocal": "Synthetic test translation",
            "language": {"id": "eng", "name": "English", "nameLocal": "English", "script": "Latn", "scriptDirection": "LTR"},
            "countries": [{"id": "US", "name": "United States", "nameLocal": "United States"}], "type": "text", "updatedAt": "2024-01-01T00:00:00.000Z", "audioBibles": [],
            "copyright": f"Synthetic copyright {abbreviation}"}
    item.update(extra)
    return item


def _api_verse(bible_id, verse_id, content, **data) -> dict:
    book, chapter, number = verse_id.split(".")
    return {"data": {"id": verse_id, "orgId": verse_id, "bibleId": bible_id, "bookId": book, "chapterId": f"{book}.{chapter}", "reference": f"{book} {chapter}:{number}", "content": content, "verseCount": 1, "copyright": None, **data},
            "meta": {"fums": "<script>/* synthetic */</script>", "fumsId": "synthetic-id", "fumsJsInclude": "https://fums.test/x.js", "fumsJs": "var _BAPI=_BAPI||{};", "fumsNoScript": "<img src=\"https://fums.test/x.gif\">"}}


def _fetch(version, book, *spans):
    return {"op": "fetch", "version": version, "book": book, "spans": list(spans)}


def api_bible_scenarios() -> list[dict]:
    versions = {"op": "versions"}
    health = {"op": "health"}
    listing = {"data": [_api_version("bible-tst", "TST"), _api_version("bible-oth", "OTH", abbreviationLocal="OTHL", nameLocal=None, language={"id": "fra"}, copyright=None, description=None)]}
    verses = {f"/v1/bibles/bible-tst/verses/JHN.3.{n}": [{"json": _api_verse("bible-tst", f"JHN.3.{n}", content)}] for n, content in
              ((16, "[16] Synthetic verse sixteen."), (17, "<p class=\"p\"><span data-number=\"17\" class=\"v\">17</span>Synthetic <i>verse</i> seventeen &amp; more.</p>"), (18, "[18-19] Synthetic bridged text."))}
    rows = [
        {"name": "list-and-fetch", "routes": {"/v1/bibles": [{"json": listing}], **verses},
         "calls": [versions, health, _fetch("TST", "JHN", [3, 16]), _fetch("tst", "JHN", [3, 16, 3, 18]), _fetch("TST", "JHN", [3, 16], [3, 18]), _fetch("TST", "JHN", [3, 16, 4, 1]), _fetch("OTH", "JHN", [3, 16]), _fetch("OTHL", "JHN", [3, 16]), _fetch("NOPE", "JHN", [3, 16])]},
        {"name": "base-path-trailing-slash", "base_path": "/v1/", "routes": {"/v1/bibles": [{"json": listing}], **verses}, "calls": [_fetch("TST", "JHN", [3, 17])]},
        {"name": "configured-ids", "config": {"bible_ids": {"tst": "bible-tst", "OTH": "missing-id"}},
         "routes": {"/v1/bibles": [{"json": listing}], **verses}, "calls": [_fetch("TST", "JHN", [3, 16]), _fetch("OTH", "JHN", [3, 16])]},
        {"name": "ambiguous-and-configured", "config": {"bible_ids": {"DUP": "bible-b"}},
         "routes": {"/v1/bibles": [{"json": {"data": [_api_version("bible-a", "DUP"), _api_version("bible-b", "dup"), _api_version("bible-c", "ZZZ")]}}],
                    "/v1/bibles/bible-b/verses/JHN.3.16": [{"json": _api_verse("bible-b", "JHN.3.16", "[16] From the configured Bible.", copyright="Per-verse copyright")}]},
         "calls": [_fetch("DUP", "JHN", [3, 16]), _fetch("ZZZ", "JHN", [3, 16])]},
        {"name": "ambiguous-without-config", "routes": {"/v1/bibles": [{"json": {"data": [_api_version("bible-a", "DUP"), _api_version("bible-b", "DUP")]}}]}, "calls": [_fetch("DUP", "JHN", [3, 16]), health]},
        {"name": "listing-shapes", "routes": {"/v1/bibles": [{"json": {"data": [
            {"id": "x1", "abbreviation": "aaa"}, {"id": "x2"}, {"abbreviation": "NOID"}, {"id": "", "abbreviation": "EMPTYID"}, {"id": "x3", "abbreviationLocal": "", "abbreviation": "bbb", "name": "", "nameLocal": ""},
            {"id": "x4", "abbreviation": "ccc", "language": {}, "description": "", "copyright": ""}, {"id": "x5", "abbreviation": "ddd", "language": None, "nameLocal": "Named Local", "name": "Named"}]}}]},
         "calls": [versions, health]},
        {"name": "empty-listing", "routes": {"/v1/bibles": [{"json": {"data": []}}]}, "calls": [versions, health, _fetch("TST", "JHN", [3, 16])]},
        {"name": "no-data-key", "routes": {"/v1/bibles": [{"json": {}}]}, "calls": [versions, health]},
        {"name": "verse-contents", "routes": {"/v1/bibles": [{"json": {"data": [_api_version("bible-tst", "TST")]}}],
            "/v1/bibles/bible-tst/verses/JHN.1.1": [{"json": _api_verse("bible-tst", "JHN.1.1", "")}],
            "/v1/bibles/bible-tst/verses/JHN.1.2": [{"json": _api_verse("bible-tst", "JHN.1.2", "<p></p>")}],
            "/v1/bibles/bible-tst/verses/JHN.1.3": [{"json": _api_verse("bible-tst", "JHN.1.3", "[3]")}],
            "/v1/bibles/bible-tst/verses/JHN.1.4": [{"json": {"data": {"id": "JHN.1.4"}}}],
            "/v1/bibles/bible-tst/verses/JHN.1.5": [{"json": {"data": None}}],
            "/v1/bibles/bible-tst/verses/JHN.1.6": [{"json": {"data": {"content": "  [6]  Synthetic   text\u00a0with\u00a0nbsp.  ", "copyright": "Own copyright"}}}],
            "/v1/bibles/bible-tst/verses/JHN.1.7": [{"json": {"data": {"content": "<p>[7] Synthetic &mdash; text&hellip; &unknown; &#169;</p><!-- c --><script>x</script>", "copyright": ""}}}]},
         "calls": [_fetch("TST", "JHN", [1, 1]), _fetch("TST", "JHN", [1, 2]), _fetch("TST", "JHN", [1, 3]), _fetch("TST", "JHN", [1, 4]), _fetch("TST", "JHN", [1, 5]), _fetch("TST", "JHN", [1, 6]), _fetch("TST", "JHN", [1, 7]), _fetch("TST", "JHN", [1, 6, 1, 7])]},
        {"name": "request-shape", "routes": {"/v1/bibles": [{"json": {"data": [_api_version("bible id/with space", "TST")]}}], "/v1/bibles/bible id/with space/verses/1CO.13.4": [{"json": _api_verse("x", "1CO.13.4", "[4] Synthetic.")}]},
         "calls": [_fetch("TST", "1CO", [13, 4])]},
        # status codes, no retry
        {"name": "status-401", "routes": {"/v1/bibles": [{"status": 401, "json": {"message": "bad key"}}]}, "calls": [versions, health]},
        {"name": "status-403", "routes": {"/v1/bibles": [{"status": 403, "text": "Forbidden"}]}, "calls": [versions]},
        {"name": "status-429", "routes": {"/v1/bibles": [{"status": 429, "headers": {"retry-after": "1"}, "text": ""}]}, "calls": [versions]},
        {"name": "status-500-then-200", "routes": {"/v1/bibles": [{"status": 500, "text": "oops"}, {"json": listing}]}, "calls": [versions, versions]},
        {"name": "status-404-and-301-and-204", "routes": {"/v1/bibles": [{"status": 404, "json": {}}]}, "calls": [versions]},
        {"name": "status-301", "routes": {"/v1/bibles": [{"status": 301, "headers": {"location": "/v1/elsewhere"}, "text": ""}], "/v1/elsewhere": [{"json": listing}]}, "calls": [versions]},
        {"name": "status-204-no-body", "routes": {"/v1/bibles": [{"status": 204}]}, "calls": [versions]},
        {"name": "status-in-verse", "routes": {"/v1/bibles": [{"json": {"data": [_api_version("bible-tst", "TST")]}}], "/v1/bibles/bible-tst/verses/JHN.3.16": [{"status": 429, "text": ""}]}, "calls": [_fetch("TST", "JHN", [3, 16]), _fetch("TST", "JHN", [3, 16])]},
        {"name": "timeout-listing", "config": {"timeout_seconds": 0.3}, "routes": {"/v1/bibles": [{"timeout": True}]}, "calls": [versions, health]},
        {"name": "timeout-verse", "config": {"timeout_seconds": 0.3}, "routes": {"/v1/bibles": [{"json": {"data": [_api_version("bible-tst", "TST")]}}], "/v1/bibles/bible-tst/verses/JHN.3.16": [{"timeout": True}]}, "calls": [_fetch("TST", "JHN", [3, 16, 3, 17])]},
        {"name": "no-key", "key": False, "routes": {"/v1/bibles": [{"json": listing}]}, "calls": [versions, health, _fetch("TST", "JHN", [3, 16])]},
    ]
    for index, body in enumerate(("", "not json", "<html><body>Bad gateway</body></html>", "{\"data\": [", "{\"data\": []", "[1, 2,]", "{\"a\": 1,}", "\"unterminated", "nul", "{'a': 1}", "{\"a\" 1}", "\ufeff{\"data\": []}")):
        rows.append({"name": f"bad-json-{index}", "routes": {"/v1/bibles": [{"text": body}]}, "calls": [versions]})
    rows.append({"name": "bad-json-bytes", "routes": {"/v1/bibles": [{"hex": "ff7b7d"}]}, "calls": [versions]})
    rows.append({"name": "bad-json-bytes-2", "routes": {"/v1/bibles": [{"hex": "7b2261223a22ff227d"}]}, "calls": [versions]})
    return rows


def _yv_version(id_, abbreviation, **extra) -> dict:
    item = {"id": id_, "abbreviation": abbreviation, "localized_abbreviation": abbreviation, "title": f"Synthetic {abbreviation}", "localized_title": f"Synthetic {abbreviation} (local)", "language_tag": "en",
            "copyright": f"Synthetic copyright {abbreviation}", "promotional_content": f"Synthetic promo {abbreviation}", "publisher_url": "https://publisher.test"}
    item.update(extra)
    return item


def youversion_scenarios() -> list[dict]:
    versions = {"op": "versions"}
    health = {"op": "health"}
    listing = {"data": [_yv_version(111, "NIV11"), _yv_version("222", "tst"), _yv_version(333, "ZZZ", localized_abbreviation=None)], "next_page_token": None, "total_size": 3}
    passages = {f"/v1/bibles/{bid}/passages/JHN.3.{n}": [{"json": {"id": f"JHN.3.{n}", "content": content, "reference": f"John 3:{n}"}}] for bid in (111, 222, 333) for n, content in
                ((16, "Synthetic YouVersion verse."), (17, "  Synthetic\n   spaced\t verse  \u00a0 seventeen. "), (18, "Synthetic <b>markup</b> stays &amp; verbatim."))}
    return [
        {"name": "list-and-fetch", "routes": {"/v1/bibles": [{"json": listing}], **passages},
         "calls": [versions, health, _fetch("TST", "JHN", [3, 16]), _fetch("tst", "JHN", [3, 16, 3, 18]), _fetch("ZZZ", "JHN", [3, 16]), _fetch("NIV11", "JHN", [3, 17]), _fetch("NIV", "JHN", [3, 16]), _fetch("TST", "JHN", [3, 17], [3, 18]), _fetch("TST", "JHN", [3, 16, 4, 1])]},
        {"name": "configured-aliases", "config": {"bible_ids": {"NIV": "111", "alias": "333", "missing": "999"}}, "routes": {"/v1/bibles": [{"json": listing}], **passages},
         "calls": [versions, _fetch("NIV", "JHN", [3, 16]), _fetch("NIV11", "JHN", [3, 16]), _fetch("ALIAS", "JHN", [3, 18]), _fetch("MISSING", "JHN", [3, 16]), _fetch("ZZZ", "JHN", [3, 16])]},
        {"name": "alias-beats-abbreviation", "config": {"bible_ids": {"niv": "111"}}, "routes": {"/v1/bibles": [{"json": {"data": [_yv_version(111, "OLD"), _yv_version(112, "NIV")]}}], "/v1/bibles/111/passages/JHN.3.16": [{"json": {"content": "From 111."}}]},
         "calls": [versions, _fetch("NIV", "JHN", [3, 16])]},
        {"name": "ambiguous", "routes": {"/v1/bibles": [{"json": {"data": [_yv_version(1, "DUP"), _yv_version(2, "dup")]}}]}, "calls": [_fetch("DUP", "JHN", [3, 16]), health]},
        {"name": "listing-shapes", "routes": {"/v1/bibles": [{"json": {"data": [
            {"id": 1, "abbreviation": "aaa"}, {"id": 2}, {"abbreviation": "NOID"}, {"id": None, "abbreviation": "NULLID"}, {"id": 0, "abbreviation": "zero"}, {"id": "", "abbreviation": "emptyid"}, {"id": 3, "localized_abbreviation": "", "abbreviation": "bbb", "title": "", "localized_title": ""},
            {"id": 4, "abbreviation": "ccc", "language_tag": "", "promotional_content": "", "publisher_url": "https://p.test", "copyright": ""}, {"id": 5, "abbreviation": "ddd", "promotional_content": None, "publisher_url": None, "language_tag": "fr", "title": "Titled"},
            {"id": 6, "abbreviation": "eee", "promotional_content": "", "publisher_url": ""}, {"id": True, "abbreviation": "boolean"}]}}]}, "calls": [versions]},
        {"name": "empty-and-missing-data", "routes": {"/v1/bibles": [{"json": {"data": []}}]}, "calls": [versions, health]},
        {"name": "no-data-key", "routes": {"/v1/bibles": [{"json": {"total_size": 0}}]}, "calls": [versions]},
        {"name": "passage-contents", "routes": {"/v1/bibles": [{"json": {"data": [_yv_version(111, "TST")]}}],
            "/v1/bibles/111/passages/JHN.1.1": [{"json": {"content": ""}}], "/v1/bibles/111/passages/JHN.1.2": [{"json": {"content": "   \n  "}}], "/v1/bibles/111/passages/JHN.1.3": [{"json": {}}],
            "/v1/bibles/111/passages/JHN.1.4": [{"json": {"content": None}}], "/v1/bibles/111/passages/JHN.1.5": [{"json": {"content": "Plain."}}]},
         "calls": [_fetch("TST", "JHN", [1, 1]), _fetch("TST", "JHN", [1, 2]), _fetch("TST", "JHN", [1, 3]), _fetch("TST", "JHN", [1, 4]), _fetch("TST", "JHN", [1, 5])]},
        {"name": "status-401", "routes": {"/v1/bibles": [{"status": 401, "json": {}}]}, "calls": [versions, health]},
        {"name": "status-403", "routes": {"/v1/bibles": [{"status": 403, "text": "no licence"}]}, "calls": [versions]},
        {"name": "status-429", "routes": {"/v1/bibles": [{"status": 429, "text": ""}]}, "calls": [versions]},
        {"name": "status-502", "routes": {"/v1/bibles": [{"status": 502, "text": "bad gateway"}, {"json": listing}]}, "calls": [versions, versions]},
        {"name": "status-in-passage", "routes": {"/v1/bibles": [{"json": {"data": [_yv_version(111, "TST")]}}], "/v1/bibles/111/passages/JHN.3.16": [{"status": 403, "text": ""}]}, "calls": [_fetch("TST", "JHN", [3, 16])]},
        {"name": "timeout", "config": {"timeout_seconds": 0.3}, "routes": {"/v1/bibles": [{"timeout": True}]}, "calls": [versions, health]},
        {"name": "timeout-passage", "config": {"timeout_seconds": 0.3}, "routes": {"/v1/bibles": [{"json": {"data": [_yv_version(111, "TST")]}}], "/v1/bibles/111/passages/JHN.3.16": [{"timeout": True}]}, "calls": [_fetch("TST", "JHN", [3, 16])]},
        {"name": "no-key", "key": False, "routes": {"/v1/bibles": [{"json": listing}]}, "calls": [versions, health, _fetch("TST", "JHN", [3, 16])]},
        {"name": "bad-json", "routes": {"/v1/bibles": [{"text": "<html>nope</html>"}]}, "calls": [versions, health]},
        {"name": "bad-json-passage", "routes": {"/v1/bibles": [{"json": {"data": [_yv_version(111, "TST")]}}], "/v1/bibles/111/passages/JHN.3.16": [{"text": "{\"content\": "}]}, "calls": [_fetch("TST", "JHN", [3, 16])]},
    ]


def esv_scenarios() -> list[dict]:
    health = {"op": "health"}
    versions = {"op": "versions"}

    def passage(query, *texts):
        return {"query": query, "canonical": query, "parsed": [[43003016, 43003016]], "passage_meta": [{"canonical": query, "chapter_start": [43003001, 43003036], "chapter_end": [43003001, 43003036], "prev_verse": 43003015, "next_verse": 43003017, "prev_chapter": [43002001, 43002025], "next_chapter": [43004001, 43004054]}], "passages": list(texts)}
    routes = {"/v3/passage/text/": [{"json": passage("John 3:16", "  Synthetic ESV verse.  \n\n")}]}
    return [
        {"name": "fetch", "routes": routes, "calls": [versions, health, _fetch("ESV", "JHN", [3, 16]), _fetch("esv", "JHN", [3, 16, 3, 17]), _fetch("ESV", "1CO", [13, 4]), _fetch("ESV", "PSA", [23, 1]), _fetch("ESV", "SNG", [2, 1]), _fetch("ESV", "JHN", [3, 16, 4, 1]), _fetch("NIV", "JHN", [3, 16])], "base_path": "/v3"},
        {"name": "unknown-book", "routes": routes, "calls": [_fetch("ESV", "XXX", [1, 1]), health], "base_path": "/v3"},
        {"name": "passages-shapes", "base_path": "/v3", "routes": {"/v3/passage/text/": [{"json": passage("a", "One", " two\n\n  three ")}, {"json": passage("b")}, {"json": {"query": "c"}}, {"json": {"passages": None}}, {"json": {"passages": ["   "]}}, {"json": passage("f", "Line one\n    Line two\n\n\u00a0nbsp\u00a0 end.")}]},
         "calls": [_fetch("ESV", "JHN", [1, 1]), _fetch("ESV", "JHN", [1, 2]), _fetch("ESV", "JHN", [1, 3]), _fetch("ESV", "JHN", [1, 4]), _fetch("ESV", "JHN", [1, 5]), _fetch("ESV", "JHN", [1, 6])]},
        {"name": "status-401", "base_path": "/v3", "routes": {"/v3/passage/text/": [{"status": 401, "json": {"detail": "Invalid token."}}]}, "calls": [_fetch("ESV", "JHN", [3, 16]), health]},
        {"name": "status-403", "base_path": "/v3", "routes": {"/v3/passage/text/": [{"status": 403, "text": ""}]}, "calls": [_fetch("ESV", "JHN", [3, 16])]},
        {"name": "status-429", "base_path": "/v3", "routes": {"/v3/passage/text/": [{"status": 429, "headers": {"retry-after": "30"}, "json": {"detail": "Request was throttled."}}]}, "calls": [_fetch("ESV", "JHN", [3, 16]), health]},
        {"name": "status-500-then-200", "base_path": "/v3", "routes": {"/v3/passage/text/": [{"status": 500, "text": "x"}, {"json": passage("John 3:16", "Second try.")}]}, "calls": [_fetch("ESV", "JHN", [3, 16]), _fetch("ESV", "JHN", [3, 16])]},
        {"name": "timeout", "base_path": "/v3", "config": {"timeout_seconds": 0.3}, "routes": {"/v3/passage/text/": [{"timeout": True}]}, "calls": [_fetch("ESV", "JHN", [3, 16]), health]},
        {"name": "no-key", "key": False, "base_path": "/v3", "routes": routes, "calls": [versions, health, _fetch("ESV", "JHN", [3, 16]), _fetch("ESV", "XXX", [3, 16])]},
        {"name": "bad-json", "base_path": "/v3", "routes": {"/v3/passage/text/": [{"text": "<!doctype html><title>Error</title>"}]}, "calls": [_fetch("ESV", "JHN", [3, 16]), health]},
        {"name": "bad-json-2", "base_path": "/v3", "routes": {"/v3/passage/text/": [{"text": "{\"passages\": [\"x\"],}"}]}, "calls": [_fetch("ESV", "JHN", [3, 16])]},
    ]


# ------------------------------------------------------------------------------------------------------------ registry
def registry_scenarios() -> list[dict]:
    api_listing = {"data": [_api_version("bible-nkjv", "NKJV"), _api_version("bible-niv", "NIV"), _api_version("bible-kjv", "KJV")]}
    routes: dict[str, list[dict]] = {"/api/bibles": [{"json": api_listing}], "/yv/bibles": [{"json": {"data": [_yv_version(111, "NASB2020")]}}]}
    for bid, abbreviation in (("bible-nkjv", "NKJV"), ("bible-niv", "NIV"), ("bible-kjv", "KJV")):
        for n in range(14, 21):
            routes[f"/api/bibles/{bid}/verses/JHN.3.{n}"] = [{"json": _api_verse(bid, f"JHN.3.{n}", f"[{n}] Synthetic {abbreviation} verse {n}.")}]
    for n in range(14, 21):
        routes[f"/yv/bibles/111/passages/JHN.3.{n}"] = [{"json": {"content": f"Synthetic NASB2020 verse {n}."}}]
    routes["/esv/passage/text/"] = [{"json": {"passages": ["Synthetic ESV verse."]}}]
    base = {
        "version_routes": {"NKJV": "api", "NIV": "api", "kjv": "api_fresh", "NASB2020": "yv", "ESV": "esv", "CSB": "ghost", "NLT": "usfm", "AMP": "yv"},
        "providers": {
            "api": {"type": "api_bible", "base_url": ORIGIN + "/api", "api_key_env": ENV_NAMES["api_bible"]},
            "api_fresh": {"type": "api_bible", "base_url": ORIGIN + "/api", "api_key_env": ENV_NAMES["api_bible"], "cache_policy": "fresh"},
            "yv": {"type": "youversion", "base_url": ORIGIN + "/yv", "api_key_env": ENV_NAMES["youversion"]},
            "esv": {"type": "esv", "base_url": ORIGIN + "/esv", "api_key_env": ENV_NAMES["esv"]},
            "usfm": {"type": "usfm", "version": "NLT", "path": "oracle/providers/usfm/basic", "copyright": "Synthetic"},
        },
    }
    steps = [
        "John 3:16 NIV", "John 3:16 NIV", "John 3:16-17 NIV", "John 3:16-17 NIV", "John 3:16 NKJV", "John 3:16 NIV",
        "John 3:16 KJV", "John 3:16 KJV", "John 3:16 kjv",
        "John 3:16 NASB2020", "John 3:16 NASB2020", "John 3:16 ESV", "John 3:16 ESV", "John 3:16-17 ESV",
        "John 3:16 CSB", "John 3:16 NRSV", "John 3:16 AMP", "John 3:16 NLT", "John 3:16-18 NLT", "John 3:16-18 NLT", "John 3:30 NLT",
        "John 3:16, 18 NIV", "John 3:19-20 NIV", "John 3:19-20 NIV", "John 3:16-4:2 NIV", "John 3:16-4:2 NIV", "John 3:16",
    ]
    return [
        {"name": "routing-and-cache", "bible": base, "steps": steps, "env": {ENV_NAMES["api_bible"]: SYNTHETIC_KEY, ENV_NAMES["youversion"]: SYNTHETIC_KEY, ENV_NAMES["esv"]: SYNTHETIC_KEY}, "routes": routes},
        {"name": "no-keys", "bible": base, "steps": ["John 3:16 NIV", "John 3:16 ESV", "John 3:16 NASB2020", "John 3:16 NLT"], "env": {}, "routes": routes},
        {"name": "no-providers", "bible": {"version_routes": {"NIV": "api"}, "providers": {}}, "steps": ["John 3:16 NIV", "John 3:16 KJV"], "env": {}, "routes": {}},
        {"name": "failing-provider-is-not-cached", "bible": {"version_routes": {"NIV": "api"}, "providers": {"api": base["providers"]["api"]}}, "steps": ["John 3:16-17 NIV", "John 3:16-17 NIV", "John 3:16-17 NIV"],
         "env": {ENV_NAMES["api_bible"]: SYNTHETIC_KEY},
         "routes": {"/api/bibles": [{"json": {"data": [_api_version("bible-niv", "NIV")]}}], "/api/bibles/bible-niv/verses/JHN.3.16": [{"json": _api_verse("bible-niv", "JHN.3.16", "[16] First.")}],
                    "/api/bibles/bible-niv/verses/JHN.3.17": [{"status": 500, "text": ""}, {"json": _api_verse("bible-niv", "JHN.3.17", "[17] Second.")}]}},
    ]


def run_registry_scenario(scenario: dict, repo_root: Path) -> dict:
    from versevision.bibles.cache import ScriptureCache
    from versevision.bibles.registry import ProviderRegistry
    from versevision.config import BibleConfig
    from versevision.errors import VerseVisionError
    from versevision.references import parse_references

    script = Script({path: list(responses) for path, responses in scenario["routes"].items()})
    old = os.getcwd()

    def run():
        os.chdir(repo_root)
        with tempfile.TemporaryDirectory() as tmp:
            cache = ScriptureCache(Path(tmp) / "scripture.sqlite3")
            registry = ProviderRegistry(BibleConfig(**scenario["bible"]), cache)
            for provider in registry.providers.values():
                if hasattr(provider, "_client"):
                    provider._client = _client(provider.config, script)

            async def go():
                out = []
                for text in scenario["steps"]:
                    reference = parse_references(text).references[0]
                    try:
                        passage, name, source = await registry._fetch(reference)
                        out.append({"provider": name, "source": source, "resolved_version": passage.resolved_version, "verses": [_dump_verse(v) for v in passage.verses]})
                    except VerseVisionError as exc:
                        out.append({"error": str(exc), "class": type(exc).__name__})
                out.append({"cache_count": cache.count()})
                return out
            try:
                return asyncio.run(go())
            finally:
                cache.close()
    try:
        results = _with_env({name: None for name in ENV_NAMES.values()} | scenario["env"], run)
    finally:
        os.chdir(old)
    rel = "oracle/providers/usfm/basic"
    return {**scenario, "results": [_scrub(r, repo_root, rel) for r in results], "requests": script.log}


# --------------------------------------------------------------------------------------------------------------- driver
def providers_oracle(out: Path, repo_root: Path) -> dict[str, list[dict]]:
    """Writes the USFM folders under ``out/providers/usfm`` and returns the rows of every file by name."""
    _write_usfm_folders(out)
    files = {
        "html_text.jsonl": html_text_oracle(),
        "json_errors.jsonl": json_errors_oracle(),
        "keys.jsonl": keys_oracle(),
        "usfm.jsonl": usfm_oracle_at(out, repo_root),
        "api_bible.jsonl": [run_http_scenario("api_bible", s, repo_root) for s in api_bible_scenarios()],
        "youversion.jsonl": [run_http_scenario("youversion", s, repo_root) for s in youversion_scenarios()],
        "esv.jsonl": [run_http_scenario("esv", s, repo_root) for s in esv_scenarios()],
        "registry.jsonl": [run_registry_scenario(s, repo_root) for s in registry_scenarios()],
    }
    return files


def usfm_oracle_at(out: Path, repo_root: Path) -> list[dict]:
    return usfm_oracle(repo_root)
