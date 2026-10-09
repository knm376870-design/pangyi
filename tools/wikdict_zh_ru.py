#!/usr/bin/env python3
"""Wikdict 中->俄 词典导出。

数据源：Wikdict（CC-BY-SA 4.0，基于维基词典）
  https://download.wikdict.com/dictionaries/wdweb/zh-ru.sqlite3

用法：
  1) 下载（约 139 MB）到 data/raw/wikdict-zh-ru.sqlite3
  2) python tools/wikdict_zh_ru.py
     -> 写出 data/raw/wikdict-zh-ru.tsv   (written  pinyin  ru  score)

做三件事：
  ① 用 CC-CEDICT 建繁->简映射（Wikdict 的词条是繁体）
  ② 清掉俄语里的维基标记 [[..]] {{..}} 和领域标记（бот./воен./мед.…）
  ③ 用 pypinyin 生成无声调拼音，好让 build_dict 能按拼音建行
"""
import sqlite3, io, re, gzip, os, sys
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'pylibs3'))
from pypinyin import lazy_pinyin
sys.stdout.reconfigure(encoding='utf-8', errors='replace')
CJK = re.compile(r'^[\u4e00-\u9fff]+$')
RAW = r'data\raw'
# 繁->简
word_map, char_map = {}, {}
lr = re.compile(r'^(\S+)\s+(\S+)\s+\[([^\]]*)\]\s+/(.*)/\s*$')
with gzip.open(os.path.join(RAW, 'cedict.txt.gz'), 'rt', encoding='utf-8') as f:
    for line in f:
        if line.startswith('#'): continue
        m = lr.match(line.rstrip('\n'))
        if not m: continue
        trad, simp = m.group(1), m.group(2)
        if CJK.match(trad) and CJK.match(simp):
            word_map.setdefault(trad, simp)
            if len(trad) == 1 and len(simp) == 1: char_map.setdefault(trad, simp)
def to_simp(w):
    return word_map.get(w) or ''.join(char_map.get(ch, ch) for ch in w)
# 俄语清洗：去掉领域标记、纯转写、非西里尔内容
DOMAIN = re.compile(r'^(бот|воен|мед|геогр|зоол|будд|мат|с\.-х|хим|физ|анат|биол|ист|лингв|муз|полит|рел|спорт|театр|тех|экон|юр|устар|обл|диал|сленг|разг|книжн|поэт|фольк|церк|астр|метео|мин|псих|филос|шахм|кулин)\.?$', re.I)
CYR = re.compile(r'[а-яёА-ЯЁ]')
WIKI = re.compile(r'\[\[[^\]]*\]\]|\{\{[^}]*\}\}|\[https?://[^\]]*\]|&[a-z]+;')
def clean_ru(g):
    out = []
    for x in g:
        x = WIKI.sub(' ', x)          # 去掉维基标记 [[..]] {{..}} 和 HTML 实体
        x = re.sub(r'[\[\]{}<>]', ' ', x)
        x = re.sub(r'\s+', ' ', x).strip()
        x = x.strip(' ,;:|·-–—')
        if not x or DOMAIN.match(x): continue
        if not CYR.search(x): continue          # 纯转写/英文/梵文丢掉
        if x.startswith(('(', '†', 'устар', 'обл.', 'перен.')): continue
        if x not in out: out.append(x)
    return out
db = sqlite3.connect(os.path.join(RAW, 'wikdict-zh-ru.sqlite3')); c = db.cursor()
best = {}
for w, tr, score in c.execute('select written_rep, trans_list, max_score from simple_translation'):
    if not w or not tr: continue
    w = w.strip()
    if not CJK.match(w): continue
    s = to_simp(w)
    g = clean_ru([x.strip() for x in tr.split('|')])
    if not g: continue
    sc = int(score or 0)
    if s not in best or sc > best[s][0]:
        best[s] = (sc, g)
print('清洗后可用词:', len(best))
rows = []
for s, (sc, g) in best.items():
    py = ''.join(lazy_pinyin(s)).replace('v', 'ü')
    if not py: continue
    rows.append((s, py, ' | '.join(g[:4]), sc))
rows.sort(key=lambda r: -r[3])
with io.open(os.path.join(RAW, 'wikdict-zh-ru.tsv'), 'w', encoding='utf-8', newline='\n') as f:
    f.write('#written\tpinyin\tru\tscore\n')
    for r in rows: f.write('\t'.join(str(x) for x in r) + '\n')
print('已写出带拼音的 wikdict-zh-ru.tsv:', len(rows), '条')
for r in rows[:6]:
    print('   ', r[0].encode('unicode_escape').decode(), r[1], '->', r[2][:40].encode('unicode_escape').decode()[:60])