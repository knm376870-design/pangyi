#!/usr/bin/env python3
"""给语言模型抓语料：从中文维基百科抓正文纯文本，攒够就停。

输出 data/raw/corpus.txt（每行一段），供 train_lm.py 统计 bigram。
"""
import io, json, os, re, sys, time, urllib.parse, urllib.request

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
RAW = os.path.join(os.path.dirname(HERE), "data", "raw")
OUT = os.path.join(RAW, "corpus.txt")
API = "https://zh.wikipedia.org/w/api.php"
UA = "Pangyi/1.0 (offline zh-ru IME prototype; local user)"

TARGET_MB = float(sys.argv[1]) if len(sys.argv) > 1 else 30.0

def api(params):
    q = dict(params)
    q["format"] = "json"
    q["formatversion"] = "2"
    req = urllib.request.Request(API + "?" + urllib.parse.urlencode(q), headers={"User-Agent": UA})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r)
        except Exception as e:
            if attempt == 4:
                return None
            time.sleep(2 * (attempt + 1))
    return None

def clean(t):
    t = re.sub(r"<[^>]+>", "", t)
    t = re.sub(r"\{\{[^{}]*\}\}", "", t)
    t = re.sub(r"\[\[(?:[^\]|]*\|)?([^\]]*)\]\]", r"\1", t)
    t = re.sub(r"[A-Za-z0-9_#*:;()\[\]{}<>/\\|=+~^%$@!?\"'`]", "", t)
    t = re.sub(r"\s+", "", t)
    return t

def main():
    seen = 0
    got = 0
    fo = io.open(OUT, "a", encoding="utf-8", newline="\n")
    if not os.path.exists(OUT):
        fo.write("# 中文语料（维基百科摘要），用于统计词 bigram\n")
    while os.path.getsize(OUT) < TARGET_MB * 1024 * 1024:
        d = api({
            "action": "query", "generator": "random", "grnnamespace": 0, "grnlimit": 50,
            "prop": "extracts", "explaintext": 1, "exintro": 1, "exlimit": 50,
        })
        pages = (d or {}).get("query", {}).get("pages", [])
        if not pages:
            time.sleep(2)
            continue
        for p in pages:
            seen += 1
            txt = clean(p.get("extract") or "")
            txt = re.sub(r"[^\u4e00-\u9fff]", "", txt)
            if len(txt) >= 40:
                fo.write(txt + "\n")
                got += 1
        fo.flush()
        mb = os.path.getsize(OUT) / 1024 / 1024
        print(f"看了 {seen} 篇，收了 {got} 段，语料 {mb:.1f} MB", flush=True)
    fo.close()
    print("语料抓取完成", flush=True)

main()