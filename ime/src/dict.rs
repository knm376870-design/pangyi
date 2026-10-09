//! 词典：嵌入的「拼音 -> 中文候选 + 俄语译词」表。
//!
//! 索引：精确（HashMap）+ 前缀（排序数组二分）+ 首字母缩写（zg → 中国）。
//! 另外会记住你选过的词（%LOCALAPPDATA%\Pangyi\learned.txt），下次把它排到前面。
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

static RAW: &str = include_str!("dict.tsv");

pub struct Hit {
    pub word: String,
    pub pinyin: String,
    pub ru: String,
}

/// 扁平行表：(key, 词条行)，按 key 升序排好，同一个 key 的多行相邻。
type Row = (&'static str, &'static str);

struct Index {
    /// (拼音, 词条行)，按拼音升序。
    /// 原来是 HashMap<&str, Vec<&str>>：6 万个 key 各带一个 Vec，
    /// 哈希桶加上每个 Vec 自己的堆分配差不多要 7MB，而内容本身只要 2MB。
    exact: Vec<Row>,
    /// (首字母缩写, 词条行)，按缩写升序
    ini: Vec<Row>,
    /// 1~2 个字母的前缀 -> 该前缀下词频最高的那条
    /// （最多 26 + 26*26 = 702 条，用来避免每敲一个键都去扫几千个拼音键）
    comp: HashMap<&'static str, &'static str>,
    learned: Mutex<HashMap<String, u32>>,
    total_freq: f64,
}

fn learned_file() -> String {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    format!("{base}\\Pangyi\\learned.txt")
}

fn load_learned() -> HashMap<String, u32> {
    let mut m = HashMap::new();
    if let Ok(s) = std::fs::read_to_string(learned_file()) {
        for line in s.lines() {
            let mut it = line.split('\t');
            if let (Some(w), Some(n)) = (it.next(), it.next()) {
                if let Ok(v) = n.trim().parse::<u32>() {
                    if !w.is_empty() {
                        m.insert(w.to_string(), v);
                    }
                }
            }
        }
    }
    m
}

/// 记住用户选的词，下次排前面。
pub fn note_choice(word: &str) {
    if word.is_empty() {
        return;
    }
    let idx = index();
    let mut g = idx.learned.lock().unwrap();
    let e = g.entry(word.to_string()).or_insert(0);
    *e = e.saturating_add(1);
    let path = learned_file();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body: String = g.iter().map(|(w, c)| format!("{w}\t{c}\n")).collect();
    let _ = std::fs::write(path, body);
}

fn index() -> &'static Index {
    static IDX: OnceLock<Index> = OnceLock::new();
    IDX.get_or_init(|| {
        let mut total_freq = 0.0f64;
        let mut exact: Vec<Row> = Vec::with_capacity(64000);
        let mut ini: Vec<Row> = Vec::with_capacity(64000);
        for line in RAW.lines().skip(1) {
            // 一条记录 8 列，边遍历边取，不 collect
            let mut it = line.split('\t');
            let _simp = it.next();
            let _trad = it.next();
            let py = match it.next() { Some(s) => s, None => continue };
            let _disp = it.next();
            let _pos = it.next();
            let _freq = it.next();
            let _ru = it.next();
            let ini_key = it.next().unwrap_or("");
            if py.is_empty() {
                continue;
            }
            exact.push((py, line));
            total_freq += line_freq(line) as f64;
            if !ini_key.is_empty() {
                ini.push((ini_key, line));
            }
        }
        // 稳定排序：同一个 key 内部保持词典原顺序（词典本身已按词频排好）
        exact.sort_by(|a, b| a.0.cmp(b.0));
        ini.sort_by(|a, b| a.0.cmp(b.0));
        // 预先算好 1~2 字母前缀的最佳补全。
        // 没有这张表的话，敲 "z" 要遍历所有以 z 开头的拼音键（几千个），
        // 每敲一个字母就要多花 1ms 左右 —— 这就是之前"打字卡"的主因之一。
        // 遍历扁平行表，按 key 分组求该组里得分最高的那行
        let mut comp: HashMap<&'static str, &'static str> = HashMap::new();
        let mut i = 0usize;
        while i < exact.len() {
            let k = exact[i].0;
            let mut j = i;
            let mut best: Option<&'static str> = None;
            let mut best_score = 0u64;
            while j < exact.len() && exact[j].0 == k {
                let sc = line_score(exact[j].1);
                if best.is_none() || sc > best_score {
                    best_score = sc;
                    best = Some(exact[j].1);
                }
                j += 1;
            }
            if let Some(b) = best {
                let klen = k.chars().count();
                if klen >= 2 {
                    for plen in [1usize, 2usize] {
                        if klen == plen {
                            continue;   // 和前缀等长的键属于"精确匹配"，不算补全
                        }
                        let pref = prefix_chars(k, plen);
                        let cur = comp.entry(pref).or_insert(b);
                        if best_score > line_score(*cur) {
                            *cur = b;
                        }
                    }
                }
            }
            i = j;
        }

        Index { exact, ini, comp, learned: Mutex::new(load_learned()), total_freq: total_freq.max(1.0) }
    })
}

