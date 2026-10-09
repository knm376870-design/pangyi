#!/usr/bin/env python3
"""Pangyi / 旁译 - 词典构建。

关键设计：多音字按「一个读音一条」输出。
  都(dōu) 和 都(dū) 是两条记录 —— 否则只存一条，敲 dou 永远查不到 都。

数据源（全部免费）:
  cedict.txt.gz       CC-CEDICT          CC BY-SA 4.0   繁/简/拼音，一个读音一条
  zho-rus.tei         FreeDict zho-rus   CC BY-SA 3.0   繁 -> 俄语（偏生僻）
  ruwikt-zh-ru.tsv    RU Wiktionary      CC BY-SA 3.0   常用词 -> 俄语（直连，高置信）
  pivot-zh-ru.tsv     中->英->俄枢轴兜底   CC BY-SA      低置信，只用于多字词
  Unihan.zip          Unicode Unihan     Unicode License 单字主读音（兜底）
  jieba_dict.txt      jieba 词频          MIT            候选排序
"""
import gzip, io, os, re, sys, zipfile
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
    """'kai1fa1' / 'kai1 fa1' -> ('kaifa', 'kāifā', 'kf')"""
    pairs = re.findall(r"([A-Za-zü:ê]+)([1-5])", num or "")
    disp = [tone_mark(s, int(d)) for s, d in pairs]
    ini = "".join((s.replace("u:", "ü")[:1]).lower() for s, _ in pairs)
    return "".join(PLAIN.get(c, c) for c in "".join(disp)), "".join(disp), ini


def split_marks(s):
    """Unihan 的 'shàng' -> ('shang', 'shàng')"""
    s = s.strip().lower()
    return "".join(PLAIN.get(c, c) for c in s), s


# ---------- Unihan: 单字主读音 ----------
kman = {}
uz = os.path.join(RAW, "Unihan.zip")
if os.path.exists(uz):
    with zipfile.ZipFile(uz).open("Unihan_Readings.txt") as fh:
        for line in io.TextIOWrapper(fh, encoding="utf-8"):
            if line.startswith("#") or "kMandarin" not in line:
                continue
            cp, k, v = line.rstrip("\n").split("\t")[:3]
            if k == "kMandarin":
                kman[chr(int(cp[2:], 16))] = v.split()

# ---------- CC-CEDICT: 每个读音一条 ----------
entries, seen = [], {}   # (简体, 拼音) -> 在 entries 里的下标
line_re = re.compile(r"^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$")
with gzip.open(os.path.join(RAW, "cedict.txt.gz"), "rt", encoding="utf-8") as f:
    for line in f:
        if line.startswith("#"):
            continue
        m = line_re.match(line.rstrip("\n"))
        if not m:
            continue
        trad, simp, num, defs = m.group(1), m.group(2), m.group(3), m.group(4)
        a, d, ini = parse_pinyin(num)
        if not a:
            continue
        k = (simp, a)
        if k in seen:
            # 同音（不分声调）已经有记录了：优先保留与 Unihan 主读音写法一致的那条。
            # 否则 上 会显示成 shǎng —— CC-CEDICT 里 shǎng 那条先出现。
            i = seen[k]
            cur = entries[i]
            pref = set(kman.get(simp, []))
            if d in pref and cur["d"] not in pref:
                entries[i] = {"simp": simp, "trad": trad, "a": a, "d": d, "ini": ini, "def": defs}
            continue
        seen[k] = len(entries)
        entries.append({"simp": simp, "trad": trad, "a": a, "d": d, "ini": ini, "def": defs})
by_simp = {}
for e in entries:
    by_simp.setdefault(e["simp"], []).append(e)

# ---------- FreeDict zho-rus ----------
ENTRY = re.compile(r"<entry\b.*?</entry>", re.S)
ORTH  = re.compile(r"<orth>(.*?)</orth>", re.S)
POSR  = re.compile(r"<pos>(.*?)</pos>", re.S)
RU    = re.compile(r'<cit type="trans" xml:lang="ru">(.*?)</cit>', re.S)
QUOTE = re.compile(r"<quote>(.*?)</quote>", re.S)
def clean(t):
    t = re.sub(r"\[\[|\]\]", "", t)
    t = re.sub(r"<[^>]+>", "", t)
    return re.sub(r"\s+", " ", t).strip()
