//! 词典自查：直接查几个拼音，看能不能出中文和俄语。
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keys = if args.is_empty() {
        vec!["kaifa".to_string(), "xiexie".to_string(), "zhongguo".to_string(), "yinggai".to_string()]
    } else {
        args
    };
    for k in keys {
        let hits = pangyi_ime::dict::lookup(&k);
        println!("{k} -> {} 个候选", hits.len());
        for h in hits.iter().take(6) {
            println!("    {}  {}  {}", h.word, h.pinyin, h.ru);
        }
    }
}