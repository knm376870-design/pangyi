#!/usr/bin/env python3
"""枢轴翻译：中文 --(CC-CEDICT 每个读音的英文释义)--> 英文 --(FreeDict eng-rus)--> 俄语

按 CC-CEDICT 的条目逐一翻译，所以同一个字的每个读音能拿到各自的译词
（都 dou1->все / du1->столица）。
输出: data/raw/pivot-zh-ru.tsv   #title  simp  trad  pinyin_ascii  pos  ru
"""
import gzip, io, os, re, sys
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW  = os.path.join(ROOT, "data", "raw")
OUT  = os.path.join(RAW, "pivot-zh-ru.tsv")

ENTRY = re.compile(r"<entry\b.*?</entry>", re.S)
ORTH  = re.compile(r"<orth>(.*?)</orth>", re.S)
QUOTE = re.compile(r"<quote>(.*?)</quote>", re.S)

def clean(t):
    t = re.sub(r"\[\[|\]\]", "", t)
    t = re.sub(r"<[^>]+>", "", t)
    return re.sub(r"\s+", " ", t).strip()

er = {}
s = io.open(os.path.join(RAW, "eng-rus", "eng-rus.tei"), encoding="utf-8").read()
for blk in ENTRY.findall(s):
    om = ORTH.search(blk)
    if not om:
        continue
    head = clean(om.group(1)).lower()
    g = [clean(q) for q in QUOTE.findall(blk)]
    g = [x for x in g if x and not x.startswith("(")]
    if g:
        er.setdefault(head, [])
        for x in g:
            if x not in er[head]:
                er[head].append(x)

line_re = re.compile(r"^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$")
STOP = re.compile(r"\([^)]*\)")
def norm_gloss(d):
    d = STOP.sub(" ", d).lower().strip()
    d = re.sub(r"^(to|a|an|the)\s+", "", d)
    d = re.sub(r"[^a-z' -]", " ", d)
    return re.sub(r"\s+", " ", d).strip()

rows = []
with gzip.open(os.path.join(RAW, "cedict.txt.gz"), "rt", encoding="utf-8") as f:
    for line in f:
        if line.startswith("#"):
            continue
        m = line_re.match(line.rstrip("\n"))
        if not m:
            continue
        trad, simp, num, defs = m.group(1), m.group(2), m.group(3), m.group(4)
        a = re.sub(r"[0-9]", "", num).replace("u:", "ü").replace(" ", "").lower()
        if not a:
            continue
        ru = []
        # 只在前两个释义里找：越往后越可能是冷门义项（都 dōu 的 "even" 会撞上
        # eng-rus 里表示「埃文基族」的同形词）
        for d in [x for x in defs.split("/") if x][:2]:
            k = norm_gloss(d)
            if not k or len(k) > 30:
                continue
            hit_cand = None
            for cand in (k, k[:-1] if k.endswith("s") else None):
                if cand and cand in er:
                    hit_cand = cand
                    break
            if hit_cand:
                # 只取第一个命中释义，不跨义项累加
                ru = er[hit_cand][:3]
                break
        if ru:
            rows.append((simp, simp, trad, a, "", " | ".join(ru)))

print(f"枢轴: {len(rows)} 行（按读音）")
with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
    f.write("#title\tsimp\ttrad\tpinyin_ascii\tpos\tru\n")
    for r in rows:
        f.write("\t".join(r) + "\n")
for w in ("都", "地", "的", "说", "长"):
    for r in rows:
        if r[1] == w:
            print(f"  {w} {r[3]:<8} {r[5][:46]}")
