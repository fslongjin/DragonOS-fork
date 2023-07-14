use core::ffi::c_void;

use alloc::{string::String, vec::Vec};

use crate::{
    arch::{asm::current::current_pcb, MMArch},
    filesystem::vfs::MAX_PATHLEN,
    include::bindings::bindings::{
        pt_regs, set_system_trap_gate, CLONE_FS, CLONE_SIGNAL, CLONE_VM, USER_CS, USER_DS,
    },
    ipc::signal::sys_rt_sigreturn,
    kinfo,
    mm::{ucontext::AddressSpace, verify_area, MemoryManagementArch, VirtAddr},
    process::exec::{load_binary_file, ExecParam, ExecParamFlags},
    syscall::{
        user_access::{check_and_clone_cstr, check_and_clone_cstr_array},
        Syscall, SystemError, SYS_EXECVE, SYS_FORK, SYS_RT_SIGRETURN, SYS_VFORK,
    },
};

use super::{asm::ptrace::user_mode, mm::barrier::mfence};

extern "C" {
    fn do_fork(regs: *mut pt_regs, clone_flags: u64, stack_start: u64, stack_size: u64) -> u64;
    // fn c_sys_execve(
    //     path: *const u8,
    //     argv: *const *const u8,
    //     envp: *const *const u8,
    //     regs: &mut pt_regs,
    // ) -> u64;

    fn syscall_int();
}

macro_rules! syscall_return {
    ($val:expr, $regs:expr) => {{
        let ret = $val;
        $regs.rax = ret as u64;
        return;
    }};
}

#[no_mangle]
pub extern "C" fn syscall_handler(regs: &mut pt_regs) -> () {
    let syscall_num = regs.rax as usize;
    let args = [
        regs.r8 as usize,
        regs.r9 as usize,
        regs.r10 as usize,
        regs.r11 as usize,
        regs.r12 as usize,
        regs.r13 as usize,
        regs.r14 as usize,
        regs.r15 as usize,
    ];
    mfence();
    mfence();
    let from_user = user_mode(regs);

    // 由于进程管理未完成重构，有些系统调用需要在这里临时处理，以后这里的特殊处理要删掉。
    match syscall_num {
        SYS_FORK => unsafe {
            syscall_return!(do_fork(regs, 0, regs.rsp, 0), regs);
        },
        SYS_VFORK => unsafe {
            syscall_return!(
                do_fork(
                    regs,
                    (CLONE_VM | CLONE_FS | CLONE_SIGNAL) as u64,
                    regs.rsp,
                    0,
                ),
                regs
            );
        },
        SYS_EXECVE => {
            let path_ptr = args[0];
            let argv_ptr = args[1];
            let env_ptr = args[2];

            // 权限校验
            if from_user
                && (verify_area(VirtAddr::new(path_ptr), MAX_PATHLEN).is_err()
                    || verify_area(VirtAddr::new(argv_ptr), MAX_PATHLEN).is_err()
                    || verify_area(VirtAddr::new(env_ptr), MAX_PATHLEN).is_err())
            {
                syscall_return!(SystemError::EFAULT.to_posix_errno() as u64, regs);
            } else {
                syscall_return!(
                    tmp_rs_execve(
                        path_ptr as *const u8,
                        argv_ptr as *const *const u8,
                        env_ptr as *const *const u8,
                        regs,
                    )
                    .map(|_| 0)
                    .map_err(|e| e.to_posix_errno() as u64)
                    .unwrap_or_else(|e| e),
                    regs
                );
            }
        }

        SYS_RT_SIGRETURN => {
            syscall_return!(sys_rt_sigreturn(regs), regs);
        }
        // SYS_SCHED => {
        //     syscall_return!(sched(from_user) as u64, regs);
        // }
        _ => {}
    }
    syscall_return!(Syscall::handle(syscall_num, &args, from_user) as u64, regs);
}

/// 系统调用初始化
pub fn arch_syscall_init() -> Result<(), SystemError> {
    kinfo!("arch_syscall_init\n");
    unsafe { set_system_trap_gate(0x80, 0, syscall_int as *mut c_void) }; // 系统调用门
    return Ok(());
}

/// 临时的execve系统调用实现，以后要把它改为普通的系统调用。
///
/// 现在放在这里的原因是，还没有重构中断管理模块，未实现TrapFrame这个抽象，
/// 导致我们必须手动设置中断返回时，各个寄存器的值，这个过程很繁琐，所以暂时放在这里。
fn tmp_rs_execve(
    path: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
    regs: &mut pt_regs,
) -> Result<(), SystemError> {
    if path.is_null() || argv.is_null() || envp.is_null() {
        return Err(SystemError::EFAULT);
    }

    kinfo!("path: {:p}\n", path);
    kinfo!("argv: {:p}\n", argv);
    kinfo!("envp: {:p}\n", envp);
    let path: String = check_and_clone_cstr(path, Some(MAX_PATHLEN))?;
    let argv: Vec<String> = check_and_clone_cstr_array(argv)?;
    let envp: Vec<String> = check_and_clone_cstr_array(envp)?;

    // 释放原来的用户地址空间
    unsafe {
        current_pcb().drop_address_space();
    }
    // 创建新的地址空间并设置为当前地址空间
    let address_space = AddressSpace::new()?;
    unsafe {
        current_pcb().set_address_space(address_space.clone());
    }
    assert!(
        AddressSpace::is_current(&address_space),
        "Failed to set address space"
    );

    // 切换到新的用户地址空间
    unsafe {
        MMArch::set_table(
            crate::mm::PageTableKind::User,
            address_space.read().user_mapper.utable.table().phys(),
        )
    };

    let mut param = ExecParam::new(path.as_str(), address_space.clone(), ExecParamFlags::EXEC);
    // 加载可执行文件
    load_binary_file(&mut param)?;

    param.init_info_mut().args = argv;
    param.init_info_mut().envs = envp;

    // 把proc_init_info写到用户栈上

    let (user_sp, argv_ptr) = unsafe {
        param
            .init_info()
            .push_at(
                address_space
                    .write()
                    .user_stack_mut()
                    .expect("No user stack found"),
            )
            .expect("Failed to push proc_init_info to user stack")
    };

    // （兼容旧版libc）把argv的指针写到寄存器内
    // TODO: 改写旧版libc，不再需要这个兼容
    regs.rdi = param.init_info().args.len() as u64;
    regs.rsi = argv_ptr.data() as u64;

    // 设置系统调用返回时的寄存器状态
    // TODO: 中断管理重构后，这里的寄存器状态设置要删掉！！！改为对trap frame的设置。要增加架构抽象。
    regs.rsp = user_sp.data() as u64;
    regs.rbp = user_sp.data() as u64;

    regs.cs = USER_CS as u64 | 3;
    regs.ds = USER_DS as u64 | 3;
    regs.ss = USER_DS as u64 | 3;
    regs.es = 0;
    regs.rflags = 0x200;
    regs.rax = 1;

    return Ok(());
}