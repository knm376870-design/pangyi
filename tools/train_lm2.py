# -*- coding: utf-8 -*-
"""训练语言模型 v2（词 bigram）—— 流式版，能吃 zhwiki 主库大文件。

与 train_lm.py 的区别：
  * 流式解压 + 滚动尾巴，不整读入内存（主库解压后有 ~8GB 文本）
  * 词表 = data/zh-ru.tsv ∪ ime/src/dict.tsv（扩容后的全词表）
  * bigram 按「前词 → 后词 Counter」计数，超内存上限时每词截顶，
    最后每词取 top-40（count>=3），全局按次数截 --keep-pairs 条
  * 只处理前 --max-gb GB 解压文本（默认 1.5），够训练出百万级搭配

用法:
  cd tools && python -I train_lm2.py            # 全量（小库 + 主库前 1.5GB）
  python -I train_lm2.py --max-gb 0.3           # 快速试跑
输出：ime/src/lm.tsv / lm_uni.tsv（旧文件先备份成 *.bak-414）
"""
import bz2, io, os, re, sys, collections
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW = os.path.join(ROOT, "data", "raw")
OUT = os.path.join(ROOT, "ime", "src", "lm.tsv")
OUT_UNI = os.path.join(ROOT, "ime", "src", "lm_uni.tsv")

MAX_WORD = 6
CJK = re.compile(r"[一-鿿]")
TEXT = re.compile(r"<text[^>]*>(.*?)</text>", re.S)

MAX_GB = 1.5
if "--max-gb" in sys.argv:
    MAX_GB = float(sys.argv[sys.argv.index("--max-gb") + 1])
KEEP_PAIRS = 1000000
UNI_KEEP = 300000
PER_WORD = 40
PAIR_GUARD = 5000000     # 总词对数超此值时，每词截顶
PER_WORD_PRUNE = 150     # 截顶时每词保留多少个后继

# ---- 1) 分词词表：zh-ru.tsv ∪ dict.tsv ----
words = set()
for path in (os.path.join(ROOT, "data", "zh-ru.tsv"),
             os.path.join(ROOT, "ime", "src", "dict.tsv")):
    with io.open(path, encoding="utf-8") as f:
        next(f, None)
        for line in f:
            w = line.split("\t")[0]
            if 1 <= len(w) <= MAX_WORD and CJK.match(w):
                words.add(w)
for ch in "的一是了我不人在他有这个上们来到时大地为子中你说生国年着就那和要她出也得里后自以会家可下而过天去能对小多然于心学么之都好看起发当没成只如事把还用第样道想作种开美总从无情己面最女但现前些所同日手又行意动方期它头经长儿回位分爱老因很给名法间斯知世什两次使身者被高已亲其进此话常与活正感":
    words.add(ch)
bylen = collections.defaultdict(set)
for w in words:
    bylen[len(w)].add(w)
print(f"分词词表 {len(words)} 个", flush=True)

def segment(text):
    out = []
    i = 0
    n = len(text)
    while i < n:
        hit = None
        for L in range(min(MAX_WORD, n - i), 0, -1):
            cand = text[i:i + L]
            if cand in bylen[L]:
                hit = cand
                break
        if hit:
            out.append(hit)
            i += len(hit)
        else:
            i += 1
    return out

def clean(t):
    t = t.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
    t = re.sub(r"\{\{[^{}]*\}\}", "", t)
    t = re.sub(r"\[\[(?:[^\]|]*\|)?([^\]]*)\]\]", r"\1", t)
    t = re.sub(r"<[^>]+>", "", t)
    t = re.sub(r"[^一-鿿]", "", t)
    return t

unigram = collections.Counter()
succ = collections.defaultdict(collections.Counter)
docs = 0
processed = 0.0
LIMIT = MAX_GB * 1024.0 * 1024.0 * 1024.0

def prune():
    n = 0
    for w, c in succ.items():
        if len(c) > PER_WORD_PRUNE:
            keep = c.most_common(PER_WORD_PRUNE)
            c.clear()
            c.update(keep)
            n += 1
    return n

CARRY = 4 * 1024 * 1024  # 滚动尾巴：<text> 块跨 chunk 边界也抓得到

for name in sorted(os.listdir(RAW)):
    if not name.endswith(".xml.bz2"):
        continue
    path = os.path.join(RAW, name)
    print(f"读 {name}（{os.path.getsize(path)/1e6:.0f} MB）", flush=True)
    f = bz2.BZ2File(path, "rb")
    carry = ""
    chunk_no = 0
    while processed < LIMIT:
        try:
            chunk = f.read(8 * 1024 * 1024)
        except EOFError:
            # 下载没下完 / 文件截断：bz2 流提前结束。已解出的部分照用，
            # 别让整个训练白跑（v4.14.0 主库下载中断时的兜底）。
            print(f"  压缩流提前结束（{name} 可能不完整），用已解出的部分继续", flush=True)
            break
        if not chunk:
            break
        chunk_no += 1
        buf = carry + chunk.decode("utf-8", errors="replace")
        if len(buf) > CARRY:
            carry = buf[-CARRY:]
        else:
            carry = buf
        for raw in TEXT.findall(buf):
            txt = clean(raw)
            if len(txt) < 30:
                continue
            docs += 1
            toks = segment(txt)
            processed += len(raw)
            unigram.update(toks)
            prev = None
            for t in toks:
                if prev is not None:
                    succ[prev][t] += 1
                prev = t
        total_pairs = sum(len(c) for c in succ.values())
        if total_pairs > PAIR_GUARD:
            k = prune()
            total_pairs = sum(len(c) for c in succ.values())
            print(f"  截顶 {k} 词（总词对 {total_pairs//1000}k，文本 {processed/1e9:.2f} GB）", flush=True)
        if chunk_no % 64 == 0:
            print(f"  进度 {processed/1e9:.2f} GB / {MAX_GB:.1f} GB，文档 {docs}，词对 {sum(len(c) for c in succ.values())//1000}k", flush=True)
    f.close()
    if processed >= LIMIT:
        print(f"达到上限 {MAX_GB} GB，停止读入", flush=True)
        break

# ---- 2) 出表：每词 top-PER_WORD（count>=3），全局按次数截 KEEP_PAIRS ----
pairs = []
for w, c in succ.items():
    for w2, n in c.most_common(PER_WORD):
        if n >= 3:
            pairs.append((w, w2, n))
pairs.sort(key=lambda x: -x[2])
pairs = pairs[:KEEP_PAIRS]
import shutil
for p in (OUT, OUT_UNI):
    if os.path.exists(p):
        shutil.copy2(p, p + ".bak-414")
with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
    for a, b, c in pairs:
        f.write(f"{a}\t{b}\t{c}\n")
print(f"写出 {OUT}: {len(pairs)} 条 bigram，{os.path.getsize(OUT)/1e6:.1f} MB", flush=True)
with io.open(OUT_UNI, "w", encoding="utf-8", newline="\n") as f:
    for w, c in unigram.most_common(UNI_KEEP):
        if c >= 2:
            f.write(f"{w}\t{c}\n")
print(f"写出 {OUT_UNI}: {os.path.getsize(OUT_UNI)/1e6:.1f} MB", flush=True)
print("完成", flush=True)
