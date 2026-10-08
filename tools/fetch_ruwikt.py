#!/usr/bin/env python3
"""从俄语维基词典抓「中文词 -> 俄语释义」（带限速 + 断点续抓）。

缓存: data/raw/ruwikt-zh-ru.tsv   已解析出的行
      data/raw/ruwikt-tried.txt   已尝试过的词（续抓用）
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

CJK = re.compile(r"^[\u4e00-\u9fff]+$")
POS = {"сущ": "n", "гл": "v", "прил": "adj", "нар": "adv", "мест": "pron",
       "числ": "num", "част": "part", "предл": "prep", "союз": "conj",
       "межд": "intj", "предик": "predic", "собств": "pn", "погов": "idiom"}


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
            wait = 5 * (attempt + 1) if code == 429 else 2 * (attempt + 1)
            if attempt == 4:
                return None
            time.sleep(wait)
    return None


def cmn_block(text):
    # 汉语词条有两种页头：{{-cmn-}} 是词，{{-hanzi-}} 是单字，两种都要认
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


def parse(text):
    blk = cmn_block(text)
    if not blk:
        return None
    head = text[:1200]
    def grab(k, src):
        m = re.search(k + r"\s*=\s*([^|}\s]+)", src)
        return m.group(1) if m else None
    pint = grab("pint", blk) or grab("pint", head)
    tra  = grab("tra", blk) or grab("tra", head)
    sim  = grab("sim", blk) or grab("sim", head)
    if not pint:
        m = re.search(r"пиньинь\s*:?\s*([^\n<]+)", blk)
        if m:
            pint = m.group(1).strip()
    pos = [c for t, c in POS.items()
           if re.search(r"\{\{\s*" + t + r"\w*\s*\|?\s*cmn", blk)]
    gloss, zm = [], re.search(r"====?\s*Значение\s*====?(.*?)(?=\n====|\Z)", blk, re.S)
    if zm:
        for line in zm.group(1).splitlines():
            line = line.strip()
            if not line.startswith("#"):
                continue
            g = clean(line.lstrip("#"))
            if g and g not in gloss and len(g) <= 60:
                gloss.append(g)
    if not gloss:
        return None
    return (sim, tra, pint, "/".join(pos), " | ".join(gloss[:4]))


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
                p = line.split("\t")
                if len(p) > 1:
                    have.add(p[0])

CHARS_ONLY = len(sys.argv) > 2 and sys.argv[2] == "chars"

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
        if w not in have and w not in done and w not in seen_out
        and (not CHARS_ONLY or len(w) == 1)]
print(f"前 {LIMIT} 常用词: 已有 {LIMIT - len([1 for w,_ in freq[:LIMIT] if w not in have])}, "
      f"待抓 {len(todo)}", flush=True)
if not todo:
    print("没有要抓的了"); sys.exit(0)

rows, miss, parsed, fails = [], 0, 0, 0
fo = io.open(OUT, "a", encoding="utf-8", newline="\n")
if not os.path.exists(OUT) or os.path.getsize(OUT) == 0:
    fo.write("#title\tsimp\ttrad\tpinyin_num\tpos\tru\n"); fo.flush()

t0 = time.time()
for i in range(0, len(todo), BATCH):
    chunk = todo[i:i + BATCH]
    pages = api(chunk)
    if pages is None:
        fails += 1
    else:
        for page in pages:
            if "missing" in page or "revisions" not in page:
                miss += 1
                continue
            try:
                text = page["revisions"][0]["slots"]["main"]["content"]
            except Exception:
                continue
            r = parse(text)
            if r:
                parsed += 1
                fo.write("\t".join([page["title"]] +
                                   [str(x) if x is not None else "" for x in r]) + "\n")
            else:
                miss += 1
    fo.write("")
    with io.open(DONE, "a", encoding="utf-8") as fd:
        fd.write("\n".join(chunk) + "\n")
    if (i // BATCH) % 5 == 0:
        el = time.time() - t0
        print(f"  {i+len(chunk)}/{len(todo)}  解析 {parsed}  失败批次 {fails}  {el:.0f}s",
              flush=True)
    fo.flush()
    time.sleep(DELAY)

fo.close()
print(f"\n本次解析 {parsed} 条，无词条/未解析 {miss}，失败批次 {fails}")
with io.open(OUT, encoding="utf-8") as f:
    total = sum(1 for l in f if not l.startswith("#"))
print(f"缓存累计: {total} 条 -> {OUT}")
