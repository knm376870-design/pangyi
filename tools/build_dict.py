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
    # 校验：这个拼音必须真的是这个字的读音（不然会出现"你们/ni"这种错行）
    if pa and sim and (sim, pa) not in rows:
        cands = [k[1] for k in rows.keys() if k[0] == sim]
        if len(cands) == 1:
            pa = cands[0]
        else:
            continue
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

def read_pivot(name, strict=False):
    d = {}
    p = os.path.join(RAW, name)
    if not os.path.exists(p):
        return d
    with io.open(p, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            c = line.rstrip("\n").split("\t")
            if len(c) < 6 or not c[1] or not c[3] or not c[5]:
                continue
            g = [x.strip() for x in c[5].split("|") if x.strip()]
            g = [x for x in g if not x.startswith(("(", "†"))]
            # 只对【扩展表】过滤"词典腔"（+ comp. / см. / чего-л 这些是英俄词典里的
            # 释义占位符，直接塞进候选只会误导）。保守表的译文质量本来就高，不动它。
            if strict:
                BAD = ("+", "см.", "напр.", "и т.", "чего-л", "кем-л", "чем-л", "чему-л", "кому-л")
                g = [x for x in g if not any(b in x for b in BAD)]
            if g:
                d[(c[1], c[3])] = g
    return d

# ---------- CEDICT 英文首义（最后的兜底用） ----------
GLOSS_SKIP = ("CL:", "variant of", "old variant of", "see ", "used in",
              "surname ", "abbr. for", "erhua variant", "also written")
def clean_gloss(defs):
    parts = [x.strip() for x in defs.split("/") if x.strip()]
    parts = [x for x in parts if not x.startswith(GLOSS_SKIP)]
    if not parts:
        return ""
    g = re.sub(r"\([^)]*\)", " ", parts[0])
    g = re.sub(r"\s+", " ", g).strip()
    return g[:44]
ced_gloss = {}
for e in entries:
    k = (e["simp"], e["a"])
    if k in ced_gloss:
        continue
    g = clean_gloss(e["def"])
    if g:
        ced_gloss[k] = g

# ---------- 人工撰写的高频词（优先级最高，先读进来让枢轴避开） ----------
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

# 【顺序很重要】人工表先建行，再由枢轴补其它读音：
#   人工表里的词如果有别的读音没被覆盖（了 le/liao、还 hai/huan），
#   那些读音还是要让枢轴补上；但同一个 (词,拼音) 绝对不许枢轴插手。
# 之前把"人工表建行"放在枢轴之后，导致枢轴先占了读音，人工译文被静默丢弃。
# 人工表里若出现 CEDICT/维基都没有的词（或读音写法对不上），也照样建行 ——
# 人工表是权威来源，不能被静默丢掉。
rows_by_simp = {}
for (s, _a) in rows:
    rows_by_simp.setdefault(s, []).append((s, _a))
manual_only = 0
for (simp, a), g in manual.items():
    if not CJK.match(simp):
        continue
    if (simp, a) in rows:
        continue
    cand = rows_by_simp.get(simp, [])
    if len(cand) == 1:
        # 读音写法不一致（如 jie vs jie2），挂到词典已有的那个读音上
        k = cand[0]
        if not rows[k]["ru"]:
            rows[k]["ru"] = list(g)
            rows[k]["src"] = "manual"
            manual_only += 1
        continue
    d = a
    for e in by_simp.get(simp, []):
        if e["a"] == a:
            d = e["d"]
            break
    rows[(simp, a)] = {"trad": simp, "d": d, "pos": "", "ru": list(g),
                       "src": "manual", "ini": ""}
    manual_only += 1


# 保守表（质量优先）和扩展表（覆盖优先，只当兜底）
pivot_by = read_pivot("pivot-zh-ru.tsv")
pivot_ext_by = read_pivot("pivot-zh-ru-ext.tsv", strict=True)

pivot_n = 0
pivot_ext_n = 0
for (simp, a), rec in rows.items():
    if rec["ru"]:
        continue          # 已有直连译词就不动：实测枢轴在单字上会出「эвен」这类
                          # 更糟的结果，宁可留直连的，也不让它覆盖
    multi = len(simp) >= 2
    # 只收多字词、以及有多个读音的单字。
    # 试过给常用单字也放宽，实测质量不行（的→яркий、开→кипеть、喝→крик 都是错的），
    # 错译比没有更糟，所以回退。
    if not (multi or len(by_simp.get(simp, [])) >= 2):
        continue
    if (simp, a) in manual:
        continue          # 这个读音人工表自己给了译文，枢轴不许覆盖
    g = pivot_by.get((simp, a))
    src = "pivot"
    if not g:
        # 保守表没命中的，才去查扩展表；而且只给多字词用 ——
        # 单字用"拆词匹配"太容易撞同形词（的→новый、can→бидон）。
        # 这样扩展表只会【新增】词条，绝不会改动已有的好译文。
        if not multi:
            continue
        g = pivot_ext_by.get((simp, a))
        src = "pivot-ext"
    if not g:
        continue
    rec["ru"] = g
    rec["src"] = src
    if src == "pivot":
        pivot_n += 1
    else:
        pivot_ext_n += 1

# 人工表在下面「枢轴」之前就解析好：枢轴必须先跳过人工词，
# 否则人工词会被枢轴先填掉，导致人工译文被静默丢弃（见长 就踩过这个坑）。
manual_n = 0

# 每个词有几个读音（后面几处都要用：多音字不能乱贴按词给的译文）
from collections import Counter as _Counter
_reading_count = _Counter(s for (s, _a) in rows)

# ---------- 英文释义兜底 ----------
# 直连语料和两套枢轴都没翻出来的词，至少把 CEDICT 的英文首义带上：
# 这样这些词【能打出来】，而且有个意思可看。前面加 "en: " 标明这不是俄语。
eng_n = 0
for (simp, a), rec in rows.items():
    if rec["ru"]:
        continue
    # 单字也允许，但只限【单读音】的；多音字给英文释义反而误导
    if len(simp) < 2 and _reading_count[simp] > 1:
        continue
    if (simp, a) in manual:
        continue
    g = ced_gloss.get((simp, a))
    if not g:
        continue
    rec["ru"] = ["en: " + g]
    rec["src"] = "english"
    eng_n += 1

# ---------- Wikdict 中->俄（直连，人工编纂，质量高于机器枢轴） ----------
# 来源：Wikdict（CC-BY-SA），从维基词典抽取的中俄翻译，15.5 万词条（已转简体）
wik = {}
wik_py = {}
wp = os.path.join(RAW, "wikdict-zh-ru.tsv")
if os.path.exists(wp):
    with io.open(wp, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            c = line.rstrip("\n").split("\t")
            # written  pinyin  ru  score
            if len(c) >= 3 and c[0] and c[1] and c[2]:
                g = [x.strip() for x in c[2].split("|") if x.strip()]
                if g:
                    wik[c[0]] = g
                    wik_py[c[0]] = c[1]
print(f"Wikdict 中->俄词条: {len(wik)}")

# Wikdict 是按【词】给的，不区分读音。多音字（说 shuō/shuì、重 chóng/zhòng）
# 直接套上去会张冠李戴（说 shuì 被填成"голос"），所以只用于单音词。
# 只填【谁都翻不出来】的空缺：直连语料、枢轴、英文兜底都已经试过了。
# 这样已有译文一条都不会被动，Wikdict 纯粹是净增。
wik_n = 0
for (simp, a), rec in rows.items():
    if rec["ru"]:
        continue
    if _reading_count[simp] > 1:
        continue          # 多音字交给读音感知的数据源
    g = wik.get(simp)
    if not g:
        continue
    rec["ru"] = g[:4]
    rec["src"] = "wikdict"
    wik_n += 1

# Wikdict 里有、我们【完全没有】的词：直接建行
# （拼音是导出时用 pypinyin 预生成的，build_dict 不用额外依赖）
existing_words = {s for (s, _a) in rows}
wik_new = 0
for w, g in wik.items():
    if w in existing_words:
        continue
    py = wik_py.get(w)
    if not py:
        continue
    rows[(w, py)] = {"trad": w, "d": py, "pos": "", "ru": g[:4],
                     "src": "wikdict-new", "ini": ""}
    wik_new += 1

# ---------- 异体字：跟随 "variant of X" 沿用正字的译文 ----------
# CEDICT 里很多生僻字是异体字，释义写成 "variant of 麴|曲[qu1]"，
# 本身没有可翻译的英文，但正字我们多半有译文，直接沿用。
VARIANT_RE = re.compile(r"^(?:old )?variant of ([^\[/|]+)")
ced_var = {}
for e in entries:
    m = VARIANT_RE.match(e["def"])
    if not m:
        continue
    ref = m.group(1).strip()
    if CJK.match(ref):
        ced_var[(e["simp"], e["a"])] = ref

ru_by_word = {}
for (s2, _a2), r2 in rows.items():
    if r2["ru"] and s2 not in ru_by_word:
        ru_by_word[s2] = r2["ru"]

var_n = 0
for (simp, a), rec in rows.items():
    if rec["ru"]:
        continue
    ref = ced_var.get((simp, a))
    if not ref:
        continue
    g = ru_by_word.get(ref)
    if not g:
        continue
    rec["ru"] = list(g)
    rec["src"] = "variant"
    var_n += 1

# ---------- 用 Wikdict 升级机器翻的词 ----------
# 机器枢轴(pivot/pivot-ext)和英文兜底(english)都不可靠；
# 只要 Wikdict 有这个词，就换成人工编纂的译文。
wik_up = 0
for (simp, a), rec in rows.items():
    if rec.get("src") != "english":
        continue          # 只把"英文释义"换成俄语，别动枢轴/直连的结果
    g = wik.get(simp)
    if not g:
        continue
    rec["ru"] = g[:4]
    rec["src"] = "wikdict-up"
    wik_up += 1

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

print(f"CC-CEDICT {len(entries)}（去重读音） FreeDict {len(fd)}  RU-wikt {len(rw)}  Wikdict直连 {wik_n}  Wikdict新建 {wik_new}  枢轴补 {pivot_n}  扩展枢轴兜底 {pivot_ext_n}  英文兜底 {eng_n}  异体字跟随 {var_n}  Wikdict升级 {wik_up}  人工补 {manual_n}（另新建 {manual_only}）")

# 现代常用词加权：jieba 词频对这几个字/词低估（古典语料里反而是另一些更常见），手工校正
FREQ_BOOST = {"帮": 8.0, "给": 2.0, "让": 2.0, "请": 1.5, "邦": 0.15, "浜": 0.05}

out = []
for (simp, a), r in rows.items():
    if r["ru"]:
        _f = int(freq.get(simp, 0) * FREQ_BOOST.get(simp, 1.0))
        out.append((simp, r["trad"], a, r["d"], r["pos"], _f, " | ".join(r["ru"][:4]),
                    r.get("ini", ""), r.get("src", "")))
out.sort(key=lambda x: (-x[5], len(x[0]), x[2]))
with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
    f.write("#simplified\ttraditional\tpinyin_ascii\tpinyin_display\tpos\tfreq\tru\tinitials\tsrc\n")
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
