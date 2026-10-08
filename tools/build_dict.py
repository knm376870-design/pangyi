#!/usr/bin/env python3
"""Pangyi / 旁译 - dictionary builder.

Joins three free datasets into one table:
  raw/cedict.txt.gz            CC-CEDICT          (CC BY-SA 4.0)  trad/simp/pinyin
  raw/zho-rus/zho-rus.tei      FreeDict zho-rus   (CC BY-SA 3.0)  trad -> Russian
  raw/jieba_dict.txt           jieba word freq    (MIT)           ranking

Output: data/zh-ru.tsv
  simplified \t traditional \t pinyin_ascii \t pinyin_display \t pos \t freq \t ru
"""
import gzip, io, os, re, sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
RAW  = os.path.join(ROOT, "data", "raw")
OUT  = os.path.join(ROOT, "data", "zh-ru.tsv")

TONE = {"a":"āáǎà","e":"ēéěè","i":"īíǐì","o":"ōóǒò","u":"ūúǔù","ü":"ǖǘǚǜ"}
PLAIN = {c: v for v, s in TONE.items() for c in s}
VOWELS = "aeiouü"

SYL_RE = re.compile(r"^([A-Za-zü:ê]+)([0-9])?$")

def tone_mark(syl, tone):
    syl = syl.replace("u:", "ü").replace("v", "ü").lower()
    if tone is None or tone == 5:
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
    if c not in TONE:
        return syl
    return syl[:i] + TONE[c][tone - 1] + syl[i + 1:]

def parse_pinyin(num):
    disp, ascii_parts = [], []
    for tok in num.split():
        tok = tok.strip()
        if not tok:
            continue
        m = SYL_RE.match(tok)
        if not m:
            disp.append(tok); ascii_parts.append(tok.lower()); continue
        syl, d = m.group(1), m.group(2)
        t = int(d) if d else 5
        marked = tone_mark(syl, t)
        disp.append(marked)
        ascii_parts.append("".join(PLAIN.get(c, c) for c in marked))
    return "".join(ascii_parts), "".join(disp)

def load_cedict(path):
    """trad -> (simp, pinyin_ascii, pinyin_display)."""
    out = {}
    line_re = re.compile(r"^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$")
    with gzip.open(path, "rt", encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            m = line_re.match(line.rstrip("\n"))
            if not m:
                continue
            trad, simp, num = m.group(1), m.group(2), m.group(3)
            a, d = parse_pinyin(num)
            out.setdefault(trad, (simp, a, d))
    return out

ENTRY = re.compile(r"<entry\b.*?</entry>", re.S)
ORTH  = re.compile(r"<orth>(.*?)</orth>", re.S)
POS   = re.compile(r"<pos>(.*?)</pos>", re.S)
RU    = re.compile(r'<cit type="trans" xml:lang="ru">(.*?)</cit>', re.S)
QUOTE = re.compile(r"<quote>(.*?)</quote>", re.S)
WIKI  = re.compile(r"\[\[|\]\]")

def clean(txt):
    txt = WIKI.sub("", txt)
    txt = re.sub(r"<[^>]+>", "", txt)
    return re.sub(r"\s+", " ", txt).strip()

def load_zhorus(path):
    """trad -> (pos, [russian...])."""
    s = io.open(path, encoding="utf-8").read()
    out = {}
    for block in ENTRY.findall(s):
        om = ORTH.search(block)
        if not om:
            continue
        trad = clean(om.group(1))
        pos = ""
        pm = POS.search(block)
        if pm:
            pos = clean(pm.group(1))
        ru = []
        for cit in RU.findall(block):
            for q in QUOTE.findall(cit):
                q = clean(q)
                if q and q not in ru:
                    ru.append(q)
        if ru:
            prev = out.get(trad)
            if prev:
                if pos and pos not in prev[0].split("/"):
                    prev[0] = (prev[0] + "/" + pos).strip("/")
                for g in ru:
                    if g not in prev[1]:
                        prev[1].append(g)
            else:
                out[trad] = [pos, ru]
    return out

def load_freq(path):
    """word(simplified) -> frequency."""
    freq = {}
    with io.open(path, encoding="utf-8") as f:
        for line in f:
            p = line.split()
            if len(p) >= 2 and p[1].isdigit():
                freq[p[0]] = int(p[1])
    return freq

def main():
    ced = load_cedict(os.path.join(RAW, "cedict.txt.gz"))
    ru  = load_zhorus(os.path.join(RAW, "zho-rus", "zho-rus.tei"))
    frq = load_freq(os.path.join(RAW, "jieba_dict.txt"))
    print(f"cedict entries : {len(ced)}")
    print(f"zho-rus entries: {len(ru)}")

    rows, matched = [], 0
    for trad, (simp, a, d) in ced.items():
        r = ru.get(trad)
        if not r:
            continue
        matched += 1
        pos, glosses = r
        f = frq.get(simp, 0)
        rows.append((simp, trad, a, d, pos, f, " | ".join(glosses[:4])))

    print(f"joined         : {matched} ({matched*100.0/max(1,len(ced)):.1f}% of cedict)")
    rows.sort(key=lambda r: (-r[5], len(r[0]), r[2]))
    with io.open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("#simplified\ttraditional\tpinyin_ascii\tpinyin_display\tpos\tfreq\tru\n")
        for r in rows:
            f.write("\t".join([r[0], r[1], r[2], r[3], r[4], str(r[5]), r[6]]) + "\n")
    print(f"wrote          : {OUT}  ({len(rows)} rows)")

    idx = {}
    for r in rows:
        idx.setdefault(r[2], []).append(r)
    for probe in ("kaifa", "diannao", "xuexi", "zhongguo", "shijian"):
        hits = idx.get(probe, [])
        print(f"\n  {probe}  -> {len(hits)} candidates")
        for h in hits[:5]:
            print(f"     {h[0]}  [{h[3]}]  {h[6][:70]}")

if __name__ == "__main__":
    main()