fd = {}
s = io.open(os.path.join(RAW, "zho-rus", "zho-rus.tei"), encoding="utf-8").read()
for blk in ENTRY.findall(s):
    om = ORTH.search(blk)
    if not om:
        continue
    trad = clean(om.group(1))
    pos = clean(POSR.search(blk).group(1)) if POSR.search(blk) else ""
    g = [clean(q) for cit in RU.findall(blk) for q in QUOTE.findall(cit)]
    g = [x for x in g if x]
    if g:
        fd[trad] = (pos, g)

# ---------- RU Wiktionary（直连） / 枢轴（兜底） ----------
def read_cache(path):
    d = {}
    if os.path.exists(path):
        with io.open(path, encoding="utf-8") as f:
            for line in f:
                if line.startswith("#"):
                    continue
                c = line.rstrip("\n").split("\t")
                if len(c) >= 6:
                    d[c[0]] = c[1:6]
    return d
rw    = read_cache(os.path.join(RAW, "ruwikt-zh-ru.tsv"))
pivot = read_cache(os.path.join(RAW, "pivot-zh-ru.tsv"))  # 兼容旧读法

# ---------- 词频 ----------
freq = {}
with io.open(os.path.join(RAW, "jieba_dict.txt"), encoding="utf-8") as f:
    for line in f:
        c = line.split()
        if len(c) >= 2 and c[1].isdigit():
            freq[c[0]] = int(c[1])

# ---------- 合并：key = (简体, 拼音) ----------
rows = {}


def emit(simp, trad, a, d, pos="", ru=None, ini=""):
    if not simp or not a or not CJK.match(simp):
        return
    key = (simp, a)
    cur = rows.get(key)
    if cur is None:
        rows[key] = {"trad": trad or simp, "d": d, "pos": pos, "ru": list(ru or []), "ini": ini}
        return cur if cur else rows[key]
    if pos and not cur["pos"]:
        cur["pos"] = pos
    if ru and not cur["ru"]:
        cur["ru"] = list(ru)
    return cur


# 1) CC-CEDICT 每个读音一条 + FreeDict 直连
for e in entries:
    f = fd.get(e["trad"])
    emit(e["simp"], e["trad"], e["a"], e["d"], f[0] if f else "", f[1] if f else [], e.get("ini", ""))

# 2) 俄语维基（直连）—— 覆盖到该词的每个读音上
for title, (sim, tra, pint, pos, ru) in rw.items():
    simp = sim or title
    if not simp:
        continue
    g = [x.strip() for x in ru.split("|") if x.strip()]
    g = [x for x in g if not x.startswith(("(", "†", "устар", "обл."))]
    if not g:
        continue
    if pint:
        # 直连来源自带读音 —— 只贴这一个读音，别污染别的读音
        pa, pd, _pi = parse_pinyin(pint)
        reads = [(pa, pd, tra or simp)] if pa else []
    else:
        reads = [(e["a"], e["d"], e["trad"]) for e in by_simp.get(simp, [])]
        if not reads and len(simp) == 1:
            reads = [(split_marks(k)[0], split_marks(k)[1], tra or simp)
                     for k in kman.get(simp, [])]
    for a, d, tr in reads:
        cur = emit(simp, tr, a, d, pos)
        if cur:
            cur["ru"] = g
            cur["src"] = "pint" if pint else "rw"

# 3) 枢轴（中->英->俄），按读音：
#    - 多字词：正常补
#    - 单字：只有当这个字有多个读音时才用（用来区分读音，不做无中生有）
# 最常用的 3000 个词（含单字），用于放宽单字枢轴
TOP_COMMON = set()
with io.open(os.path.join(RAW, "jieba_dict.txt"), encoding="utf-8") as f:
    _ws = []
    for line in f:
        c = line.split()
        if len(c) >= 2 and c[1].isdigit() and CJK.match(c[0]):
            _ws.append((c[0], int(c[1])))
