#!/usr/bin/env python3
"""枢轴翻译：中文 --(CC-CEDICT 英文释义)--> 英文 --(FreeDict eng-rus)--> 俄语

直连的中俄数据覆盖不到的词，用这条链补。精度低于直连，所以只做兜底。
输出: data/raw/pivot-zh-ru.tsv
"""
import gzip, io, os, re, sys
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW  = os.path.join(ROOT, "data", "raw")
OUT  = os.path.join(RAW, "pivot-zh-ru.tsv")

# ---- eng-rus TEI ----
ENTRY = re.compile(r"<entry\b.*?</entry>", re.S)
ORTH  = re.compile(r"<orth>(.*?)</orth>", re.S)
QUOTE = re.compile(r"<quote>(.*?)</quote>", re.S)

def clean(t):
    t = re.sub(r"\[\[|\]\]", "", t)
    t = re.sub(r"<[^>]+>", "", t)
    return re.sub(r"\s+", " ", t).strip()

er = {}
p = os.path.join(RAW, "eng-rus", "eng-rus.tei")
s = io.open(p, encoding="utf-8").read()
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
print(f"eng-rus 词条: {len(er)}")

# ---- CC-CEDICT ----
line_re = re.compile(r"^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$")
ced = []
with gzip.open(os.path.join(RAW, "cedict.txt.gz"), "rt", encoding="utf-8") as f:
    for line in f:
        if line.startswith("#"):
            continue
        m = line_re.match(line.rstrip("\n"))
        if m:
            ced.append((m.group(1), m.group(2), m.group(3), [d for d in m.group(4).split("/") if d]))

STOP = re.compile(r"\([^)]*\)")
def norm_gloss(d):
    d = STOP.sub(" ", d).lower().strip()
    d = re.sub(r"^(to|a|an|the)\s+", "", d)
    d = re.sub(r"[^a-z' -]", " ", d)
    return re.sub(r"\s+", " ", d).strip()

rows, hit = [], 0
for trad, simp, num, defs in ced:
    ru = []
    for d in defs:
        k = norm_gloss(d)
        if not k or len(k) > 30:
            continue
        hit_cand = None
        for cand in (k, k[:-1] if k.endswith("s") else None):
            if cand and cand in er:
                hit_cand = cand
                break
        if hit_cand:
            # 只取第一个命中释义的译法：跨义项累加会把「和」变成「сумма」
            ru = [x for x in er[hit_cand][:3]]
            hit_key = hit_cand
            break
    if ru:
        hit += 1
        rows.append((simp, simp, trad, "", "", " | ".join(ru[:3])))
        if len(rows) <= 8:
            print(f"    [debug] {simp} <- 英文键「{hit_key}」")

print(f"CC-CEDICT {len(ced)} 条，其中 {hit} 条能经英文枢轴拿到俄语")
with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
    f.write("#title\tsimp\ttrad\tpint\tpos\tru\n")
    for r in rows:
        f.write("\t".join(r) + "\n")
print("写出:", OUT)
for w in ("了", "是", "不", "我", "你", "和", "就", "学习", "问题", "时间", "朋友"):
    for r in rows:
        if r[0] == w:
            print(f"  {w}  {r[4][:70]}")
            break
