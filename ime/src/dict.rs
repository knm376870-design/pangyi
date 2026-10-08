//! 词典：嵌入的「拼音 -> 中文候选 + 俄语译词」表。
static RAW: &str = include_str!("dict.tsv");

pub struct Hit {
    pub word: String,
    pub pinyin: String,
    pub ru: String,
}

/// 按无声调拼音精确/前缀查找候选。
pub fn lookup(ascii: &str) -> Vec<Hit> {
    let mut exact = Vec::new();
    let mut prefix = Vec::new();
    for line in RAW.lines().skip(1) {
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() < 7 {
            continue;
        }
        if c[2] == ascii {
            exact.push(Hit { word: c[0].into(), pinyin: c[3].into(), ru: c[6].into() });
        } else if prefix.len() < 16 && c[2].starts_with(ascii) {
            prefix.push(Hit { word: c[0].into(), pinyin: c[3].into(), ru: c[6].into() });
        }
    }
    exact.extend(prefix);
    exact.truncate(16);
    exact
}
