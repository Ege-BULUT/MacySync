#!/usr/bin/env python3
"""Write the Turkish copy of the site under site/tr/ and link the two languages.

The English pages are the source. Each `data-i` text gets its Turkish value from the TR
dictionary in site/site.js; long sections already carry both languages as data-l blocks.
Search engines need a real URL per language, so Turkish searches can find the site.

Run after any site edit:  python3 tools/build_tr.py
"""
import json, os, posixpath, re

SITE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "site")
BASE = "https://macysync.vercel.app/"
# page dir -> (Turkish title, Turkish description)
PAGES = {
    "": ("MacySync: bir Mac'i yeni bir Mac'e uçtan uca şifreli taşı",
         "MacySync bir Mac'i diğerine taşır: ayarlarıyla uygulamalar, anahtarlar, dotfile'lar ve seçtiğin klasörler. Her Mac gönderir ya da alır, dosyalar doğrudan Mac'ten Mac'e şifreli gider ve kurulumu bir yapay zekâ ajanı bitirir."),
    "features/": ("Özellikler: Gönder ya da Al, seç, taşı ve kur · MacySync",
                  "Her Mac oturum başına gönderir ya da alır; bir ağaçtan istediklerini işaretle; dosyalar Mac'ten Mac'e uçtan uca şifreli gider; kurulumu bir yapay zekâ ajanı yapar."),
    "docs/": ("Belgeler: MacySync ile Mac nasıl taşınır",
              "MacySync'in neye ihtiyacı olduğu, ilk açılış, Mac'leri bulma ve eşleme, seçme ve taşıma, kurulum ajanı, güvenlik ve komut satırı."),
    "changelog/": ("Değişiklikler · MacySync", "Her MacySync sürümü ve neyin değiştiği, en yenisi üstte."),
    "download/": ("MacySync'i macOS için indir",
                  "Apple silicon macOS için MacySync 0.1.0: eski Mac için de yeni Mac için de tek uygulama."),
    "wavy/": ("WavySync, MacySync'in ikizi: Wave Terminal'i bilgisayarlar arasında senkronla",
              "WavySync, Wave Terminal'i her bilgisayarda aynı tutar: ayarlar, çalışma alanları, düzenler ve widget'lar, senin GitHub deponla. MacySync'in ikizi."),
}


def read(rel):
    with open(os.path.join(SITE, rel), encoding="utf-8") as f:
        return f.read()


def write(rel, text):
    path = os.path.join(SITE, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)


js = read("site.js")
TR = json.loads(js[js.index("const TR = ") + len("const TR = "):js.index("\n};") + 2])


def esc(v):
    return v.replace("&", "&amp;").replace('"', "&quot;")


def alternates(page):
    return (f'<link rel="alternate" hreflang="en" href="{BASE}{page}">\n'
            f'<link rel="alternate" hreflang="tr" href="{BASE}tr/{page}">\n'
            f'<link rel="alternate" hreflang="x-default" href="{BASE}{page}">\n')


def with_alternates(html, page, locale):
    html = re.sub(r'<link rel="alternate" hreflang="[^"]+" href="[^"]*">\n', "", html)
    html = re.sub(r'<meta property="og:locale" content="[^"]*">\n', "", html)
    i = html.index('<link rel="icon"')
    return html[:i] + alternates(page) + f'<meta property="og:locale" content="{locale}">\n' + html[i:]


def set_meta(html, attr, name, value):
    pat = rf'(<meta {attr}="{re.escape(name)}" content=")[^"]*(">)'
    assert re.search(pat, html), name
    return re.sub(pat, lambda m: m.group(1) + esc(value) + m.group(2), html)


def retarget(html, page):
    """Point relative links of an English page at /tr/: pages to their Turkish copy, files to the root."""
    en_dir, tr_dir = page, "tr/" + page

    def fix(m):
        attr, url = m.group(1), m.group(2)
        if re.match(r"^(?:[a-z]+:|//|#|/)", url):
            return m.group(0)
        path, hash_ = (url.split("#", 1) + [""])[:2]
        target = posixpath.normpath(posixpath.join(en_dir, path)) if path else en_dir.rstrip("/")
        target = "" if target == "." else target
        is_page = path == "" or path.endswith("/") or (target + "/") in PAGES or target == ""
        if is_page:
            target = posixpath.join("tr", target) if target else "tr"
            rel = posixpath.relpath(target, tr_dir.rstrip("/") or ".")
            rel = "./" if rel == "." else rel + "/"
        else:
            rel = posixpath.relpath(target, tr_dir.rstrip("/"))
        return f'{attr}="{rel}{"#" + hash_ if hash_ else ""}"'

    return re.sub(r'\b(href|src)="([^"]*)"', fix, html)


def translate(html):
    def fill(m):
        v = TR.get(m.group(3))
        return m.group(0) if v is None else f"<{m.group(1)}{m.group(2)}>{v}</{m.group(1)}>"
    return re.sub(r'<(\w+)((?:\s[^>]*)?\sdata-i="([^"]+)"[^>]*)>(.*?)</\1>', fill, html, flags=re.S)


for page, (title, desc) in PAGES.items():
    src = page + "index.html"
    en = with_alternates(read(src), page, "en_US")
    write(src, en)

    tr = en.replace('<html lang="en">', '<html lang="tr">', 1)
    tr = with_alternates(tr, page, "tr_TR")
    tr = re.sub(r"<title>.*?</title>", f"<title>{title}</title>", tr, count=1)
    for attr, name, value in [("name", "description", desc), ("property", "og:title", title),
                              ("property", "og:description", desc), ("name", "twitter:title", title),
                              ("name", "twitter:description", desc), ("property", "og:url", BASE + "tr/" + page)]:
        tr = set_meta(tr, attr, name, value)
    tr = tr.replace(f'<link rel="canonical" href="{BASE}{page}">', f'<link rel="canonical" href="{BASE}tr/{page}">', 1)
    tr = translate(retarget(tr, page))
    # English-only blocks would sit hidden in the Turkish page; drop them.
    tr = re.sub(r'\n?[ \t]*<(\w+)[^>]*\sdata-l="en"[^>]*>.*?</\1>', "", tr, flags=re.S)
    assert 'data-l="en"' not in tr and tr.count("<div") == tr.count("</div>"), page
    tr = tr.replace('aria-label="Language">TR</button>', 'aria-label="Language">EN</button>', 1)
    write("tr/" + src, tr)

urls = "".join(
    f'  <url><loc>{BASE}{p}</loc><xhtml:link rel="alternate" hreflang="en" href="{BASE}{q}"/>'
    f'<xhtml:link rel="alternate" hreflang="tr" href="{BASE}tr/{q}"/></url>\n'
    for q in PAGES for p in (q, "tr/" + q))
write("sitemap.xml", '<?xml version="1.0" encoding="UTF-8"?>\n'
      '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">\n'
      + urls + "</urlset>\n")
print(f"wrote {len(PAGES)} Turkish pages and sitemap.xml")