/// 补全用的打分：词频为主，长度做微调
/// 取前 n 个「字符」组成的切片。
/// 注意拼音里有 ü，是多字节字符，绝不能按字节下标切。
fn prefix_chars(k: &str, n: usize) -> &str {
    match k.char_indices().nth(n) {
        Some((i, _)) => &k[..i],
        None => k,
    }
}

fn line_score(line: &str) -> u64 {
    line_freq(line) * 10 + line_word(line).chars().count() as u64
}

fn hit_of(line: &'static str) -> Option<Hit> {
    // 直接用迭代器取需要的列：原来 split().collect() 会给每一行分配一个 Vec，
    // 而这里每敲一个字母要被调用几十次
    let mut it = line.split('\t');
    let word = it.next()?;
    let _trad = it.next()?;
    let _py = it.next()?;
    let disp = it.next()?;
    let _pos = it.next()?;
    let _freq = it.next()?;
    let ru = it.next().unwrap_or("");
    Some(Hit { word: word.into(), pinyin: disp.into(), ru: ru.into() })
}

/// 取 exact 里 key 完全相等的连续区间（表是按 key 排好序的）
fn exact_group<'a>(idx: &'a Index, key: &str) -> &'a [Row] {
    let a = idx.exact.partition_point(|(k, _)| *k < key);
    let b = idx.exact.partition_point(|(k, _)| *k <= key);
    &idx.exact[a..b]
}

fn push_lines(rows: &[Row], out: &mut Vec<Hit>, cap: usize) {
    for (_, line) in rows {
        if let Some(h) = hit_of(line) {
            out.push(h);
        }
        if out.len() >= cap * 3 {
            return;
        }
    }
}

