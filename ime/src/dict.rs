//! 词典：嵌入的「拼音 -> 中文候选 + 俄语译词」表。
//!
//! 索引：精确（HashMap）+ 前缀（排序数组二分）+ 首字母缩写（zg → 中国）。
//! 另外会记住你选过的词（%LOCALAPPDATA%\Pangyi\learned.txt），下次把它排到前面。
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

static RAW: &str = include_str!("dict.tsv");

/// 编译期生成的缩写键表（gen_abbr 产出）：每行「键 TAB 行号」，
/// 行号 = dict.tsv 里该词的行号（1 起，表头不计）。
/// 原本在 index() 里对 29 万行现切拼音生成（预热 312→997 ms、私有内存 30→80 MB，
/// 用户每开一个新进程第一次按键就卡 1 秒），挪到编译期后预热回到 ~350 ms。
/// ⚠️ 往 dict.tsv 追加词条后必须重跑 gen_abbr 再构建，否则新词没有缩写键。
static ABBR: &str = include_str!("abbr-keys.tsv");

/// dict.tsv 原文（给 gen_abbr 用；词典本体只能纯追加，不能重建 —— 见存档技术笔记）
pub fn raw() -> &'static str {
    RAW
}

pub struct Hit {
    pub word: String,
    pub pinyin: String,
    pub ru: String,
    /// 词典词频（来自 jieba）。用于同音词重排。
    pub freq: u64,
    /// 来源置信度分层（排序降权用）：
    /// 0 = 直连语料 / 人工表；1 = 机器枢轴扩展；2 = 只有英文释义
    pub tier: u8,
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
    /// 1~2 个字母的前缀 -> 该前缀下（拼音比前缀更长的）词频最高的前 200 条。
    /// 打 "y" 时不再按拼音字母序先给一堆 ya 系词，而是全 y 系里最高频的词；
    /// 打 "yi" 时精确组翻到底还能接着翻到 yichang（一场）/yijing（已经）这些
    /// 更长读音的词 —— 「像微软那样翻页看到全部」靠的就是这张表。
    prefix_top: HashMap<&'static str, Vec<(&'static str, u64)>>,
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
        // 行向量：行号 → 行内容。缩写键表（ABBR）里存的就是这里的行号，
        // 这样不用把 29 万行再复制一份进二进制。
        let lines: Vec<&'static str> = RAW.lines().skip(1).collect();
        let mut exact: Vec<Row> = Vec::with_capacity(64000);
        let mut ini: Vec<Row> = Vec::with_capacity(320000);
        for line in &lines {
            // 一条记录 8 列，边遍历边取，不 collect
            let mut it = line.split('\t');
            let _simp = it.next().unwrap_or("");
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
            exact.push((py, *line));
            // ü 的替代拼法：中文输入法里一律用 v 打 ü（nv→女、lvxing→旅行、falv→法律）。
            // 词典里存的是 ü，不建这条别名的话这 500 多个词永远敲不出来。
            if py.contains('ü') {
                let alt: &'static str = Box::leak(py.replace('ü', "v").into_boxed_str());
                exact.push((alt, *line));
            }
            total_freq += line_freq(*line) as f64;
            if !ini_key.is_empty() {
                ini.push((ini_key, *line));
            }
        }
        // 首字母缩写键（v4.2.2）：词典第八列只覆盖 12 万条，还有 17 万条
        // 多字词没有缩写键（两个/最大/很多…… 敲 lg/hd 出不来）。这些键
        // 由 gen_abbr 在编译期生成（ABBR 表），index() 只做「行号 → 行」解析。
        // 生成规则见 gen_abbr.rs：按「一个汉字一个音节」切拼音，每词两种键 ——
        //   首字母键    gongzuo→gz、zhongguo→zg
        //   完整声母键  zhongguo→zhg（zh/ch/sh 算一个声母；an/er 这类
        //               没有声母的音节就取首字母，方案 fang|an → fa）
        for line in ABBR.lines() {
            if let Some((k, n)) = line.split_once('\t') {
                if let Ok(i) = n.parse::<usize>() {
                    if let Some(l) = lines.get(i.wrapping_sub(1)) {
                        ini.push((k, l));
                    }
                }
            }
        }
        // 稳定排序：同一个 key 内部保持词典原顺序（词典本身已按词频排好）
        exact.sort_by(|a, b| a.0.cmp(b.0));
        ini.sort_by(|a, b| a.0.cmp(b.0));
        // 预先算好 1~2 字母前缀的最佳补全。
        // 没有这张表的话，敲 "z" 要遍历所有以 z 开头的拼音键（几千个），
        // 每敲一个字母就要多花 1ms 左右 —— 这就是之前"打字卡"的主因之一。
        // 遍历扁平行表，按 key 分组求该组里得分最高的那行。
        // 同一遍里顺手把 1~2 字母前缀的高频词表也建了（见 Index::prefix_top）。
        let mut comp: HashMap<&'static str, &'static str> = HashMap::new();
        let mut prefix_top: HashMap<&'static str, Vec<(&'static str, u64)>> = HashMap::new();
        let mut i = 0usize;
        while i < exact.len() {
            let k = exact[i].0;
            let klen = k.chars().count();
            let mut j = i;
            let mut best: Option<&'static str> = None;
            let mut best_score = 0u64;
            while j < exact.len() && exact[j].0 == k {
                let line = exact[j].1;
                let sc = line_score(line);
                if best.is_none() || sc > best_score {
                    best_score = sc;
                    best = Some(line);
                }
                // 只收「拼音比前缀更长」的行：和前缀等长的属于"精确匹配"，
                // 由 lookup 的精确分支负责，别重复算
                if klen > 1 {
                    prefix_top.entry(prefix_chars(k, 1)).or_default().push((line, sc));
                }
                if klen > 2 {
                    prefix_top.entry(prefix_chars(k, 2)).or_default().push((line, sc));
                }
                j += 1;
            }
            if let Some(b) = best {
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
        // 每个前缀只留分最高的前 200 条；shrink_to_fit 把建表时的
        // 临时大桶（打 "y" 的那个桶曾装下两万多条）真正还回去
        for v in prefix_top.values_mut() {
            v.sort_by(|a, b| b.1.cmp(&a.1));   // 稳定：同分保持词典原顺序
            v.truncate(PREFIX_TOP);
            v.shrink_to_fit();
        }

        Index {
            exact,
            ini,
            comp,
            prefix_top,
            learned: Mutex::new(load_learned()),
            total_freq: total_freq.max(1.0),
        }
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

/// 把词拼音切成「一个字一个音节」，每个切法生成缩写键（见 index() 里调用处）。
/// 切不出（词典音节表覆盖不到的读音）返回 None；最多收 3 种切法。
pub fn abbr_keys(py: &'static str, want: usize, syll: &HashSet<&'static str>) -> Option<Vec<&'static str>> {
    if want < 2 || py.chars().count() < want {
        return None;
    }
    let chars: Vec<char> = py.chars().collect();
    let mut segs: Vec<Vec<&'static str>> = Vec::with_capacity(3);
    let mut cur: Vec<&'static str> = Vec::with_capacity(want);
    seg_dfs(&chars, 0, want, syll, &mut cur, &mut segs);
    if segs.is_empty() {
        return None;
    }
    let mut keys: Vec<&'static str> = Vec::with_capacity(segs.len() * 2);
    for seg in segs {
        let mut a = String::new(); // 首字母键
        let mut b = String::new(); // 完整声母键
        for s in &seg {
            let c0 = match s.chars().next() {
                Some(c) if c.is_ascii_lowercase() => c,
                // ü/ǹ 开头的音节键盘敲不出来，这种切法的键不要
                _ => {
                    a.clear();
                    b.clear();
                    break;
                }
            };
            a.push(c0);
            if s.starts_with("zh") || s.starts_with("ch") || s.starts_with("sh") {
                b.push_str(&s[..2]);
            } else {
                b.push(c0);
            }
        }
        if a.is_empty() || b.is_empty() {
            continue;
        }
        for k in [a, b] {
            if !keys.iter().any(|x| *x == k) {
                keys.push(Box::leak(k.into_boxed_str()));
            }
        }
    }
    if keys.is_empty() {
        None
    } else {
        Some(keys)
    }
}

/// 回溯切分：把 chars[pos..] 切成「剩余音节数」个音节。每个音节 1~6 个字符
/// （最长的 zhuang 是 6），必须在音节表里。找到 3 种切法就停。
fn seg_dfs(
    chars: &[char],
    pos: usize,
    want: usize,
    syll: &HashSet<&'static str>,
    cur: &mut Vec<&'static str>,
    out: &mut Vec<Vec<&'static str>>,
) {
    if out.len() >= 3 {
        return;
    }
    let rem_syl = want - cur.len();
    let rem_ch = chars.len() - pos;
    if rem_syl == 0 {
        if pos == chars.len() {
            out.push(cur.clone());
        }
        return;
    }
    // 每个音节至少 1 个字符、至多 6 个：先剪掉必然失败的枝
    if rem_ch < rem_syl || rem_ch > rem_syl * 6 {
        return;
    }
    let maxl = (rem_ch - (rem_syl - 1)).min(6); // 给后面的音节各留至少 1 字符
    for len in 1..=maxl {
        let cand: String = chars[pos..pos + len].iter().collect();
        if !syll.contains(cand.as_str()) {
            continue;
        }
        let leaked: &'static str = Box::leak(cand.into_boxed_str());
        cur.push(leaked);
        seg_dfs(chars, pos + len, want, syll, cur, out);
        cur.pop();
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
    let freq: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let ru = it.next().unwrap_or("");
    let _ini = it.next();
    let src = it.next().unwrap_or("");
    let tier = match src {
        "pivot-ext" => 1,
        "english" => 2,
        _ => 0,
    };
    Some(Hit { word: word.into(), pinyin: disp.into(), ru: ru.into(), freq, tier })
}

/// 取 exact 里 key 完全相等的连续区间（表是按 key 排好序的）
fn exact_group<'a>(idx: &'a Index, key: &str) -> &'a [Row] {
    let a = idx.exact.partition_point(|(k, _)| *k < key);
    let b = idx.exact.partition_point(|(k, _)| *k <= key);
    &idx.exact[a..b]
}

