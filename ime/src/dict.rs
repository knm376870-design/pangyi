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

struct Index {
    exact: HashMap<&'static str, Vec<&'static str>>,
    keys: Vec<&'static str>,
    initials: HashMap<&'static str, Vec<&'static str>>,
    ini_keys: Vec<&'static str>,
    learned: Mutex<HashMap<String, u32>>,
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
        let mut exact: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        let mut initials: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        for line in RAW.lines().skip(1) {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() < 7 {
                continue;
            }
            let py = c[2];
            if py.is_empty() {
                continue;
            }
            exact.entry(py).or_default().push(line);
            if let Some(ini) = c.get(7) {
                if !ini.is_empty() {
                    initials.entry(ini).or_default().push(line);
                }
            }
        }
        let mut keys: Vec<&'static str> = exact.keys().copied().collect();
        keys.sort_unstable();
        let mut ini_keys: Vec<&'static str> = initials.keys().copied().collect();
        ini_keys.sort_unstable();
        Index { exact, keys, initials, ini_keys, learned: Mutex::new(load_learned()) }
    })
}

fn hit_of(line: &'static str) -> Option<Hit> {
    let c: Vec<&str> = line.split('\t').collect();
    if c.len() < 7 {
        return None;
    }
    Some(Hit { word: c[0].into(), pinyin: c[3].into(), ru: c[6].into() })
}

fn push_lines(lines: &[&'static str], out: &mut Vec<Hit>, cap: usize) {
    for line in lines {
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
    if let Some(lines) = idx.exact.get(ascii) {
        push_lines(lines, &mut out, 16);
    }
    // 2) 前缀（二分定位）
    if out.len() < 16 {
        let start = idx.keys.partition_point(|k| *k < ascii);
        for key in idx.keys[start..].iter() {
            if !key.starts_with(ascii) {
                break;
            }
            if *key == ascii {
                continue;
            }
            if let Some(lines) = idx.exact.get(key) {
                push_lines(lines, &mut out, 16);
            }
            if out.len() >= 16 {
                break;
            }
        }
    }
    // 3) 首字母缩写（输入全是字母且不长时才用）
    if out.len() < 9 && ascii.len() >= 2 && ascii.len() <= 6 {
        let start = idx.ini_keys.partition_point(|k| *k < ascii);
        for key in idx.ini_keys[start..].iter() {
            if !key.starts_with(ascii) {
                break;
            }
            if let Some(lines) = idx.initials.get(key) {
                push_lines(lines, &mut out, 16);
            }
            if out.len() >= 16 {
                break;
            }
        }
    }

    out.truncate(16);

    // 4) 自学习：选过的词排前面
    let learned = idx.learned.lock().unwrap();
    if !learned.is_empty() {
        out.sort_by_key(|h| {
            let c = learned.get(&h.word).copied().unwrap_or(0);
            std::cmp::Reverse(c)
        });
    }
    out
}

/// 补全：在「拼音以 ascii 开头、且比它更长」的词条里，挑最高频的那个。
pub fn complete(ascii: &str) -> Option<(String, String)> {
    if ascii.is_empty() {
        return None;
    }
    let idx = index();
    let start = idx.keys.partition_point(|k| *k < ascii);
    let mut best: Option<(u64, &'static str)> = None;
    for key in idx.keys[start..].iter() {
        if !key.starts_with(ascii) {
            break;
        }
        if *key == ascii {
            continue;
        }
        if let Some(lines) = idx.exact.get(key) {
            for line in lines {
                let c: Vec<&str> = line.split('\t').collect();
                if c.len() < 7 {
                    continue;
                }
                let freq: u64 = c[5].parse().unwrap_or(0);
                let wlen = c[0].chars().count() as u64;
                let score = freq * 10 + wlen;
                if best.map(|(s, _)| score > s).unwrap_or(true) {
                    best = Some((score, line));
                }
            }
        }
    }
    best.map(|(_, line)| {
        let c: Vec<&str> = line.split('\t').collect();
        (c[0].to_string(), c[3].to_string())
    })
}