/// 按无声调拼音精确/前缀查找；也支持首字母缩写（zg → 中国）。
pub fn lookup(ascii: &str) -> Vec<Hit> {
    if ascii.is_empty() {
        return Vec::new();
    }
    let idx = index();
    let mut out: Vec<Hit> = Vec::new();

    // 1) 精确
    push_lines(exact_group(idx, ascii), &mut out, 16);
    // 2) 前缀（二分定位）
    if out.len() < 16 {
        let start = idx.exact.partition_point(|(k, _)| *k < ascii);
        for (key, line) in idx.exact[start..].iter() {
            if !key.starts_with(ascii) {
                break;
            }
            if *key == ascii {
                continue;
            }
            if let Some(h) = hit_of(line) {
                out.push(h);
            }
            if out.len() >= 16 {
                break;
            }
        }
    }
    // 3) 首字母缩写（输入全是字母且不长时才用）
    if out.len() < 9 && ascii.len() >= 2 && ascii.len() <= 6 {
        let start = idx.ini.partition_point(|(k, _)| *k < ascii);
        for (key, line) in idx.ini[start..].iter() {
            if !key.starts_with(ascii) {
                break;
            }
            if let Some(h) = hit_of(line) {
                out.push(h);
            }
            if out.len() >= 16 {
                break;
            }
        }
    }

    out.truncate(16);

    // 自定义短语优先（dz → 电子邮箱）
    if let Some(h) = phrase(ascii) {
        out.insert(0, h);
    }

    // 整句候选放最前面（打字长的拼音时）
    // 整句切词现在只用词典词频，效果不稳（会出错的句子），先不进候选；
    // 等接上真正的语料 bigram 语言模型再打开。
    if ascii.chars().count() >= 5 {
        if let Some(s) = sentence_hit(ascii) {
            if s.word.chars().count() >= 2 {
                out.insert(0, s);
            }
        }
    }

    // 4) 自学习：选过的词排前面
    let learned = idx.learned.lock().unwrap();
    if !learned.is_empty() {
        out.sort_by_key(|h| {
            let c = learned.get(&h.word).copied().unwrap_or(0);
            std::cmp::Reverse(c)
        });
    }
    // 候选窗一页 9 个，留 3 页足够；再多的候选每敲一个键都要被克隆好几遍，
    // 对体验没有任何帮助，纯浪费。
    out.truncate(27);
    out
}


// ---------------- 自定义短语（讯飞那种"打几个字母出长句"） ----------------
//
// 文件：%LOCALAPPDATA%\Pangyi\phrases.txt
// 每行：触发字母 <TAB> 中文 <TAB> 俄语(可省)
fn phrases_file() -> String {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    format!("{base}\\Pangyi\\phrases.txt")
}

fn ensure_phrases_file() {
    let path = phrases_file();
    if std::path::Path::new(&path).exists() {
        return;
    }
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let demo = "# 自定义短语：每行「触发字母 TAB 中文 TAB 俄语(可省)」\n# 打 dz 第一个就是「电子邮箱」。改完保存立刻生效。\ndz\t电子邮箱\tэлектронная почта\nwxh\t微信号\tWeChat ID\n";
    let _ = std::fs::write(path, demo);
}

/// 精确匹配用户自定义短语
pub fn phrase(trigger: &str) -> Option<Hit> {
    ensure_phrases_file();
    let s = std::fs::read_to_string(phrases_file()).ok()?;
    for line in s.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() < 2 {
            continue;
        }
        if c[0].trim().eq_ignore_ascii_case(trigger) {
            let word = c[1].trim().to_string();
            let ru = c.get(2).map(|x| x.trim().to_string()).unwrap_or_default();
            return Some(Hit { word, pinyin: trigger.to_string(), ru });
        }
    }
    None
}

