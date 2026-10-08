#!/usr/bin/env python3
"""量一下：按词频排序的常用词里，有多少已经拿到俄语译词。"""
import gzip, io, os, re, sys
sys.stdout.reconfigure(encoding="utf-8", errors="replace")

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW  = os.path.join(ROOT, "data", "raw")

# CC-CEDICT: simplified -> traditional
simp2trad = {}
trad2simp = {}
line_re = re.compile(r"^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$")
with gzip.open(os.path.join(RAW, "cedict.txt.gz"), "rt", encoding="utf-8") as f:
    for line in f:
        if line.startswith("#"):
            continue
        m = line_re.match(line.rstrip("\n"))
        if not m:
            continue
        trad, simp = m.group(1), m.group(2)
        simp2trad.setdefault(simp, trad)
        trad2simp.setdefault(trad, simp)

# zho-rus headwords
s = io.open(os.path.join(RAW, "zho-rus", "zho-rus.tei"), encoding="utf-8").read()
orths = set(re.findall(r"<orth>(.*?)</orth>", s))
print(f"CC-CEDICT 词条: {len(simp2trad)}    zho-rus 词头: {len(orths)}")
print(f"两边交集(直接按繁体): {len(set(trad2simp) & orths)}")

# jieba word frequencies
CJK = re.compile(r"^[\u4e00-\u9fff]+$")
words = []
with io.open(os.path.join(RAW, "jieba_dict.txt"), encoding="utf-8") as f:
    for line in f:
        p = line.split()
        if len(p) >= 2 and p[1].isdigit() and CJK.match(p[0]):
            words.append((p[0], int(p[1])))
words.sort(key=lambda x: -x[1])
print(f"jieba 纯中文词条: {len(words)}")

print("\n按常用度取前 N 个词，看有多少能出俄语:")
print(f"{'N':>7} {'有俄语':>8} {'占比':>7}   说明")
for n in (100, 500, 1000, 2000, 3000, 5000, 10000, 20000):
    seg = words[:n]
    hit = sum(1 for w, _ in seg if simp2trad.get(w) in orths)
    print(f"{n:>7} {hit:>8} {hit*100.0/n:>6.1f}%")

print("\n最常用的 200 个词里缺俄语的（前 40 个）:")
miss = [w for w, _ in words[:200] if simp2trad.get(w) not in orths]
print("  " + " ".join(miss[:40]))
print(f"  ... 共 {len(miss)} / 200 缺")

print("\n最常用 5000 里缺俄语的（随机抽 40 个）:")
miss5 = [w for w, _ in words[:5000] if simp2trad.get(w) not in orths]
print("  " + " ".join(miss5[::max(1, len(miss5)//40)][:40]))
print(f"  ... 共 {len(miss5)} / 5000 缺")
