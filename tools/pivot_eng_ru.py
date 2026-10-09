#!/usr/bin/env python3
"""枢轴翻译：中文 --(CC-CEDICT 每个读音的英文释义)--> 英文 --(FreeDict eng-rus)--> 俄语

按 CC-CEDICT 的条目逐一翻译，所以同一个字的每个读音能拿到各自的译词
（都 dou1->все / du1->столица）。
输出:
  python pivot_eng_ru.py           -> data/raw/pivot-zh-ru.tsv      （保守，质量优先）
  python pivot_eng_ru.py --ext     -> data/raw/pivot-zh-ru-ext.tsv  （激进，只当兜底）

build_dict.py 的用法是【保守表优先、扩展表兜底】：
先用保守表，保守表没命中的词才查扩展表，
所以扩展表只会"多补词"，绝不会覆盖已有的好译文。
"""
import gzip, io, os, re, sys
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW  = os.path.join(ROOT, "data", "raw")
EXT = "--ext" in sys.argv
OUT  = os.path.join(RAW, "pivot-zh-ru-ext.tsv" if EXT else "pivot-zh-ru.tsv")

ENTRY = re.compile(r"<entry\b.*?</entry>", re.S)
ORTH  = re.compile(r"<orth>(.*?)</orth>", re.S)
QUOTE = re.compile(r"<quote>(.*?)</quote>", re.S)

def clean(t):
    t = re.sub(r"\[\[|\]\]", "", t)
    t = re.sub(r"<[^>]+>", "", t)
    return re.sub(r"\s+", " ", t).strip()

# 英俄词典：先用 Wikdict（9.5 万词条，基于维基词典，新），再用 FreeDict 兜底
er = {}
n_wk = 0
wk = os.path.join(RAW, "wikdict-en-ru.tsv")
if os.path.exists(wk):
    with io.open(wk, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            c = line.rstrip("\n").split("\t")
            if len(c) >= 2 and c[0] and c[1]:
                g = [x.strip() for x in c[1].split("|") if x.strip()]
                if g:
                    er[c[0]] = g
                    n_wk += 1

n_fd = 0
s = io.open(os.path.join(RAW, "eng-rus", "eng-rus.tei"), encoding="utf-8").read()
for blk in ENTRY.findall(s):
    om = ORTH.search(blk)
    if not om:
        continue
    head = clean(om.group(1)).lower()
    g = [clean(q) for q in QUOTE.findall(blk)]
    g = [x for x in g if x and not x.startswith("(")]
    if g and head not in er:      # Wikdict 有的就不覆盖
        er[head] = g
        n_fd += 1
print(f"英俄词典: Wikdict {n_wk} + FreeDict {n_fd} = {len(er)}")

line_re = re.compile(r"^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$")
STOP = re.compile(r"\([^)]*\)")
def norm_gloss(d):
    d = STOP.sub(" ", d).lower().strip()
    d = re.sub(r"^(to|a|an|the)\s+", "", d)
    d = re.sub(r"[^a-z' -]", " ", d)
    return re.sub(r"\s+", " ", d).strip()

def match_ru(defs):
    """把一条英文释义列表匹配成俄语（返回前 3 个，找不到返回空）"""
    for d in [x for x in defs if x][: (4 if EXT else 2)]:
        k = norm_gloss(d)
        if not k or len(k) > 30:
            continue
        cands = [k]
        if k.endswith("s"):
            cands.append(k[:-1])
        if EXT and " " in k:
            cands += [w for w in k.split() if len(w) >= 3]
        for cand in cands:
            if cand and cand in er:
                return er[cand][:3]
    return []

rows = []
uh_rows = []
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
        # 只取第一个命中释义，不跨义项累加
        ru = match_ru([x for x in defs.split("/") if x])
        if ru:
            rows.append((simp, simp, trad, a, "", " | ".join(ru)))

# ---------- Unihan 补字 ----------
# CC-CEDICT 没收录的生僻字，Unihan 有 kMandarin(读音) + kDefinition(英文释义)。
# 用同一套"英文 -> 俄语"匹配把它们也翻出来。
import zipfile
uz = os.path.join(RAW, "Unihan.zip")
n_uh = 0
if os.path.exists(uz):
    kman, kdef = {}, {}
    with zipfile.ZipFile(uz).open("Unihan_Readings.txt") as fh:
        for line in io.TextIOWrapper(fh, encoding="utf-8"):
            if line.startswith("#"):
                continue
            p2 = line.rstrip("\n").split("\t")
            if len(p2) < 3 or not p2[0].startswith("U+"):
                continue
            ch = chr(int(p2[0][2:], 16))
            if p2[1] == "kMandarin":
                kman[ch] = p2[2].split()[0]
            elif p2[1] == "kDefinition":
                kdef[ch] = p2[2]
    for ch, dfn in kdef.items():
        md = kman.get(ch)
        if not md:
            continue
        a = re.sub(r"[0-9]", "", md).replace("u:", "ü").replace(" ", "").lower()
        if not a:
            continue
        # Unihan 的释义用 ; 和 , 分隔同义项
        defs = [x.strip() for x in re.split(r"[;,]", dfn) if x.strip()]
        ru = match_ru(defs)
        if ru:
            uh_rows.append((ch, ch, ch, a, "", " | ".join(ru)))
            n_uh += 1
    print(f"  Unihan 补 {n_uh} 个单字")
    uh_out = os.path.join(RAW, "pivot-unihan-zh-ru-ext.tsv" if EXT else "pivot-unihan-zh-ru.tsv")
    with io.open(uh_out, "w", encoding="utf-8", newline="\n") as f:
        f.write("#title\tsimp\ttrad\tpinyin_ascii\tpos\tru\n")
        for r in uh_rows:
            f.write("\t".join(r) + "\n")
    print(f"  -> {os.path.basename(uh_out)}: {len(uh_rows)} 行")

print(f"枢轴{'（扩展）' if EXT else ''}: {len(rows)} 行 -> {os.path.basename(OUT)}")
with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
    f.write("#title\tsimp\ttrad\tpinyin_ascii\tpos\tru\n")
    for r in rows:
        f.write("\t".join(r) + "\n")
for w in ("都", "地", "的", "说", "长"):
    for r in rows:
        if r[1] == w:
            print(f"  {w} {r[3]:<8} {r[5][:46]}")
