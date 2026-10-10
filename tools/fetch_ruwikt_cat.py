# -*- coding: utf-8 -*-
"""全量抓 ru.wiktionary 的汉语词条（走分类树，不再靠候选词表）。
用法: python tools/fetch_ruwikt_cat.py enum      # 只枚举，报数量
      python tools/fetch_ruwikt_cat.py fetch 6000
输出追加到 data/raw/ruwikt-zh-ru.tsv（与旧脚本同格式）
"""
import io, json, os, re, sys, time, urllib.parse, urllib.request
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__)); ROOT = os.path.dirname(HERE)
RAW  = os.path.join(ROOT, "data", "raw")
OUT  = os.path.join(RAW, "ruwikt-zh-ru.tsv")
DONE = os.path.join(RAW, "ruwikt-cat-done.txt")
API  = "https://ru.wiktionary.org/w/api.php"
UA   = "Pangyi/0.4 (personal zh-ru IME; local user) python-urllib"
CJK  = re.compile(r"[\u4e00-\u9fff]")
DELAY = 2.5
POS  = {"сущ":"n","гл":"v","прил":"adj","нар":"adv","мест":"pron","числ":"num","част":"part",
        "предл":"prep","союз":"conj","межд":"intj","предик":"predic","собств":"pn","погов":"idiom"}
HEAD = re.compile(r"^==\s*\{\{\s*заголовок\s*\|[^}]*\}\}\s*==\s*$", re.M)
ZM   = re.compile(r"={3,4}\s*Значение\s*={3,4}(.*?)(?=\n={3,4}\s|\Z)", re.S)
ROOTS = ["Категория:Китайский язык", "Категория:Ханьцзы", "Категория:Китайские существительные",
         "Категория:Китайские глаголы", "Категория:Китайские прилагательные",
         "Категория:Китайские наречия", "Категория:Китайские числительные",
         "Категория:Китайские местоимения", "Категория:Китайские счётные слова",
         "Категория:Китайские фразеологизмы"]

def api(**kw):
    kw.update({"format":"json","formatversion":"2"})
    u = API + "?" + urllib.parse.urlencode(kw)
    for a in range(5):
        try:
            r = urllib.request.Request(u, headers={"User-Agent": UA})
            with urllib.request.urlopen(r, timeout=90) as x:
                return json.load(x)
        except Exception as e:
            code = getattr(e, "code", 0)
            if a == 4: return None
            time.sleep((20 if code == 429 else 3) * (a + 1))
    return None

def members(cat, types="page|subcat"):
    out, cont = [], None
    while True:
        kw = dict(action="query", list="categorymembers", cmtitle=cat, cmlimit="500", cmtype=types)
        if cont: kw["cmcontinue"] = cont
        d = api(**kw)
        if not d: break
        q = d.get("query", {})
        for m in q.get("categorymembers", []):
            out.append((m["title"], m.get("ns")))
        cont = (d.get("continue") or {}).get("cmcontinue")
        if not cont: break
    return out

def enumerate_all():
    seen_cat, pages = set(), set()
    q = list(ROOTS)
    for depth in range(3):
        nxt = []
        for cat in q:
            if cat in seen_cat: continue
            seen_cat.add(cat)
            ms = members(cat)
            for t, ns in ms:
                if ns == 14:
                    nxt.append(t)
                elif CJK.search(t):
                    pages.add(t)
            print(f"   {cat:<48} 成员 {len(ms):>5}  累计词条 {len(pages):>6}", flush=True)
        q = nxt
        if not q: break
    return pages

def cmn_block(text):
    m = re.search(r"=\s*\{\{\s*-(?:cmn|hanzi)-\s*\}\}\s*=", text)
    if not m: return None
    rest = text[m.end():]
    n = re.search(r"^=\s*\{\{\s*-[a-z-]+-\s*\}\}\s*=", rest, re.M)
    return rest[:n.start()] if n else rest

def strip_t(s):
    p = None
    while p != s:
        p = s; s = re.sub(r"\{\{[^{}]*\}\}", "", s)
    return s

