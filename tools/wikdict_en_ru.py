#!/usr/bin/env python3
"""Wikdict 英->俄 词典导出（给中->英->俄枢轴用）。

数据源：Wikdict（CC-BY-SA 4.0，基于维基词典）
  https://download.wikdict.com/dictionaries/wdweb/en-ru.sqlite3

用法：
  1) 下载（约 83 MB）到 data/raw/wikdict-en-ru.sqlite3
  2) python tools/wikdict_en_ru.py
     -> 写出 data/raw/wikdict-en-ru.tsv  (english  ru1 | ru2 | ...)

比原来的 FreeDict eng-rus（5.7 万词条）大得多（9.5 万），
之前查不到的常见释义（mix / begin / extinguish / caterpillar）现在都有了。
"""
import sqlite3, io, re, sys
sys.stdout.reconfigure(encoding='utf-8', errors='replace')
db = sqlite3.connect(r'data\raw\wikdict-en-ru.sqlite3'); c = db.cursor()
CYR = re.compile(r'[а-яёА-ЯЁ]')
WIKI = re.compile(r'\[\[[^\]]*\]\]|\{\{[^}]*\}\}')
DOMAIN = re.compile(r'^(бот|воен|мед|геогр|зоол|будд|мат|с\.-х|хим|физ|анат|биол|ист|лингв|муз|полит|рел|спорт|театр|тех|экон|юр|устар|обл|диал|сленг|разг|книжн)\.?$', re.I)
def clean_ru(g):
    out = []
    for x in g:
        x = WIKI.sub(' ', x)
        x = re.sub(r'[\u005b\u005d{}<>]', ' ', x)   # 残留的方括号/花括号碎片
        x = re.sub(r'\s+', ' ', x).strip()
        x = x.strip(' ,;:|·—-–')
        if not x or DOMAIN.match(x) or not CYR.search(x): continue
        if x.startswith(('(', '†')): continue
        if x not in out: out.append(x)
    return out
rows = {}
for w, tr in c.execute('select written_rep, trans_list from simple_translation'):
    if not w or not tr: continue
    # 词头也清一遍：有些条目的 written_rep 是 "ania]]" / "burkina faso|kaya]]" 这种带残留标记的
    w = re.sub(r'[\u005b\u005d{}<>]', '', w)
    w = w.split('|')[0].strip().lower()
    if not w or len(w) > 40: continue
    g = clean_ru([x.strip() for x in tr.split('|')])
    if g: rows[w] = g
print('Wikdict 英俄词条:', len(rows))
with io.open(r'data\raw\wikdict-en-ru.tsv', 'w', encoding='utf-8', newline='\n') as f:
    f.write('#english\tru\n')
    for w, g in rows.items():
        f.write(f'{w}\t' + ' | '.join(g[:6]) + '\n')
print('已写出 data/raw/wikdict-en-ru.tsv')
for k in ['water','house','quick','mix','begin','extinguish','pekingese','caterpillar']:
    print(f'   {k:<14} {str(rows.get(k))[:70]}')