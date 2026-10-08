#!/usr/bin/env python3
"""Pangyi / 旁译 - demo: type pinyin, get Chinese candidates with Russian glosses."""
import io, os, sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
TSV  = os.path.join(ROOT, "data", "zh-ru.tsv")

POS = {"n": "n.", "v": "v.", "adj": "adj.", "adv": "adv.", "pn": "pn."}

def load():
    exact = {}
    with io.open(TSV, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            p = line.rstrip("\n").split("\t")
            if len(p) < 7:
                continue
            exact.setdefault(p[2], []).append(p)
    return exact

def norm(q):
    q = q.strip().lower().replace("'", "").replace(" ", "")
    return q.replace("u:", "ü").replace("v", "ü")

def render(rows, q, limit=8):
    print()
    exact = rows.get(q, [])
    if exact:
        hits = [(r, True) for r in exact[:limit]]
    else:
        hits = [(r, False) for k, v in sorted(rows.items()) if k.startswith(q)
                for r in v[:2]][:limit]
        if not hits:
            print(f"  (没有匹配 {q})")
            return
    for i, (r, is_exact) in enumerate(hits, 1):
        simp, trad, pa, pd, pos, freq, ru = r
        tag = POS.get(pos.split("/")[0], "")
        ru = " · ".join(ru.split(" | ")[:3])
        print(f"  {i}. {simp:<6} {pd:<12} {tag:<5} {ru}")
    if not exact:
        print("  ^ 前缀匹配，不是完整拼音")

def main():
    rows = load()
    args = sys.argv[1:]
    if args:
        for a in args:
            print(f"> {a}")
            render(rows, norm(a))
        return
    print("旁译 Pangyi - 输入拼音看候选（空行退出）")
    print(f"词条数: {sum(len(v) for v in rows.values())}")
    while True:
        try:
            q = input("\n拼音> ")
        except EOFError:
            break
        if not q.strip():
            break
        render(rows, norm(q))

if __name__ == "__main__":
    main()