def clean(line):
    line = re.sub(r"\{\{пример\|.*", "", line)
    line = strip_t(line)
    line = re.sub(r"\[\[([^\]|]*\|)?([^\]]*)\]\]", r"\2", line)
    line = re.sub(r"''+", "", line)
    line = re.sub(r"<[^>]+>", "", line)
    line = re.sub(r"\{\{|\}\}", "", line)
    return re.sub(r"\s+", " ", line).strip(" ,;:·—-—")

def parse(text):
    blk = cmn_block(text)
    if not blk: return []
    head = text[:1200]
    tra = re.search(r"\btra\s*=\s*([^|}\s]+)", head); sim = re.search(r"\bsim\s*=\s*([^|}\s]+)", head)
    tra = tra.group(1) if tra else None; sim = sim.group(1) if sim else None
    secs = [p for p in HEAD.split(blk)[1:] if p.strip()] if HEAD.search(blk) else [blk]
    out = []
    for sec in secs:
        m = re.search(r"pint\s*=\s*([^|}\s]+)", sec); pint = m.group(1) if m else None
        if not pint:
            m2 = re.search(r"пиньинь\s*:?\s*([^\n<]+)", sec); pint = m2.group(1).strip() if m2 else None
        pos = [c for t, c in POS.items() if re.search(r"\{\{\s*"+t+r"\w*\s*\|?\s*cmn", sec)]
        gloss = []
        zm = ZM.search(sec)
        if zm:
            for line in zm.group(1).splitlines():
                line = line.strip()
                if line.startswith("#"):
                    g = clean(line.lstrip("#"))
                    if g and g not in gloss and len(g) <= 60: gloss.append(g)
        if gloss: out.append((sim, tra, pint, "/".join(pos), " | ".join(gloss[:4])))
    return out

if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "enum"
    limit = int(sys.argv[2]) if len(sys.argv) > 2 else 6000
    PAGESF = os.path.join(RAW, "ruwikt-cat-pages.txt")
    if os.path.exists(PAGESF) and os.path.getsize(PAGESF) > 1000:
        pages = sorted(set(x.strip() for x in io.open(PAGESF, encoding="utf-8") if x.strip()))
        print(f"复用已保存清单 {len(pages)} 个词条页（不再重新枚举）", flush=True)
    else:
        print("枚举分类树…", flush=True)
        pages = sorted(enumerate_all())
        print(f"分类树里共 {len(pages)} 个汉语词条页", flush=True)
        io.open(PAGESF, "w", encoding="utf-8").write("\n".join(pages))
    if cmd == "enum":
        print("前 60:", " ".join(pages[:60])); sys.exit(0)

    done = set()
    if os.path.exists(DONE):
        done = set(x.strip() for x in io.open(DONE, encoding="utf-8") if x.strip())
    if os.path.exists(OUT):
        for l in io.open(OUT, encoding="utf-8"):
            if not l.startswith("#") and l.strip(): done.add(l.split("\t")[0])
    PURE = re.compile(r'^[\u4e00-\u9fff\u3400-\u4dbf]+$')
    pages = [p for p in pages if PURE.match(p)]
    print(f'纯汉字标题 {len(pages)} 个', flush=True)
    todo = [p for p in pages if p not in done][:limit]
    print(f"待抓 {len(todo)}（已完成 {len(done)}）", flush=True)
    if not todo: sys.exit(0)
    fo = io.open(OUT, "a", encoding="utf-8", newline="\n")
    rows = 0; t0 = time.time()
    for i in range(0, len(todo), 20):
        chunk = todo[i:i+50]
        d = api(action="query", prop="revisions", rvprop="content", rvslots="main",
                redirects="1", titles="|".join(chunk))
        if d:
            for pg in d.get("query", {}).get("pages", []):
                try: text = pg["revisions"][0]["slots"]["main"]["content"]
                except Exception: continue
                for r in parse(text):
                    fo.write("\t".join([pg["title"]] + [str(x) if x is not None else "" for x in r]) + "\n")
                    rows += 1
        with io.open(DONE, "a", encoding="utf-8") as fd: fd.write("\n".join(chunk) + "\n")
        fo.flush()
        if (i // 50) % 5 == 0:
            print(f"   {i+len(chunk)}/{len(todo)}  解析 {rows} 行  {time.time()-t0:.0f}s", flush=True)
        time.sleep(DELAY)
    fo.close()
    print(f"完成: 本次解析 {rows} 行")
