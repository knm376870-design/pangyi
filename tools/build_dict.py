#!/usr/bin/env python3
"""Pangyi / 旁译 - 词典构建。

合并四个免费数据源 -> data/zh-ru.tsv
  cedict.txt.gz       CC-CEDICT          CC BY-SA 4.0  繁/简/拼音
  zho-rus.tei         FreeDict zho-rus   CC BY-SA 3.0  繁 -> 俄语（偏生僻）
  ruwikt-zh-ru.tsv    RU Wiktionary      CC BY-SA 3.0  常用词 -> 俄语（直接）
  jieba_dict.txt      jieba 词频          MIT           候选排序
"""
import gzip, io, os, re, sys
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW  = os.path.join(ROOT, "data", "raw")
OUT  = os.path.join(ROOT, "data", "zh-ru.tsv")

TONE = {"a": "āáǎà", "e": "ēéěè", "i": "īíǐì", "o": "ōóǒò", "u": "ūúǔù", "ü": "ǖǘǚǜ"}
PLAIN = {c: v for v, s in TONE.items() for c in s}
VOWELS = "aeiouü"
CJK = re.compile(r"^[\u4e00-\u9fff]+$")


def tone_mark(syl, tone):
    syl = syl.replace("u:", "ü").replace("v", "ü").lower()
    if not tone or tone == 5:
        return syl
    if "a" in syl:
        i = syl.index("a")
    elif "e" in syl:
        i = syl.index("e")
    elif "ou" in syl:
        i = syl.index("o")
    else:
        pos = [k for k, c in enumerate(syl) if c in VOWELS]
        if not pos:
            return syl
        i = pos[-1]
    c = syl[i]
    return syl[:i] + TONE[c][tone - 1] + syl[i + 1:] if c in TONE else syl


def parse_pinyin(num):
    disp, asc = [], []
    for syl, d in re.findall(r"([A-Za-zü:ê]+)([1-5])", num or ""):
        t = int(d)
        m = tone_mark(syl, t)
        disp.append(m)
        asc.append("".join(PLAIN.get(c, c) for c in m))
    return "".join(asc), "".join(disp)


# ---------- CC-CEDICT ----------
ced = {}
line_re = re.compile(r"^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$")
with gzip.open(os.path.join(RAW, "cedict.txt.gz"), "rt", encoding="utf-8") as f:
    for line in f:
        if line.startswith("#"):
            continue
        m = line_re.match(line.rstrip("\n"))
        if not m:
            continue
        trad, simp, num = m.group(1), m.group(2), m.group(3)
        a, d = parse_pinyin(num)
        ced.setdefault(trad, (simp, a, d))
        ced.setdefault("=" + simp, (simp, a, d))

# ---------- FreeDict zho-rus ----------
ENTRY = re.compile(r"<entry\b.*?</entry>", re.S)
ORTH  = re.compile(r"<orth>(.*?)</orth>", re.S)
POSR  = re.compile(r"<pos>(.*?)</pos>", re.S)
RU    = re.compile(r'<cit type="trans" xml:lang="ru">(.*?)</cit>', re.S)
QUOTE = re.compile(r"<quote>(.*?)</quote>", re.S)
WIKI  = re.compile(r"\[\[|\]\]")


def clean(t):
    t = WIKI.sub("", t)
    t = re.sub(r"<[^>]+>", "", t)
    return re.sub(r"\s+", " ", t).strip()


fd = {}
s = io.open(os.path.join(RAW, "zho-rus", "zho-rus.tei"), encoding="utf-8").read()
for block in ENTRY.findall(s):
    om = ORTH.search(block)
    if not om:
        continue
    trad = clean(om.group(1))
    pos = clean(POSR.search(block).group(1)) if POSR.search(block) else ""
    g = [clean(q) for cit in RU.findall(block) for q in QUOTE.findall(cit)]
    g = [x for x in g if x]
    if g:
        fd[trad] = (pos, g)

