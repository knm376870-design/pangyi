//! 热路径耗时测量：每次按键要跑的 lookup + complete
fn main() {
    let t0 = std::time::Instant::now();
    pangyi_ime::dict::warmup();
    println!("预热(词典索引 + 20万条 bigram 语言模型) 一次性耗时: {:?}", t0.elapsed());
    println!();
    println!("{:>7}  {:>14}  {:>16}  {}", "前缀", "100次总耗时", "平均每次", "候选数");
    for p in ["k", "ka", "kai", "kaif", "kaifa", "n", "ni", "nih", "nihao", "z", "zh", "zho", "zhon", "zhong"] {
        let t = std::time::Instant::now();
        let mut n = 0usize;
        for _ in 0..100 {
            n += pangyi_ime::dict::lookup(p).len();
            let _ = pangyi_ime::dict::complete(p);
        }
        let d = t.elapsed();
        println!("{p:>7}  {:>14?}  {:>16?}  {}", d, d / 100, n / 100);
    }
}