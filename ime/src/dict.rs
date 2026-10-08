//! 词典：嵌入的「拼音 -> 中文候选 + 俄语译词」表。
//!
//! 表有 5 万多条，逐行扫太慢（每次按键都要查），所以第一次用时建一次索引：
//!   精确查找用 HashMap，前缀查找用排好序的键做二分。
use std::collections::HashMap;
use std::sync::OnceLock;

static RAW: &str = include_str!("dict.tsv");

pub struct Hit {
    pub word: String,
    pub pinyin: String,
    pub ru: String,
}

struct Index {
    /// 无声调拼音 -> 该拼音的所有行
    exact: HashMap<&'static str, Vec<&'static str>>,
    /// 排好序的拼音键，用于前缀查找
    keys: Vec<&'static str>,
}

fn index() -> &'static Index {
    static IDX: OnceLock<Index> = OnceLock::new();
    IDX.get_or_init(|| {
        let mut exact: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        for line in RAW.lines().skip(1) {
            let mut it = line.split('\t');
            let _word = it.next();
            let _trad = it.next();
            let py = match it.next() {
                Some(p) if !p.is_empty() => p,
                _ => continue,
            };
            exact.entry(py).or_default().push(line);
        }
        let mut keys: Vec<&'static str> = exact.keys().copied().collect();
        keys.sort_unstable();
        Index { exact, keys }
    })
}

fn hit_of(line: &'static str) -> Option<Hit> {
    let c: Vec<&str> = line.split('\t').collect();
    if c.len() < 7 {
        return None;
    }
    Some(Hit {
        word: c[0].into(),
        pinyin: c[3].into(),
        ru: c[6].into(),
    })
}

/// 按无声调拼音精确/前缀查找候选。
pub fn lookup(ascii: &str) -> Vec<Hit> {
    if ascii.is_empty() {
        return Vec::new();
    }
    let idx = index();
    let mut out: Vec<Hit> = Vec::new();

    // 1) 精确
    if let Some(lines) = idx.exact.get(ascii) {
        for line in lines {
            if let Some(h) = hit_of(line) {
                out.push(h);
            }
        }
    }

    // 2) 前缀（二分找到起点，然后顺序扫到不再是这个前缀为止）
    let start = idx.keys.partition_point(|k| *k < ascii);
    for key in idx.keys[start..].iter() {
        if !key.starts_with(ascii) {
            break;
        }
        if *key == ascii {
            continue; // 精确的已经收过了
        }
        if let Some(lines) = idx.exact.get(key) {
            for line in lines {
                if let Some(h) = hit_of(line) {
                    out.push(h);
                }
                if out.len() >= 16 {
                    break;
                }
            }
        }
        if out.len() >= 16 {
            break;
        }
    }

    out.truncate(16);
    out
}