# ---------- RU Wiktionary ----------
rw = {}
p = os.path.join(RAW, "ruwikt-zh-ru.tsv")
if os.path.exists(p):
    with io.open(p, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            c = line.rstrip("\n").split("\t")
            if len(c) >= 6:
                rw[c[0]] = c[1:6]   # simp, trad, pint, pos, ru

# ---------- 词频 ----------
freq = {}
with io.open(os.path.join(RAW, "jieba_dict.txt"), encoding="utf-8") as f:
    for line in f:
        c = line.split()
        if len(c) >= 2 and c[1].isdigit():
            freq[c[0]] = int(c[1])

# ---------- 合并 ----------
rows = {}

def put(simp, trad, a, d, pos, ru):
    if not simp or not CJK.match(simp):
        return
    old = rows.get(simp)
    if old is None:
        rows[simp] = {"trad": trad or simp, "a": a, "d": d, "pos": pos, "ru": list(ru)}
    else:
        if not old["a"] and a:
            old["a"], old["d"] = a, d
        if ru and not old["ru"]:
            old["ru"] = list(ru)
        if pos and not old["pos"]:
            old["pos"] = pos


for trad, (simp, a, d) in ced.items():
    if trad.startswith("="):
        continue
    f = fd.get(trad)
    put(simp, trad, a, d, f[0] if f else "", f[1] if f else [])

for title, (sim, tra, pint, pos, ru) in rw.items():
    key = sim or title
    a, d = "", ""
    src = ced.get(key) or ced.get("=" + key) or (ced.get(tra) if tra else None)
    # 优先用维基词典这一条自己的拼音：多音字（如 都 dou1/du1）才不会被 CC-CEDICT 取错读音
    if pint:
        a, d = parse_pinyin(pint)
    if not a and src:
        a, d = src[1], src[2]
    g = [x.strip() for x in ru.split("|") if x.strip()]
    # 丢掉领域/古旧标记的义项（「(шахматы) королева」「† устар.」），那多半不是常用义
    g = [x for x in g if not x.startswith(("(", "†", "устар", "обл."))]
    put(key, tra or key, a, d, pos, g)
    if g and key in rows:
        rows[key]["ru"] = g   # 俄语维基是直连来源，优先于 FreeDict

# ---------- 枢轴兜底（中->英->俄）: 只补还没有俄语的词 ----------
pivot_n = 0
pp = os.path.join(RAW, "pivot-zh-ru.tsv")
if os.path.exists(pp):
    with io.open(pp, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            c = line.rstrip("\n").split("\t")
            if len(c) < 6 or not c[1] or not c[5]:
                continue
            simp, trad, ru = c[1], c[2], c[5]
            # 单字歧义太大（简繁合并 + 多音字），只信直连来源，宁缺毋滥
            if len(simp) < 2:
                continue
            g = [x.strip() for x in ru.split("|") if x.strip()]
            cur = rows.get(simp)
            if cur is None:
                src = ced.get(simp) or ced.get("=" + simp) or (ced.get(trad) if trad else None)
                a, d = (src[1], src[2]) if src else ("", "")
                rows[simp] = {"trad": trad or simp, "a": a, "d": d, "pos": "", "ru": g}
                pivot_n += 1
            elif not cur["ru"]:
                cur["ru"] = g
                pivot_n += 1

print(f"CC-CEDICT {len(ced)}  FreeDict {len(fd)}  RU-wikt {len(rw)}  枢轴 {pivot_n}  ->  合并 {len(rows)} 条")

out_rows = []
for simp, r in rows.items():
    out_rows.append((simp, r["trad"], r["a"], r["d"], r["pos"], freq.get(simp, 0),
                     " | ".join(r["ru"][:4])))
out_rows.sort(key=lambda x: (-x[5], len(x[0]), x[2]))
with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
    f.write("#simplified\ttraditional\tpinyin_ascii\tpinyin_display\tpos\tfreq\tru\n")
    for r in out_rows:
        if r[2] and r[6]:
            f.write("\t".join(str(x) for x in r) + "\n")
n = sum(1 for _ in io.open(OUT, encoding="utf-8")) - 1
print(f"写出 {OUT}: {n} 条（有拼音 + 有俄语）")

# ---------- 常用词覆盖率 ----------
words = []
with io.open(os.path.join(RAW, "jieba_dict.txt"), encoding="utf-8") as f:
    for line in f:
        c = line.split()
        if len(c) >= 2 and c[1].isdigit() and CJK.match(c[0]):
            words.append((c[0], int(c[1])))
words.sort(key=lambda x: -x[1])
have = {r[0] for r in out_rows if r[6]}
print("\n常用词覆盖率（有俄语译词）:")
for k in (100, 500, 1000, 2000, 3000, 5000, 10000, 20000):
    h = sum(1 for w, _ in words[:k] if w in have)
    print(f"  前 {k:>6}: {h:>6}  {h*100.0/k:>5.1f}%")

print("\n以前查不到、现在有的:")
for w in ("人", "中国", "又", "都", "她", "这", "月", "日", "道"):
    r = rows.get(w)
    if r and r["ru"]:
        print(f"  {w}  [{r['d']}]  {' · '.join(r['ru'][:3])}")