/// 补全：在「拼音以 ascii 开头、且比它更长」的词条里，挑最高频的那个。
pub fn complete(ascii: &str) -> Option<(String, String)> {
    if ascii.is_empty() {
        return None;
    }
    let idx = index();
    // 1~2 个字母：直接查预算表，O(1)，不用扫几千个键
    if ascii.chars().count() <= 2 {
        if let Some(line) = idx.comp.get(ascii) {
            let mut it = line.split('\t');
            let w = it.next().unwrap_or("").to_string();
            let d = it.nth(2).unwrap_or("").to_string();
            return Some((w, d));
        }
        // 表里没有（比如带 ü 的拼音）就照旧走扫描，行为不变
    }
    let start = idx.exact.partition_point(|(k, _)| *k < ascii);
    let mut best: Option<(u64, &'static str)> = None;
    for (key, line) in idx.exact[start..].iter() {
        if !key.starts_with(ascii) {
            break;
        }
        if *key == ascii {
            continue;
        }
        // 只取第 0 列（词）和第 5 列（词频），不再为每行分配 Vec
        let mut it = line.split('\t');
        let word = it.next().unwrap_or("");
        let freq: u64 = match it.nth(4).and_then(|s| s.parse().ok()) {
            Some(f) => f,
            None => continue,
        };
        let score = freq * 10 + word.chars().count() as u64;
        if best.map(|(s, _)| score > s).unwrap_or(true) {
            best = Some((score, line));
        }
    }
    best.map(|(_, line)| {
        let mut it = line.split('\t');
        let w = it.next().unwrap_or("").to_string();
        let d = it.nth(2).unwrap_or("").to_string();
        (w, d)
    })
}

// ---------------- 语言模型（词 bigram，语料训练） ----------------
static LM_RAW: &str = include_str!("lm.tsv");
static LM_UNI_RAW: &str = include_str!("lm_uni.tsv");

type UniMap = std::collections::HashMap<&'static str, u32>;

fn lm_uni_total() -> f64 {
    static T: OnceLock<f64> = OnceLock::new();
    *T.get_or_init(|| lm_uni().values().map(|v| *v as f64).sum::<f64>().max(1.0))
}

fn lm_uni() -> &'static UniMap {
    static U: OnceLock<UniMap> = OnceLock::new();
    U.get_or_init(|| {
        let mut m = UniMap::default();
        for line in LM_UNI_RAW.lines() {
            let mut it = line.split('\t');
            if let (Some(a), Some(b)) = (it.next(), it.next()) {
                if let Ok(n) = b.trim().parse::<u32>() {
                    m.insert(a, n);
                }
            }
        }
        m
    })
}

type LmMap = std::collections::HashMap<(&'static str, &'static str), u32>;

fn lm() -> &'static LmMap {
    static LM: OnceLock<LmMap> = OnceLock::new();
    LM.get_or_init(|| {
        let mut m = LmMap::default();
        for line in LM_RAW.lines() {
            let mut it = line.split('\t');
            if let (Some(a), Some(b), Some(c)) = (it.next(), it.next(), it.next()) {
                if let Ok(n) = c.trim().parse::<u32>() {
                    m.insert((a, b), n);
                }
            }
        }
        m
    })
}

/// 两个词连在一起出现过多少次（语言模型的核心）
fn bigram_count(a: &str, b: &str) -> u32 {
    lm().get(&(a, b)).copied().unwrap_or(0)
}
// ---------------- 整句转换（词典频次 + 动态规划 + 语料 bigram） ----------------
//
// 讯飞/微软拼音那种"打一整句拼音、直接出句子"。
// 这里不用神经网络，用词频做动态规划切词——离线、快、够用。

