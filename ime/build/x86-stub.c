/* i686-pc-windows-gnu + llvm-mingw：Rust 的 rsend.o 引用 GCC 时代的帧注册符号，
   llvm-mingw 用 libunwind 不提供。本工程是 panic=abort 的 cdylib，这里给空实现。
   注意 i686 的 C 符号带一个前导下划线，所以这里写两个下划线。 */
void __register_frame_info(void *a, void *b) { (void)a; (void)b; }
void __deregister_frame_info(void *a) { (void)a; }
void __register_frame_info_bases(void *a, void *b, void *c, void *d) { (void)a; (void)b; (void)c; (void)d; }
void __register_frame_info_table(void *a, void *b) { (void)a; (void)b; }
void __deregister_frame_info_bases(void *a) { (void)a; }