/// 一次查找最多返回多少候选 —— 也就是翻页的总深度。
/// 词库里最大的一组读音（yi）有 557 条，600 能装下整组还有余量。
const MAX_HITS: usize = 600;
/// 1~2 字母前缀的高频表每个前缀留多少条（见 Index::prefix_top）
const PREFIX_TOP: usize = 200;
/// 3 个字母及以上的前缀，顺序扫最多多少条
const PREFIX_SCAN: usize = 200;

/// 同一个词（候选文本）只留一条。
/// 词典里同一个词可能有多行（Unihan 一条带译文 + nogloss 一条没译文，
/// 补全的时候没合并干净）。候选少的时候看不出来，一翻页就露馅：
/// 第 5 页出现两个一模一样的「槸」。优先留有俄语译文的，其次词频高的。
fn dedup_words(out: &mut Vec<Hit>) {
    if out.len() < 2 {
        return;
    }
    let mut best: HashMap<&str, usize> = HashMap::with_capacity(out.len());
    for (i, h) in out.iter().enumerate() {
        match best.get(h.word.as_str()) {
            None => {
                best.insert(h.word.as_str(), i);
            }
            Some(&j) => {
                let cur_ru = !out[j].ru.is_empty();
                let new_ru = !h.ru.is_empty();
                if (new_ru && !cur_ru) || (new_ru == cur_ru && h.freq > out[j].freq) {
                    best.insert(h.word.as_str(), i);
                }
            }
        }
    }
    if best.len() == out.len() {
        return; // 本来就没有重复，不用再扫一遍
    }
    let keep: std::collections::HashSet<usize> = best.into_values().collect();
    let mut i = 0usize;
    out.retain(|_| {
        let r = keep.contains(&i);
        i += 1;
        r
    });
}