_ws.sort(key=lambda x: -x[1])
TOP_COMMON = {w for w, _ in _ws[:3000]}

pivot_by = {}
pp = os.path.join(RAW, "pivot-zh-ru.tsv")
if os.path.exists(pp):
    with io.open(pp, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            c = line.rstrip("\n").split("\t")
            if len(c) < 6 or not c[1] or not c[3] or not c[5]:
                continue
            g = [x.strip() for x in c[5].split("|") if x.strip()]
            g = [x for x in g if not x.startswith(("(", "†"))]
            if g:
                pivot_by[(c[1], c[3])] = g

pivot_n = 0
for (simp, a), rec in rows.items():
    if rec["ru"]:
        continue          # 已有直连译词就不动：实测枢轴在单字上会出「эвен」这类
                          # 更糟的结果，宁可留直连的，也不让它覆盖
    g = pivot_by.get((simp, a))
    if not g:
        continue
    # 只收多字词、以及有多个读音的单字。
    # 试过给常用单字也放宽，实测质量不行（的→яркий、开→кипеть、喝→крик 都是错的），
    # 错译比没有更糟，所以回退。
    if len(simp) >= 2 or len(by_simp.get(simp, [])) >= 2:
        rec["ru"] = g
        rec["src"] = "pivot"
        pivot_n += 1

# ---------- 人工撰写的高频词（助手逐条写，优先于枢轴；直连语料优先于人工） ----------
manual = {}
mp = os.path.join(RAW, "manual-zh-ru.tsv")
if os.path.exists(mp):
    with io.open(mp, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            c = line.rstrip("\n").split("\t")
            if len(c) >= 3 and c[0] and c[1] and c[2]:
                manual[(c[0], c[1])] = [x.strip() for x in c[2].split(";") if x.strip()]
manual_n = 0
for (simp, a), rec in rows.items():
    g = manual.get((simp, a))
    if not g:
        continue
    # 人工表永远优先：实测直连语料在这些高频词上也会错
    # （他→другие、很→непослушание、看→думать、会→собрание）
    rec["ru"] = g
    rec["src"] = "manual"
    manual_n += 1

print(f"CC-CEDICT {len(entries)}（去重读音） FreeDict {len(fd)}  RU-wikt {len(rw)}  枢轴补 {pivot_n}  人工补 {manual_n}")

out = []
for (simp, a), r in rows.items():
    if r["ru"]:
        out.append((simp, r["trad"], a, r["d"], r["pos"], freq.get(simp, 0), " | ".join(r["ru"][:4]), r.get("ini", "")))
out.sort(key=lambda x: (-x[5], len(x[0]), x[2]))
with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
    f.write("#simplified\ttraditional\tpinyin_ascii\tpinyin_display\tpos\tfreq\tru\tinitials\n")
    for r in out:
        f.write("\t".join(str(x) for x in r) + "\n")
print(f"写出 {OUT}: {len(out)} 条")

# ---------- 覆盖率 ----------
words = []
with io.open(os.path.join(RAW, "jieba_dict.txt"), encoding="utf-8") as f:
    for line in f:
        c = line.split()
        if len(c) >= 2 and c[1].isdigit() and CJK.match(c[0]):
            words.append((c[0], int(c[1])))
words.sort(key=lambda x: -x[1])
have = {r[0] for r in out}
print("\n常用词覆盖率:")
for k in (100, 1000, 5000, 10000):
    h = sum(1 for w, _ in words[:k] if w in have)
    print(f"  前 {k:>6}: {h:>6}  {h*100.0/k:>5.1f}%")

print("\n多音字现在有几个读音（敲不同拼音都能出）:")
for w in ("都", "地", "的", "着", "说", "上", "行", "长", "乐"):
    rs = [(a, r["d"]) for (s, a), r in rows.items() if s == w and r["ru"]]
    if rs:
        print(f"  {w}: " + "  ".join(f"{a}/{d}" for a, d in rs))
    else:
        print(f"  {w}: （无）")
