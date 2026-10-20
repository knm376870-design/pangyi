//! 表情候选（v4.15.0）：拼音关键词 → emoji / 颜文字。
//!
//! 表在 emoji.tsv（编译期打进 DLL，零运行时文件）：`拼音<TAB>表情<TAB>标签`。
//! 同一个拼音占多行 = 一组表情候选。**正好打满**关键词拼音才出（xiao 出、
//! xiaof 不出），错字不干扰正常候选；表情模式（设置面板）关时一个不出。
//!
//! 候选形状：中文列 = 表情本身（所见即所得，上屏就是它），俄语列 = 标签
//! （开心/颜文字·开心）。选中提交走正常候选路径；表情不进自学习、没有
//! 词典释义，逐字翻译查不到就原样保留（v4.4.0 起的「没译文的词不丢字」）。

use std::collections::HashMap;
use std::sync::OnceLock;

pub struct Emoji {
    /// 上屏内容（emoji 字符或颜文字串）
    pub s: &'static str,
    /// 俄语列显示的标签（中文）
    pub label: &'static str,
}

const RAW: &str = include_str!("emoji.tsv");

fn table() -> &'static HashMap<&'static str, Vec<Emoji>> {
    static T: OnceLock<HashMap<&'static str, Vec<Emoji>>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m: HashMap<&'static str, Vec<Emoji>> = HashMap::new();
        for line in RAW.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.splitn(3, '\t');
            let (Some(py), Some(s), label) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            if py.is_empty() || s.is_empty() {
                continue;
            }
            m.entry(py)
                .or_default()
                .push(Emoji { s, label: label.unwrap_or("表情") });
        }
        m
    })
}

/// 输入串正好等于某个关键词 → 该组表情候选
pub fn lookup(key: &str) -> Option<&'static Vec<Emoji>> {
    table().get(key)
}

/// 这个词是不是表情表里的（上屏路径用来跳过自学习：表情进 learned.txt
/// 会变成「关掉表情模式还会蹦出来」的怪候选）
pub fn is_emoji(s: &str) -> bool {
    static SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        table()
            .values()
            .flat_map(|v| v.iter())
            .map(|e| e.s)
            .collect()
    })
    .contains(s)
}
