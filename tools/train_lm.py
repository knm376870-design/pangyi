#!/usr/bin/env python3
"""训练语言模型（词 bigram）：用我们的词典分词，统计词与词的搭配。

输入：data/raw/zhwiki*.xml.bz2（维基导出）
输出：ime/src/lm.tsv  （w1 \t w2 \t count，已剪枝）
"""
import bz2, io, os, re, sys, collections
sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW = os.path.join(ROOT, "data", "raw")
OUT = os.path.join(ROOT, "ime", "src", "lm.tsv")
OUT_UNI = os.path.join(ROOT, "ime", "src", "lm_uni.tsv")

MAX_WORD = 6
CJK = re.compile(r"[\u4e00-\u9fff]")

# 1) 词典词表（用于正向最大匹配分词）
words = set()
with io.open(os.path.join(ROOT, "data", "zh-ru.tsv"), encoding="utf-8") as f:
    next(f)
    for line in f:
        w = line.split("\t")[0]
        if 1 <= len(w) <= MAX_WORD and CJK.match(w):
            words.add(w)
# 单字也都要有
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
            cand = text[i:i+L]
            if cand in words:
                hit = cand
                break
        if hit:
            out.append(hit)
            i += len(hit)
        else:
            i += 1
    return out

# 2) 抽取正文
TEXT = re.compile(r"<text[^>]*>(.*?)</text>", re.S)
def clean(t):
    t = t.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
    t = re.sub(r"\{\{[^{}]*\}\}", "", t)
    t = re.sub(r"\[\[(?:[^\]|]*\|)?([^\]]*)\]\]", r"\1", t)
    t = re.sub(r"<[^>]+>", "", t)
    t = re.sub(r"[^\u4e00-\u9fff]", "", t)
    return t

unigram = collections.Counter()
bigram = collections.Counter()
docs = 0
for name in sorted(os.listdir(RAW)):
    if not name.endswith(".xml.bz2"):
        continue
    path = os.path.join(RAW, name)
    print(f"读 {name}", flush=True)
    with bz2.open(path, "rt", encoding="utf-8", errors="replace") as f:
        buf = f.read()
    for raw in TEXT.findall(buf):
        txt = clean(raw)
        if len(txt) < 30:
            continue
        docs += 1
        toks = segment(txt)
        unigram.update(toks)
        bigram.update(zip(toks, toks[1:]))
    print(f"  文档 {docs} 篇，单字/词 {len(unigram)}，bigram {len(bigram)}", flush=True)

# 3) 剪枝：bigram 至少出现 2 次，最多留 20 万条
items = [(a, b, c) for (a, b), c in bigram.items() if c >= 2]
items.sort(key=lambda x: -x[2])
items = items[:200000]
os.makedirs(os.path.dirname(OUT), exist_ok=True)
with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
    for a, b, c in items:
        f.write(f"{a}\t{b}\t{c}\n")
mb = os.path.getsize(OUT) / 1024 / 1024
print(f"写出 {OUT}: {len(items)} 条 bigram，{mb:.1f} MB", flush=True)
# 语料词频（算 P(w|prev) 要用）
with io.open(OUT_UNI, "w", encoding="utf-8", newline="\n") as f:
    for w, c in unigram.most_common(60000):
        f.write(f"{w}\t{c}\n")
print(f"写出 {OUT_UNI}: {min(len(unigram), 60000)} 个词频", flush=True)
