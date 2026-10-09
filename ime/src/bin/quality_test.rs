//! 候选质量基准：敲一串常用拼音，看第一个候选是不是人想要的那个。
//! 统计 top-1 / top-3 / top-9 命中率。
const CASES: &[(&str, &str)] = &[
    ("nihao", "你好"), ("zhongguo", "中国"), ("beijing", "北京"), ("shanghai", "上海"),
    ("xiexie", "谢谢"), ("duibuqi", "对不起"), ("meiguanxi", "没关系"), ("zaijian", "再见"),
    ("qingwen", "请问"), ("shenme", "什么"), ("weishenme", "为什么"), ("zenme", "怎么"),
    ("keyi", "可以"), ("zhidao", "知道"), ("wanshang", "晚上"), ("zaoshang", "早上"),
    ("jintian", "今天"), ("mingtian", "明天"), ("zuotian", "昨天"), ("xianzai", "现在"),
    ("shijian", "时间"), ("gongzuo", "工作"), ("xuexi", "学习"), ("xuesheng", "学生"),
    ("laoshi", "老师"), ("pengyou", "朋友"), ("yisheng", "医生"), ("yiyuan", "医院"),
    ("dianhua", "电话"), ("shouji", "手机"), ("diannao", "电脑"), ("wangluo", "网络"),
    ("wenti", "问题"), ("banfa", "办法"), ("bangzhu", "帮助"), ("xuyao", "需要"),
    ("xihuan", "喜欢"), ("kuaile", "快乐"), ("gaoxing", "高兴"), ("chifan", "吃饭"),
    ("heshui", "喝水"), ("shuijiao", "睡觉"), ("qichuang", "起床"), ("mafan", "麻烦"),
    ("zhuyi", "注意"), ("anquan", "安全"), ("jiankang", "健康"), ("shenghuo", "生活"),
    ("gongsi", "公司"), ("jingli", "经理"), ("huiyi", "会议"), ("jihua", "计划"),
    ("dianying", "电影"), ("yinyue", "音乐"), ("lvxing", "旅行"), ("tianqi", "天气"),
    ("dongxi", "东西"), ("difang", "地方"), ("shihou", "时候"), ("yisi", "意思"),
];

fn main() {
    pangyi_ime::dict::warmup();
    let (mut h1, mut h3, mut h9, mut miss) = (0, 0, 0, 0);
    let mut bad: Vec<String> = Vec::new();
    for (py, want) in CASES {
        let hits = pangyi_ime::dict::lookup(py);
        let pos = hits.iter().position(|h| h.word == *want);
        match pos {
            Some(0) => h1 += 1,
            Some(1..=2) => h3 += 1,
            Some(3..=8) => h9 += 1,
            _ => {
                miss += 1;
                let top: Vec<&str> = hits.iter().take(3).map(|h| h.word.as_str()).collect();
                bad.push(format!("  {py:<12} 想要 {want:<6} 实际前3: {}", top.join(" / ")));
            }
        }
        if pos.is_some() && pos.unwrap() < 9 { h9 += 0; }
    }
    let mid = h3 + h9;
    let n = CASES.len();
    println!("用例 {n} 条");
    println!("  第一个就是想要的 (top-1): {h1:>3}  {:>5.1}%", h1 as f64 * 100.0 / n as f64);
    println!("  在第 2~9 个         : {mid:>3}  {:>5.1}%", mid as f64 * 100.0 / n as f64);
    println!("  前 9 个里都没有      : {miss:>3}  {:>5.1}%", miss as f64 * 100.0 / n as f64);
    if !bad.is_empty() {
        println!("\n没进前 9 的用例:");
        for b in bad { println!("{b}"); }
    }
}