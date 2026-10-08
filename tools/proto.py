#!/usr/bin/env python3
"""Pangyi / 旁译 - 界面原型（阶段 1）

模拟一个输入法：上面是"任意软件"的输入位置，敲拼音时候选条浮在光标下方，
每个候选旁边带一条俄语译词。空格或数字上屏，Esc 取消。

    python tools/proto.py          # 打开
    python tools/proto.py --demo   # 预置一段拼音，方便截图
"""
import io, os, sys, tkinter as tk
from tkinter import font as tkfont

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
TSV  = os.path.join(ROOT, "data", "zh-ru.tsv")

BG, BAR_BG, BAR_SEL = "#1b2130", "#2a3245", "#3b4a6b"
FG, DIM, ACCENT = "#e9ecf3", "#8792a8", "#7aa2f7"
PINYIN_C, RU_C = "#9ece6a", "#c3a6ff"
MAX_CAND = 9


def load_index(path):
    exact = {}
    with io.open(path, encoding="utf-8") as f:
        for line in f:
            if line.startswith("#"):
                continue
            p = line.rstrip("\n").split("\t")
            if len(p) < 7:
                continue
            exact.setdefault(p[2], []).append(p)
    return exact


def pick(fams, *names):
    for n in names:
        if n in fams:
            return n
    return "TkDefaultFont"


class Pangyi:
    def __init__(self, root, index):
        self.root, self.index = root, index
        self.buf, self.start, self.cands, self.sel = "", None, [], 0

        fams = set(tkfont.families(root))
        self.f_cjk  = pick(fams, "Microsoft YaHei UI", "Microsoft YaHei", "SimHei")
        self.f_ui   = pick(fams, "Segoe UI", "Microsoft YaHei UI")
        self.f_mono = pick(fams, "Consolas", "Courier New")

        root.title("旁译 Pangyi - 界面原型")
        root.configure(bg=BG)
        root.geometry("900x560+80+80")

        head = tk.Frame(root, bg=BG); head.pack(fill="x", padx=18, pady=(14, 0))
        tk.Label(head, text="旁译", font=(self.f_cjk, 15, "bold"), fg=FG, bg=BG).pack(side="left")
        tk.Label(head, text="   中 → 俄 · 界面原型", font=(self.f_ui, 10), fg=DIM,
                 bg=BG).pack(side="left")
        tk.Label(root, text="下面这个框模拟任意软件的输入位置 —— 敲拼音试试。",
                 font=(self.f_ui, 9), fg=DIM, bg=BG, anchor="w").pack(fill="x", padx=18, pady=(6, 4))

        wrap = tk.Frame(root, bg="#111722", highlightthickness=1, highlightbackground="#2f3a52")
        wrap.pack(fill="both", expand=True, padx=18, pady=(0, 8))
        self.text = tk.Text(wrap, bg="#111722", fg=FG, insertbackground=ACCENT, relief="flat",
                            padx=14, pady=12, wrap="word", font=(self.f_cjk, 15),
                            selectbackground="#33405c")
        self.text.pack(fill="both", expand=True)
        self.text.tag_configure("comp", foreground=PINYIN_C, underline=True)

        tk.Label(root, text="空格 / 数字 上屏    ·    Backspace 改拼音    ·    Esc 取消",
                 font=(self.f_ui, 9), fg=DIM, bg=BG, anchor="w").pack(fill="x", padx=18, pady=(0, 12))

        self.bar = tk.Toplevel(root)
        self.bar.overrideredirect(True)
        self.bar.attributes("-topmost", True)
        self.bar.configure(bg="#3b4a6b")
        self.bar_in = tk.Frame(self.bar, bg=BAR_BG)
        self.bar_in.pack(padx=1, pady=1)
        self.bar.withdraw()

        self.text.bind("<Key>", self.on_key)
        self.text.focus_set()

    def lookup(self, q):
        if not q:
            return []
        out = list(self.index.get(q, []))
        if len(out) < MAX_CAND:
            for k in sorted(self.index):
                if k != q and k.startswith(q):
                    out.extend(self.index[k][:1])
                if len(out) >= MAX_CAND:
                    break
        return out[:MAX_CAND]

    def refresh(self):
        self.cands, self.sel = self.lookup(self.buf), 0
        for w in self.bar_in.winfo_children():
            w.destroy()
        if not self.cands:
            self.bar.withdraw(); return
        for i, r in enumerate(self.cands):
            simp, trad, pa, pd, pos, freq, ru = r
            gl = ru.split(" | ")[0]
            bg = BAR_SEL if i == self.sel else BAR_BG
            row = tk.Frame(self.bar_in, bg=bg, padx=10, pady=6)
            row.pack(side="left")
            tk.Label(row, text=str(i + 1), font=(self.f_mono, 10),
                     fg=ACCENT if i == 0 else DIM, bg=bg).pack(side="left", padx=(0, 7))
            tk.Label(row, text=simp, font=(self.f_cjk, 13, "bold"), fg=FG, bg=bg).pack(side="left")
            tk.Label(row, text="  " + gl[:26], font=(self.f_ui, 10), fg=RU_C,
                     bg=bg).pack(side="left")
        self.place_bar()

    def place_bar(self):
        self.bar.update_idletasks()
        try:
            bbox = self.text.bbox("insert")
        except tk.TclError:
            bbox = None
        if not bbox:
            self.bar.withdraw(); return
        x = self.text.winfo_rootx() + bbox[0]
        y = self.text.winfo_rooty() + bbox[1] + bbox[3] + 6
        self.bar.geometry("+%d+%d" % (x, y))
        self.bar.deiconify(); self.bar.lift()

    def clear(self):
        self.buf, self.start = "", None
        self.bar.withdraw()

    def on_key(self, e):
        ch, ks = e.char, e.keysym
        if ks == "Escape" and self.buf:
            self.text.delete(self.start, "insert"); self.clear(); return "break"
        if ks == "BackSpace" and self.buf:
            self.buf = self.buf[:-1]
            self.text.delete("insert-1c", "insert")
            if self.buf:
                self.refresh()
            else:
                self.start = None; self.bar.withdraw()
            return "break"
        if ch and ch.isascii() and ch.isalpha():
            if not self.buf:
                self.start = self.text.index("insert")
            self.buf += ch.lower()
            self.text.insert("insert", ch.lower(), "comp")
            self.refresh(); return "break"
        if self.buf and (ks == "space" or (ch.isdigit() and ch != "0")):
            i = (int(ch) - 1) if ch.isdigit() else self.sel
            if 0 <= i < len(self.cands):
                self.commit(self.cands[i][0])
            return "break"
        if ks in ("Return", "KP_Enter") and self.buf:
            if self.cands:
                self.commit(self.cands[0])
            return "break"
        if ch:
            self.clear()
        return None

    def commit(self, word):
        self.text.delete(self.start, "insert")
        self.text.insert("insert", word)
        self.clear()
        self.text.focus_set()

    def type_str(self, s):
        for c in s:
            if not self.buf:
                self.start = self.text.index("insert")
            self.buf += c
            self.text.insert("insert", c, "comp")
        self.refresh()


def main():
    root = tk.Tk()
    app = Pangyi(root, load_index(TSV))
    app.text.insert("1.0", "我在学中文，今天想说的是：")
    app.text.mark_set("insert", "end")
    if "--demo" in sys.argv:
        root.after(500, lambda: app.type_str("kaifa"))
    root.mainloop()


if __name__ == "__main__":
    main()
