//! 整句 DP 的直接检查（排查用）：打印切分结果和每段的得分依据。
//!
//! 用法: sent_test.exe [拼音串...]   （不给参数就用默认的一串）
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let keys = if args.is_empty() {
        vec!["womenmingtianqubeijing".to_string()]
    } else {
        args
    };
    for k in keys {
        match pangyi_ime::dict::sentence(&k) {
            Some((hits, tail)) => {
                let w: String = hits.iter().map(|h| h.word.as_str()).collect();
                if tail.is_empty() {
                    println!("{k} -> {w}");
                } else {
                    println!("{k} -> {w} + [{tail}]");
                }
                for h in &hits {
                    println!("    {:>6}  {:>10}  {}", h.word, h.pinyin, h.ru);
                }
            }
            None => println!("{k} -> 无整句候选"),
        }
    }
}
