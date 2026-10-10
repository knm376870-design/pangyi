//! 编译期生成缩写键表 `src/abbr-keys.tsv`。
//!
//! 每行「键 TAB 行号」—— 行号 = dict.tsv 里该词的行号（1 起，表头不计）。
//! 运行时 index() 只做行号 → 行解析，不再对 29 万行现切拼音：
//! 索引预热从 997 ms 回到 ~350 ms（用户每开一个新进程第一次按键不用卡 1 秒）。
//!
//! 用法（ime 目录下）:
//!   cargo run --release --bin gen_abbr -- src/abbr-keys.tsv
//!
//! ⚠️ 往 dict.tsv 追加词条后必须重跑本程序再构建，否则新词没有缩写键。
//! 生成规则与 index() 的 v4.2.2 运行时版完全一致：按「一个汉字一个音节」把拼音
//! 切开，每个切法生成两种键 —— 首字母键（gongzuo→gz）+ 完整声母键（zhongguo→zhg，
//! zh/ch/sh 当一个声母；没有声母的音节取首字母）。与词典第八列相同的键跳过。
fn main() {
    let out_path = std::env::args()
        .nth(1)
        .expect("用法: gen_abbr.exe 输出路径（如 src/abbr-keys.tsv）");
    let raw = pangyi_ime::dict::raw();
    // 音节表：所有单字词的拼音就是一个个合法音节，直接从词典本身收集
    let mut syll: std::collections::HashSet<&'static str> =
        std::collections::HashSet::with_capacity(512);
    for line in raw.lines().skip(1) {
        let mut it = line.split('\t');
        let word = it.next().unwrap_or("");
        let _trad = it.next();
        let py = it.next().unwrap_or("");
        if word.chars().count() == 1 && !py.is_empty() {
            syll.insert(py);
        }
    }
    let mut out = String::with_capacity(16 << 20);
    let mut keys = 0usize;
    for (i, line) in raw.lines().skip(1).enumerate() {
        let mut it = line.split('\t');
        let word = it.next().unwrap_or("");
        let _trad = it.next();
        let py = it.next().unwrap_or("");
        let mut it8 = line.split('\t');
        let _ = it8.nth(7);
        let ini_key = it8.next().unwrap_or("");
        let wc = word.chars().count();
        if wc >= 2 {
            if let Some(ks) = pangyi_ime::dict::abbr_keys(py, wc, &syll) {
                for k in ks {
                    if k != ini_key {
                        out.push_str(k);
                        out.push('\t');
                        out.push_str(&(i + 1).to_string());
                        out.push('\n');
                        keys += 1;
                    }
                }
            }
        }
    }
    std::fs::write(&out_path, out).expect("写 abbr-keys.tsv 失败");
    println!("{out_path}: {keys} 条缩写键");
}