/// 同音词排序用的混合分。
///
/// 只看词典词频（jieba）会把书面/正式词排前面 —— 例如 lvxing 把「履行」排在
/// 「旅行」前面、yisheng 把「一声」排在「医生」前面。再掺一份语料词频
/// （语言模型的 unigram，训练语料是真实文本）就能掰回来。
fn blended_rank(freq: u64, word: &str, tier: u8) -> f64 {
    if freq == u64::MAX {
        return f64::INFINITY;   // 自定义短语永远排最前
    }
    let corpus = lm_uni().get(word).copied().unwrap_or(0) as f64;
    let base = 0.5 * ((freq as f64) + 1.0).ln() + 0.5 * (corpus + 1.0).ln();
    // 按来源置信度降权：机器枢轴猜的、只有英文释义的，不许压过有可靠来源的词。
    // （机器翻的"河水"频次 917，不该把有据可查的"喝水"302 顶下去）
    let penalty = match tier {
        1 => 1.0,
        2 => 2.5,
        _ => 0.0,
    };
    base - penalty
}

/// 按无声调拼音精确/前缀查找；也支持首字母缩写（zg → 中国）。
///
/// 候选池放到「翻页能翻到底」的规模（v4.2.0）：精确组整组都收，
/// 1~2 字母前缀走预热的按词频排的表。之前只留 max_cands*3 条，
/// 用户看到的就永远是那八九个，翻不动。
pub fn lookup(ascii: &str) -> Vec<Hit> {
    if ascii.is_empty() {
        return Vec::new();
    }
    let idx = index();
    let mut out: Vec<Hit> = Vec::new();

    // 1) 精确：整个读音组都收（最大的一组「yi」557 条）
    for (_, line) in exact_group(idx, ascii) {
        if let Some(h) = hit_of(line) {
            out.push(h);
        }
        if out.len() >= MAX_HITS {
            break;
        }
    }
    // 2) 前缀：只有精确组是【空的】才用 —— 输入已经是一个完整读音
    //    （li / han / xian）时只给这个读音，和微软一致。
    //    不拦的话："li" 会把 liao 的「了」（词频 88 万）顶到第一个；
    //    "han" 会把 hang 的「行/航」混进来。
    if out.is_empty() {
        let nchars = ascii.chars().count();
        if nchars <= 2 {
            // 1~2 个字母走高频表：全前缀空间里词频最高的一批。
            // （旧实现按拼音字母序取前 16 条，打 y 先出来一堆 ya 系词）
            if let Some(list) = idx.prefix_top.get(ascii) {
                for (line, _) in list {
                    if out.len() >= MAX_HITS {
                        break;
                    }
                    if let Some(h) = hit_of(line) {
                        out.push(h);
                    }
                }
            }
        } else {
            // 3 个字母以上：词量已经不大，顺序扫「以它开头、比它长」的词条
            // （nih → nihao 你好 这种打一半的整词）
            let start = idx.exact.partition_point(|(k, _)| *k < ascii);
            let mut n = 0usize;
            for (key, line) in idx.exact[start..].iter() {
                if !key.starts_with(ascii) {
                    break;
                }
                if *key == ascii {
                    continue;
                }
                if let Some(h) = hit_of(line) {
                    out.push(h);
                    n += 1;
                }
                if n >= PREFIX_SCAN || out.len() >= MAX_HITS {
                    break;
                }
            }
        }
    }
    // 3) 首字母缩写：只要候选池没满就补 —— 微软也是把缩写候选和拼音候选
    //    混在一起按词频排。以前"池里 ≥9 条就不补"太粗：fa 的精确组一满，
    //    方案（f+a）就永远出不来；现在 fa 的精确组 + 缩写一起排，发/法在前，
    //    方案跟在后面 —— 和微软一致。
    //    上限放到 12 个字母：以前 7 字母以上的长缩写（zhrmghg→中华人民共和国、
    //    zgrmjfj→中国人民解放军）直接出不来 —— "多打几个字母候选就没了"
    //    的主因之一。深度 160 条：da 这种精确组本身就 70 多条的输入，
    //    缩写得能接着补进去（答案 d+a 才进得来），也够翻十几页看全部。
    //    池子已经很大的输入（yi/zh 几百条）进循环第一下就撞上限退出，
    //    不会污染拼音候选。
    if ascii.len() >= 2 && ascii.len() <= 12 {
        let start = idx.ini.partition_point(|(k, _)| *k < ascii);
        for (key, line) in idx.ini[start..].iter() {
            if !key.starts_with(ascii) {
                break;
            }
            if out.len() >= 160 {
                break;
            }
            if let Some(h) = hit_of(line) {
                out.push(h);
            }
        }
    }

    // 3.5) 整句切词（≥5 字母）—— 提前切好：切出来的"尾巴"补全候选
    //      要进池参与排序，整句本体稍后放最前。
    //      旧版整串切不完就什么都不给（zhongguorenm 之类前面全是空窗），
    //      尾巴补全保证窗口永远有候选 —— "多打几个字母候选就没了"的另一半。
    let sentence_plan = if ascii.chars().count() >= 5 {
        sentence(ascii)
    } else {
        None
    };
    if let Some((_, tail)) = &sentence_plan {
        if !tail.is_empty() {
            for h in prefix_hits(tail, 8) {
                out.push(h);
                if out.len() >= MAX_HITS {
                    break;
                }
            }
        }
    }

    // 同一个词只留一条（见 dedup_words 的注释）
    dedup_words(&mut out);

    // 同音词重排（稳定排序：同分保持原有顺序）。
    // 预先给每条算好一次 blended_rank —— 原来是 sort_by 里每比较一次算两遍，
    // 候选从十几条放大到几百条后这里会变成热点。
    let mut ranked: Vec<(f64, Hit)> = out
        .drain(..)
        .map(|h| (blended_rank(h.freq, &h.word, h.tier), h))
        .collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<Hit> = ranked.into_iter().map(|(_, h)| h).collect();

    // 短语 + 整句候选放最前面。用 head 记下插到头部这几条的条数，
    // 下面自学习排序只动 head 之后的部分 —— 否则学过的单字（米/个/高
    // 这类高频字）会把整句拼出来的答案顶下去：打 zhongguorenm 想要
    // 中国人，学过一次「米」之后按 1 就变成米了（v4.2.2 实测翻车）。
    let mut head: usize = 0;

    // 自定义短语优先（dz → 电子邮箱）
    if let Some(h) = phrase(ascii) {
        out.insert(0, h);
        head += 1;
    }

    // 整句候选放最前面：整串切成 ≥2 个词，或单个多字词带尾巴
    // （zhongguom → 中国 + m）。单字带尾巴（nih → 你+h）不插 ——
    // 这种时候前缀候选（你好）已经对了，插个「你」反而把对的顶下去。
    // 查重：输入本身就是整词时，整句候选与精确候选是同一个词
    // （zhongguorenmin → 中国人民），不查重会看到两条一模一样的
    // （v4.2.2 修掉的旧 bug）。
    if let Some((parts, _)) = sentence_plan {
        let ok_insert =
            parts.len() >= 2 || (parts.len() == 1 && parts[0].word.chars().count() >= 2);
        if ok_insert {
            let word: String = parts.iter().map(|h| h.word.clone()).collect();
            if !out.iter().any(|h| h.word == word) {
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
                out.insert(0, Hit {
                    word,
                    pinyin: ascii.to_string(),
                    ru: rus.join(" | "),
                    freq: 0,
                    tier: 0,
                });
                head += 1;
            }
        }
    }

    // 4) 自学习：选过的词排前面（只排 head 之后的普通候选，
    //    短语/整句候选保持头部 —— 见 head 上面的注释）
    let learned = idx.learned.lock().unwrap();
    if !learned.is_empty() && head < out.len() {
        let mut rest = out.split_off(head);
        rest.sort_by_key(|h| {
            let c = learned.get(&h.word).copied().unwrap_or(0);
            std::cmp::Reverse(c)
        });
        out.extend(rest);
    }
    out.truncate(MAX_HITS);
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 自定义短语的缓存。
///
/// 老实现是「每敲一个键就读一遍短语文件」——还带 env::var 取路径、exists() 探测、
/// 每次读写文件。这一项就占了每次按键 0.42ms 里的绝大部分，而且是无谓的磁盘 I/O。
/// 现在改成内存缓存：每 1 秒最多查一次文件是否被改过，改了自动重载，
/// 按键路径上完全不碰磁盘。
struct PhraseCache {
    path: String,
    stamp: Option<std::time::SystemTime>,
    loaded: bool,
    last_check_ms: u64,
    entries: Vec<(String, String, String)>, // 触发词, 中文, 俄语
}
static PHRASES: OnceLock<Mutex<PhraseCache>> = OnceLock::new();

/// 精确匹配用户自定义短语
pub fn phrase(trigger: &str) -> Option<Hit> {
    let cell = PHRASES.get_or_init(|| {
        Mutex::new(PhraseCache {
            path: phrases_file(),
            stamp: None,
            loaded: false,
            last_check_ms: 0,
            entries: Vec::new(),
        })
    });
    let mut g = cell.lock().unwrap();

    // 1 秒最多查一次文件有没有被改过；按键路径上不做任何 I/O
    let now = now_ms();
    let need_check = !g.loaded || now.saturating_sub(g.last_check_ms) >= 1000;
    if need_check {
        g.last_check_ms = now;
        let stamp = std::fs::metadata(&g.path).and_then(|m| m.modified()).ok();
        if !g.loaded || g.stamp != stamp {
            ensure_phrases_file();
            g.entries.clear();
            if let Ok(s) = std::fs::read_to_string(&g.path) {
                for line in s.lines() {
                    let line = line.trim_end();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    let c: Vec<&str> = line.split('\t').collect();
                    if c.len() < 2 {
                        continue;
                    }
                    let t = c[0].trim().to_string();
                    if t.is_empty() {
                        continue;
                    }
                    let word = c[1].trim().to_string();
                    let ru = c.get(2).map(|x| x.trim().to_string()).unwrap_or_default();
                    g.entries.push((t, word, ru));
                }
            }
            g.stamp = std::fs::metadata(&g.path).and_then(|m| m.modified()).ok();
            g.loaded = true;
        }
    }

    g.entries
        .iter()
        .find(|(t, _, _)| t.eq_ignore_ascii_case(trigger))
        .map(|(_, w, r)| Hit { word: w.clone(), pinyin: trigger.to_string(), ru: r.clone(), freq: u64::MAX, tier: 0 })
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

fn line_py(line: &str) -> &str {
    line.split('\t').nth(2).unwrap_or("")
}

/// 把一串无声调拼音切成词。
/// 返回 (切出的词序列, 没切完的尾巴)。尾巴是最后一个词只打了一半剩下的
/// 拼音（zhongguorenm → 中国/人 + "m"），整串切完时为空。切不出词时 None。
pub fn sentence(pinyin: &str) -> Option<(Vec<Hit>, String)> {
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
            // 取前 48 条再排序：词典文件本身按词频降序，同一个读音的前几十条
            // 就是最高频的那些（实测全库 17 万个读音组里只有 12 个组的前 6 名
            // 会因此不同）。不设上限的话，"yi"这种 500 多条的读音组每次转移
            // 都要解析+排序 500 行，长句子输入从 0.4ms 涨到 1.5ms。
            // sort_by_cached_key：词频只解析一次/行（sort_by_key 是每次比较都解析）。
            let mut alts: Vec<&'static str> =
                lines.iter().take(48).map(|(_, l)| *l).collect();
            alts.sort_by_cached_key(|l| std::cmp::Reverse(line_freq(l)));
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
            // 单字词旧版统一 -5：本意是压住被"作为字的频率"抬高的生僻字
            // （呣/進这种），但 我/你/是 这些高频字也被一起压死了 ——
            // woshi 会切出「卧室」而不是「我+是」。改成只在两种情况下罚：
            //   词频 < 120 的生僻单字 → 照罚 5（再叠下面的 -6，共 -11）
            //   高频单字 → 不罚，交给语料词频和 bigram 自己排序
            // jintiantianqihenhao 仍切「今天/天气/很好」：今天(词频15万+长词
            // 加成)本来就压得过 进/提/按，不靠这一刀。
            let single_pen = if wlen <= 1.0 && freq < 120.0 { 5.0 } else { 0.0 };
            let score = cur.0 + log_p
                + 2.5 * (wlen - 1.0)
                - single_pen
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
    // 整串切完了 → 尾巴为空，切分就是答案。
    // 例外：最后一个词是生僻单字（呣/啊这类词频 <120 的单字）时，
    // 多半是下一个词只打了一半被硬拼成了字（zhongguorenm → 中国人+呣），
    // 把最后一个词退回去当"尾巴"，候选才像微软（中国人 + m 的补全）。
    if let Some((_, seq)) = &best[n] {
        let last_is_junk = seq.len() >= 2
            && seq
                .last()
                .map_or(false, |l| line_word(l).chars().count() == 1 && line_freq(l) < 120);
        if last_is_junk {
            let last_chars = line_py(seq.last().unwrap()).chars().count();
            let p = n.saturating_sub(last_chars);
            if let Some((_, pseq)) = &best[p] {
                if !pseq.is_empty() {
                    let hits: Vec<Hit> = pseq.iter().filter_map(|l| hit_of(l)).collect();
                    let tail: String = chars[p..].iter().collect();
                    return Some((hits, tail));
                }
            }
        }
        if !seq.is_empty() {
            let hits: Vec<Hit> = seq.iter().filter_map(|l| hit_of(l)).collect();
            return Some((hits, String::new()));
        }
        // seq 是空的只会出现在 n=0（不可能），继续走下面的回退
    }
    // 整串切不完 → 最长可切前缀 + 尾巴（最后一个词只打了一半：
    // zhongguorenx → 中国人 + "x"）。这样窗口永远不会空。
    for p in (1..n).rev() {
        if let Some((_, pseq)) = &best[p] {
            if !pseq.is_empty() {
                let hits: Vec<Hit> = pseq.iter().filter_map(|l| hit_of(l)).collect();
                let tail: String = chars[p..].iter().collect();
                return Some((hits, tail));
            }
        }
    }
    None
}

/// 前缀补全：拼音以 ascii 开头、且比它更长的词条，最多 cap 条。
/// 1~2 个字母走预算好的高频表；更长顺序扫。整句尾巴补全用它
/// （zhongguorenm 的尾巴 "m" → 吗/们/没……）。
fn prefix_hits(ascii: &str, cap: usize) -> Vec<Hit> {
    let idx = index();
    let mut out: Vec<Hit> = Vec::with_capacity(cap.min(16));
    let nchars = ascii.chars().count();
    if nchars <= 2 {
        if let Some(list) = idx.prefix_top.get(ascii) {
            for (line, _) in list {
                if out.len() >= cap {
                    break;
                }
                if let Some(h) = hit_of(line) {
                    out.push(h);
                }
            }
        }
    } else {
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
            if out.len() >= cap {
                break;
            }
        }
    }
    out
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
