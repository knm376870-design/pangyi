//! 热路径耗时 + 内存占用测量
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::GetCurrentProcess;

fn priv_mb() -> f64 {
    unsafe {
        let mut c = PROCESS_MEMORY_COUNTERS::default();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb).is_ok() {
            c.PagefileUsage as f64 / 1048576.0
        } else { -1.0 }
    }
}

fn ws_mb() -> f64 {
    unsafe {
        let mut c = PROCESS_MEMORY_COUNTERS::default();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb).is_ok() {
            c.WorkingSetSize as f64 / 1048576.0
        } else { -1.0 }
    }
}

fn main() {
    println!("启动后(未建索引) 工作集 {:.1} MB / 私有 {:.1} MB", ws_mb(), priv_mb());
    let t0 = std::time::Instant::now();
    pangyi_ime::dict::warmup();
    let w = t0.elapsed();
    println!("预热(索引+语言模型) 耗时: {:?}", w);
    println!("预热后 工作集 {:.1} MB / 私有 {:.1} MB", ws_mb(), priv_mb());
    println!();
    println!("{:>7}  {:>14}  {:>12}", "前缀", "平均每次", "候选数");
    for p in ["z", "zh", "k", "n", "w", "woaini",
              "womenshizhongguoren", "jintiantianqihenhao",
              "womenshizhonghuarenmingongheguo",
              "qingnibangwomenkanyixiazhegedianying"] {
        let t = std::time::Instant::now();
        let mut n = 0usize;
        let iters: usize = if p.len() > 12 { 20 } else { 200 };
        for _ in 0..iters { n += pangyi_ime::dict::lookup(p).len(); let _ = pangyi_ime::dict::complete(p); }
        let d = t.elapsed();
        println!("{p:>36}  {:>14?}  {:>4}", d / iters as u32, n / iters);
    }
}