fn line_freq(line: &str) -> u64 {
    line.split('\t').nth(5).and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn line_word(line: &str) -> &str {
    line.split('\t').next().unwrap_or("")
}

fn line_ru(line: &str) -> &str {
    line.split('\t').nth(6).unwrap_or("")
}

/// 把一串无声调拼音切成词，返回最优切分（至少两个词才算整句）。
pub fn sentence(pinyin: &str) -> Option<Vec<Hit>> {
    if pinyin.chars().count() < 3 {
        return None;
    }
    let idx = index();
    let chars: Vec<char> = pinyin.chars().collect();
    let n = chars.len();
    // best[i] = (得分, 切分结果)
    let mut best: Vec<Option<(f64, Vec<&'static str>)>> = vec![None; n + 1];
    best[0] = Some((0.0, Vec::new()));
    for i in 0..n {
        let cur = match best[i].as_ref() {
            Some(c) => c.clone(),
            None => continue,
        };
        for len in 1..=(n - i).min(14) {
            let key: String = chars[i..i + len].iter().collect();
            let lines = exact_group(idx, key.as_str());
            if lines.is_empty() {
                continue;
            }
            // 同一个拼音可能有多个词（如 shijie → 世纪/世界），全都要试，
            // 交给动态规划按上下文挑最好的，而不是这里就定死。
            let mut alts: Vec<&'static str> = lines.iter().map(|(_, l)| *l).collect();
            alts.sort_by_key(|l| std::cmp::Reverse(line_freq(l)));
            for line in alts.into_iter().take(6) {
            let freq = line_freq(line).max(1) as f64;
            let wlen = line_word(line).chars().count() as f64;
            // 打分（离线词模型：词频 + 长度先验 + 单字惩罚）
            //   ln(词频)      —— 常用词得分高
            //   + 12*(字数-1) —— 长词强偏好
            //   - 8（单字）   —— 单字词频被"作为字的频率"抬高了，必须压一压，
            //                    否则 jintiantianqihenhao 会被切成"进/提/按..."
            // 语言模型（log 空间，避免两个概率分布量纲不一致）：
            //   log P(词)  = 0.75*log(词典频率占比) + 0.25*log(语料频率占比)
            //   log P(词|上一个词) = 0.8*log(搭配概率) + 0.2*log P(词)（没见过搭配再打折）
            let prev = cur.1.last().map(|l| line_word(l)).unwrap_or("");
            let word = line_word(line);
            let cu = lm_uni().get(word).copied().unwrap_or(0) as f64;
            let log_dict = (freq / idx.total_freq).max(1e-12).ln();
            let log_corpus = if cu > 0.0 { (cu / lm_uni_total()).max(1e-12).ln() } else { log_dict - 3.0 };
            let log_p_uni = 0.9 * log_dict + 0.1 * log_corpus;
            let log_p = if prev.is_empty() {
                log_p_uni
            } else {
                let u = lm_uni().get(prev).copied().unwrap_or(0) as f64;
                let b = bigram_count(prev, word) as f64;
                if u > 0.0 && b > 0.0 {
                    0.8 * (b / u).ln() + 0.2 * log_p_uni
                } else {
                    log_p_uni - 3.0
                }
            };
            let score = cur.0 + log_p
                + 2.5 * (wlen - 1.0)
                - if wlen <= 1.0 { 5.0 } else { 0.0 }
                - if freq < 120.0 { 6.0 } else { 0.0 };
            let end = i + len;
            if best[end].as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                let mut seq = cur.1.clone();
                seq.push(line);
                best[end] = Some((score, seq));
            }
            }
        }
    }
    let (_, seq) = best[n].as_ref()?;
    if seq.len() < 2 {
        return None;
    }
    Some(seq.iter().filter_map(|l| hit_of(l)).collect())
}

/// 整句候选：整句本身 + 每个词的俄语（拼在一起，长了会截断）
pub fn sentence_hit(pinyin: &str) -> Option<Hit> {
    let parts = sentence(pinyin)?;
    if parts.len() < 2 {
        return None;
    }
    let word: String = parts.iter().map(|h| h.word.clone()).collect();
    let mut rus: Vec<String> = Vec::new();
    for h in &parts {
        for g in h.ru.split('|').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            if !rus.iter().any(|x| x == g) {
                rus.push(g.to_string());
            }
            if rus.len() >= 4 {
                break;
            }
        }
        if rus.len() >= 4 {
            break;
        }
    }
    Some(Hit { word, pinyin: pinyin.to_string(), ru: rus.join(" | ") })
}
/// 预热：把词典索引 + 语言模型一次性建好。
///
/// 这两块都是「每个进程一份」的重量级结构（词典 6 万条、bigram 20 万条）。
/// 老版本是在用户打到第 3 个字母时才临时构建，那一下会明显卡顿，
/// 而每换一个程序都要重新来一遍。现在改成激活时丢到后台线程先建好。
pub fn warmup() {
    let _ = lookup("kaifa");
    let _ = complete("kaif");
    let _ = sentence("nihaoshijie");
}

/// (词库条数, 语言模型 bigram 条数)，给设置面板显示用。
/// 结果缓存一次：设置面板每次重绘都会问，别每帧都扫 6MB 字符串。
pub fn stats() -> (usize, usize) {
    static S: OnceLock<(usize, usize)> = OnceLock::new();
    *S.get_or_init(|| {
        (RAW.lines().count().saturating_sub(1), LM_RAW.lines().count())
    })
}
