#!/usr/bin/env python3
"""从俄语维基词典抓「中文词 -> 俄语释义」，按义项分节、逐读音。

俄语维基的汉语词条按「== {{заголовок|N}} ==」分节，每节是一个读音/词性，
节内自带 pint=（拼音）和自己的 Значение（俄语释义）。分节解析后每个读音
配到自己的译词，不再是所有读音共用一份。

缓存: data/raw/ruwikt-zh-ru.tsv （一个读音一行） / ruwikt-tried.txt
"""
import io, json, os, re, sys, time, urllib.parse, urllib.request

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW  = os.path.join(ROOT, "data", "raw")
OUT  = os.path.join(RAW, "ruwikt-zh-ru.tsv")
DONE = os.path.join(RAW, "ruwikt-tried.txt")

API   = "https://ru.wiktionary.org/w/api.php"
UA    = ("Pangyi/0.1 (personal zh-ru input-method prototype; "
         "https://github.com/; contact: local-user) python-urllib")
BATCH = 20
DELAY = 1.1
LIMIT = int(sys.argv[1]) if len(sys.argv) > 1 else 5000
CHARS_ONLY = len(sys.argv) > 2 and sys.argv[2] == "chars"

CJK = re.compile(r"^[\u4e00-\u9fff]+$")
POS = {"сущ": "n", "гл": "v", "прил": "adj", "нар": "adv", "мест": "pron",
       "числ": "num", "част": "part", "предл": "prep", "союз": "conj",
       "межд": "intj", "предик": "predic", "собств": "pn", "погов": "idiom"}
HEAD = re.compile(r"^==\s*\{\{\s*заголовок\s*\|[^}]*\}\}\s*==\s*$", re.M)
ZM   = re.compile(r"={3,4}\s*Значение\s*={3,4}(.*?)(?=\n={3,4}\s|\Z)", re.S)


def api(titles):
    q = {"action": "query", "prop": "revisions", "rvprop": "content",
         "rvslots": "main", "format": "json", "formatversion": "2",
         "redirects": "1", "titles": "|".join(titles)}
    req = urllib.request.Request(API + "?" + urllib.parse.urlencode(q),
                                 headers={"User-Agent": UA})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=90) as r:
                return json.load(r)["query"].get("pages", [])
        except Exception as e:
            code = getattr(e, "code", 0)
            if attempt == 4:
                return None
            time.sleep(5 * (attempt + 1) if code == 429 else 2 * (attempt + 1))
    return None


def cmn_block(text):
    # 汉语词条两种页头：{{-cmn-}} 是词，{{-hanzi-}} 是单字
    m = re.search(r"=\s*\{\{\s*-(?:cmn|hanzi)-\s*\}\}\s*=", text)
    if not m:
        return None
    rest = text[m.end():]
    nxt = re.search(r"^=\s*\{\{\s*-[a-z-]+-\s*\}\}\s*=", rest, re.M)
    return rest[:nxt.start()] if nxt else rest


def strip_templates(s):
    prev = None
    while prev != s:
        prev = s
        s = re.sub(r"\{\{[^{}]*\}\}", "", s)
    return s


def clean(line):
    line = re.sub(r"\{\{пример\|.*", "", line)
    line = strip_templates(line)
    line = re.sub(r"\[\[([^\]|]*\|)?([^\]]*)\]\]", r"\2", line)
    line = re.sub(r"''+", "", line)
    line = re.sub(r"<[^>]+>", "", line)
    line = re.sub(r"\{\{|\}\}", "", line)
    return re.sub(r"\s+", " ", line).strip(" ,;:·—-—")


def sections(block):
    """按 == {{заголовок|N}} == 分节；没有这种标题就整块算一节。"""
    if HEAD.search(block):
        return [p for p in HEAD.split(block)[1:] if p.strip()]
    return [block]


