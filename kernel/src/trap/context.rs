//! 中断上下文信息
//! 
//! 在 trap.S 中，会读取/保存中断前的所有上下文信息，表示为 TrapContext 结构
//! 注意 trap.S 中的读写操作对应本文件中 TrapContext 的定义，但编译器不会检查汇编，所以你需要手动保证两者之间是对应的。
//! 即修改 TrapContext 的定义时需要对应修改 trap.S

#![deny(missing_docs)]

use riscv::register::sstatus::{self, Sstatus, SPP};
/// 异常/中断上下文
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrapContext {
    /// 普通寄存器。
    /// 异常/中断发生的的时候不一定是系统调用，也可能是时钟中断、缺页等，所以不能认为它是一种"函数调用"。
    /// 所以上下文里要保留所有寄存器，而不只是 callee save 的
    pub x: [usize; 32],
    /// CSR 寄存器 sstatus，含中断的信息
    pub sstatus: Sstatus,
    /// CSR 寄存器 sepc，表示发生中断的位置
    pub sepc: usize,
}

impl TrapContext {
    /// 设置 sp 寄存器
    pub fn set_sp(&mut self, sp: usize) {
        self.x[2] = sp;
    }
    /// 设置 a0 寄存器。
    /// 对于 sys_exec，它是参数 argc
    pub fn set_a0(&mut self, a0: usize) {
        self.x[10] = a0;
    }
    /// 设置 a1 寄存器。
    /// 对于 sys_exec，它是参数 argv
    pub fn set_a1(&mut self, a1: usize) {
        self.x[11] = a1;
    }
    /// 初始化用户程序的中断信息，用于第一次进入用户程序前
    pub fn app_init_context(entry: usize, sp: usize) -> Self {
        println!("init app entry {:x} sp {:x}", entry, sp);
        let mut sstatus = sstatus::read(); // 记录此时的 sstatus 寄存器
        sstatus.set_spp(SPP::User); // 把中断推出后的模式改为用户模式
        let mut cx = Self {
            x: [0; 32],
            sstatus,
            sepc: entry, // sepc 设为用户程序入口
        };
        cx.set_sp(sp); // 设置用户栈地址
        cx // return initial Trap Context of app
    }
    /// 初始化用户程序的中断信息，并设置用户程序执行时的参数
    pub fn app_exec_context(entry: usize, sp: usize, argc: usize, argv: usize) -> Self {
        let mut cx = Self::app_init_context(entry, sp);
        cx.set_a0(argc);
        cx.set_a1(argv);
        cx
    }
    /// 空的 TrapContext
    pub fn new() -> Self {
        Self {
            x: [0; 32],
            sstatus: sstatus::read(),
            sepc: 0,
        }
    }
    
}