def parse_section(sec):
    m = re.search(r"pint\s*=\s*([^|}\s]+)", sec)
    pint = m.group(1) if m else None
    if not pint:
        m2 = re.search(r"пиньинь\s*:?\s*([^\n<]+)", sec)
        pint = m2.group(1).strip() if m2 else None
    pos = [c for t, c in POS.items() if re.search(r"\{\{\s*" + t + r"\w*\s*\|?\s*cmn", sec)]
    gloss = []
    zm = ZM.search(sec)
    if zm:
        for line in zm.group(1).splitlines():
            line = line.strip()
            if line.startswith("#"):
                g = clean(line.lstrip("#"))
                if g and g not in gloss and len(g) <= 60:
                    gloss.append(g)
    return pint, "/".join(pos), gloss


def parse(text):
    """返回 [(simp, trad, pint, pos, ru), ...]，一个读音一行。"""
    blk = cmn_block(text)
    if not blk:
        return []
    head = text[:1200]
    tra = re.search(r"\btra\s*=\s*([^|}\s]+)", head)
    sim = re.search(r"\bsim\s*=\s*([^|}\s]+)", head)
    tra = tra.group(1) if tra else None
    sim = sim.group(1) if sim else None
    out = []
    for sec in sections(blk):
        pint, pos, gloss = parse_section(sec)
        if gloss:
            out.append((sim, tra, pint, pos, " | ".join(gloss[:4])))
    return out


# ---------- 待抓清单 ----------
freq = []
with io.open(os.path.join(RAW, "jieba_dict.txt"), encoding="utf-8") as f:
    for line in f:
        p = line.split()
        if len(p) >= 2 and p[1].isdigit() and CJK.match(p[0]):
            freq.append((p[0], int(p[1])))
freq.sort(key=lambda x: -x[1])

have = set()
cur = os.path.join(ROOT, "data", "zh-ru.tsv")
if os.path.exists(cur):
    with io.open(cur, encoding="utf-8") as f:
        for line in f:
            if not line.startswith("#"):
                c = line.split("\t")
                if len(c) > 1:
                    have.add(c[0])

done = set()
if os.path.exists(DONE) and not CHARS_ONLY:
    with io.open(DONE, encoding="utf-8") as f:
        done = set(x.strip() for x in f if x.strip())

seen_out = set()
if os.path.exists(OUT):
    with io.open(OUT, encoding="utf-8") as f:
        for line in f:
            if not line.startswith("#") and line.strip():
                seen_out.add(line.split("\t")[0])

todo = [w for w, _ in freq[:LIMIT]
        if (CHARS_ONLY or w not in have) and w not in done and w not in seen_out
        and (not CHARS_ONLY or len(w) == 1)]
print(f"前 {LIMIT} 常用词: 待抓 {len(todo)}  (chars_only={CHARS_ONLY})", flush=True)
if not todo:
    print("没有要抓的了"); sys.exit(0)

rows, pages_n, fails = [], 0, 0
fo = io.open(OUT, "a", encoding="utf-8", newline="\n")
if not os.path.exists(OUT) or os.path.getsize(OUT) == 0:
    fo.write("#title\tsimp\ttrad\tpint\tpos\tru\n"); fo.flush()

t0 = time.time()
for i in range(0, len(todo), BATCH):
    chunk = todo[i:i + BATCH]
    pages = api(chunk)
    if pages is None:
        fails += 1
    else:
        for page in pages:
            if "missing" in page or "revisions" not in page:
                continue
            try:
                text = page["revisions"][0]["slots"]["main"]["content"]
            except Exception:
                continue
            pages_n += 1
            for r in parse(text):
                rows.append((page["title"],) + r)
                fo.write("\t".join([page["title"]] +
                                   [str(x) if x is not None else "" for x in r]) + "\n")
    with io.open(DONE, "a", encoding="utf-8") as fd:
        fd.write("\n".join(chunk) + "\n")
    if (i // BATCH) % 5 == 0:
        print(f"  {i+len(chunk)}/{len(todo)}  有词条 {pages_n}  解析 {len(rows)} 行  "
              f"失败 {fails}  {time.time()-t0:.0f}s", flush=True)
    fo.flush()
    time.sleep(DELAY)
fo.close()

print(f"\n本次: 有词条页面 {pages_n}，解析出 {len(rows)} 行（一个读音一行）")
